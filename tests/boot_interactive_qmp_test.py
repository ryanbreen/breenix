"""Run the launcher with stub host services and inspect the QEMU command line it builds."""

import json
import os
from pathlib import Path
import pwd
import re
import shlex
import shutil
import subprocess
import tempfile
import unittest
import uuid


ROOT = Path(__file__).resolve().parents[1]
BOOT_ID = "boot-123"
BOOTS = "Library/Application Support/Vigil/breenix/boots"
SOCKET_SUFFIX = f"/{BOOTS}/{BOOT_ID}/qmp.sock"


class BootInteractiveQmpTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="vq-", dir="/tmp")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name).resolve()
        scripts = self.root / "scripts"
        scripts.mkdir()
        shutil.copyfile(ROOT / "scripts/boot-interactive.sh", scripts / "boot-interactive.sh")
        shutil.copyfile(ROOT / "scripts/suite-sequence.py", scripts / "suite-sequence.py")
        shutil.copytree(ROOT / "docs", self.root / "docs")
        self.executable(scripts / "write-boot-target.sh", '#!/bin/sh\nexit 0\n')
        (scripts / "host-slots.sh").write_text("\n".join(
            f"{name}() {{ :; }}" for name in (
                "host_slots_start", "host_slot_acquire", "host_slot_serial",
                "host_slot_quiet", "host_slot_header")))
        lock = self.root / "docker/qemu/lib/qemu-host-lock.sh"
        lock.parent.mkdir(parents=True)
        lock.write_text("qemu_host_lock_acquire() { :; }\nqemu_host_lock_track_pid() { :; }\n")
        # Like vigil-cli, registration makes the boot's folder under the home Vigil uses (STUB_BOOT_HOME).
        self.calls = self.root / "vigil-calls"
        self.executable(scripts / "vigil-record.sh",
                        '#!/bin/sh\n[ "$1" = start ] || exit 0\necho start >> "$CALLS"\n'
                        '[ -n "$STUB_VIGIL_ID" ] || exit 0\n'
                        f'for id in $(echo "$STUB_VIGIL_ID" | tr , " "); do [ -z "$STUB_BOOT_HOME" ] || mkdir -p "$STUB_BOOT_HOME/{BOOTS}/$id"; done\n'
                        'printf "%s" "$STUB_VIGIL_ID"\n')
        self.executable(scripts / "qemu-1x-app.sh", '#!/bin/sh\necho qemu-system-aarch64\n')
        (scripts / "qemu-arm64.py").write_text('print("default")\n')
        (scripts / "watch-qemu.py").write_text(
            'import json, os, pathlib, sys\n'
            'pathlib.Path(os.environ["CAPTURE"] + ".watch").write_text(json.dumps(sys.argv))\n'
            'pathlib.Path(os.environ["CAPTURE"]).write_text(json.dumps(sys.argv[sys.argv.index("--") + 1:]))\n')
        kernel = self.root / "target/aarch64-breenix-kernel/release/kernel-aarch64"
        kernel.parent.mkdir(parents=True)
        kernel.touch()
        (self.root / "target/ext2-aarch64.img").touch()
        self.home = self.root / "home"
        self.home.mkdir()
        self.vigil_home = self.root / "vh"
        self.vigil_home.mkdir()

    @staticmethod
    def executable(path, text):
        path.write_text(text)
        path.chmod(0o755)

    def launch(self, *args, boot_id=BOOT_ID, vigil_home="default", boot_home="default",
               mode=("--mode", "probe"), expected_code=0):
        """Boots with HOME and Vigil's home in different folders; vigil_home=None leaves VIGIL_TEST_HOME unset."""
        vigil_home = self.vigil_home if vigil_home == "default" else vigil_home
        boot_home = vigil_home if boot_home == "default" else boot_home
        env = {k: v for k, v in os.environ.items() if k not in ("VIGIL_TEST_HOME", "VIGIL_BOOT_ID")}
        env.update(HOME=str(self.home), STUB_VIGIL_ID=boot_id, STUB_BOOT_HOME=str(boot_home or ""),
                   CALLS=str(self.calls), CAPTURE=str(self.root / "args.json"))
        if vigil_home is not None:
            env["VIGIL_TEST_HOME"] = str(vigil_home)
        self.calls.unlink(missing_ok=True)
        result = subprocess.run(
            ["bash", str(self.root / "scripts/boot-interactive.sh"), "--no-build",
             *mode, "--serial-log", str(self.root / "serial.txt"), *args],
            cwd=self.root, env=env, capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, expected_code, result.stdout + result.stderr)
        if expected_code:
            return [], result.stderr
        return json.loads((self.root / "args.json").read_text()), result.stdout

    def test_terminal_kernel_rejects_completion_modes(self):
        kernel = self.root / "target/aarch64-breenix-kernel/release/kernel-aarch64"
        for profile in ("capture_oracle", "gpu_fault_acceptance"):
            kernel.write_text(profile)
            for mode in (("--mode", "suite", "--suite", "smoke"),
                         ("--mode", "tests"), ("--mode", "probe"),
                         ("--mode", "shell"), ("--mode", "desktop")):
                with self.subTest(profile=profile, mode=mode):
                    _, error = self.launch(mode=mode, expected_code=2)
                    self.assertIn("terminal profile needs --mode program", error)

    def test_firmware_panic_program_does_not_require_gpu_pixels(self):
        kernel = self.root / "target/aarch64-breenix-kernel/release/kernel-aarch64"
        kernel.write_text("capture_oracle")
        self.launch(mode=("--mode", "program", "--program", "/sbin/probe"))
        watcher = json.loads((self.root / "args.json.watch").read_text())
        self.assertEqual(watcher[watcher.index("--mode") + 1], "default")

    @staticmethod
    def with_qmp(argv, socket):
        """The command line with only QEMU's QMP option added, where the launcher places it."""
        at = argv.index("-device")
        return argv[:at] + ["-qmp", f"unix:{socket},server=on,wait=off"] + argv[at:]

    @staticmethod
    def notices(output):
        return [line for line in output.splitlines() if "Vigil QMP disabled" in line]

    def test_registered_boot_puts_the_socket_in_vigils_boot_folder_not_under_home(self):
        for display in ("--display", "--no-display"):
            with self.subTest(display=display):
                bare, _ = self.launch(display, boot_id="")
                argv, output = self.launch(display)
                socket = f"{self.vigil_home}{SOCKET_SUFFIX}"
                self.assertEqual(argv, self.with_qmp(bare, socket))
                self.assertTrue(Path(socket).parent.is_dir())
                self.assertEqual(self.notices(output), [])

    def test_multi_suite_boot_uses_the_first_registered_folder(self):
        argv, output = self.launch(boot_id="boot-a,boot-b", mode=("--mode", "suite", "--suite", "smoke,crypto"))
        self.assertIn(f"unix:{self.vigil_home}/{BOOTS}/boot-a/qmp.sock,server=on,wait=off", argv)
        self.assertEqual(self.notices(output), [])

    def test_a_missing_boot_folder_leaves_qmp_off_and_says_so_once(self):
        bare, _ = self.launch(boot_id="")
        argv, output = self.launch(boot_home=None)
        self.assertEqual(argv, bare)
        self.assertEqual(self.notices(output),
                         [f"==> Vigil QMP disabled: no boot folder at {self.vigil_home}/{BOOTS}/{BOOT_ID}"])

    def test_without_a_test_home_the_folder_is_the_account_home_not_home(self):
        # A launcher that trusted HOME would find this folder; Vigil made the boot under the account's home.
        boot_id = f"boot-{uuid.uuid4().hex}"
        bare, _ = self.launch(boot_id="", vigil_home=None)
        argv, output = self.launch(boot_id=boot_id, vigil_home=None, boot_home=self.home)
        self.assertEqual(argv, bare)
        account = pwd.getpwuid(os.getuid()).pw_dir
        self.assertNotEqual(account, str(self.home))
        self.assertEqual(self.notices(output),
                         [f"==> Vigil QMP disabled: no boot folder at {account}/{BOOTS}/{boot_id}"])

    def test_unregistered_boot_has_no_socket(self):
        argv, output = self.launch(boot_id="")
        self.assertNotIn("-qmp", argv)
        self.assertEqual(self.notices(output), [])

    def test_a_single_case_run_is_not_registered_and_has_no_socket(self):
        argv, output = self.launch("--case", "dirs/rename", mode=("--mode", "suite", "--suite", "directories"))
        self.assertFalse(self.calls.exists(), "a one-case run must not register with Vigil")
        self.assertNotIn("-qmp", argv)
        self.assertIn("name=opt/breenix/case,string=dirs/rename", argv)
        self.assertEqual(self.notices(output), [])

    def test_explicit_socket_keeps_callers_path_with_or_without_registration(self):
        for boot_id in (BOOT_ID, ""):
            for path in ("caller.sock", str(self.root / "absolute.sock")):
                with self.subTest(boot_id=boot_id, path=path):
                    bare, _ = self.launch(boot_id="")
                    argv, _ = self.launch("--qmp", path, boot_id=boot_id)
                    self.assertEqual(argv, self.with_qmp(bare, self.root / path))

    def test_explicit_empty_socket_remains_disabled(self):
        self.assertNotIn("-qmp", self.launch("--qmp", "")[0])

    def test_the_printed_screendump_hint_pastes_as_one_path(self):
        _, output = self.launch()
        hint = next(line for line in output.splitlines() if line.startswith("==> QMP: "))
        command = re.search(r"\((scripts/qmp-screendump\.py .*)\)$", hint).group(1)
        self.assertEqual(shlex.split(command),
                         ["scripts/qmp-screendump.py", f"{self.vigil_home}{SOCKET_SUFFIX}", "out.png"])

    def test_103_byte_path_is_allowed_and_104_byte_path_is_not(self):
        pad = 103 - len(f"{self.root}/{SOCKET_SUFFIX}".encode())
        home = self.root / ("h" * pad)
        self.assertEqual(len(f"{home}{SOCKET_SUFFIX}".encode()), 103)
        bare, _ = self.launch(boot_id="")
        self.assertEqual(self.launch(vigil_home=home)[0], self.with_qmp(bare, f"{home}{SOCKET_SUFFIX}"))
        argv, output = self.launch(vigil_home=Path(f"{home}h"))
        self.assertEqual(argv, bare)
        notices = self.notices(output)
        self.assertEqual(len(notices), 1)
        self.assertIn("exceeds 103 bytes", notices[0])

    def test_path_limit_counts_utf8_bytes(self):
        home = self.root / ("é" * ((103 - len(f"{self.root}/{SOCKET_SUFFIX}".encode())) // 2 + 1))
        self.assertLess(len(f"{home}{SOCKET_SUFFIX}"), 103)
        self.assertGreater(len(f"{home}{SOCKET_SUFFIX}".encode()), 103)
        argv, output = self.launch(vigil_home=home)
        self.assertNotIn("-qmp", argv)
        self.assertIn("exceeds 103 bytes", output)


if __name__ == "__main__":
    unittest.main()
