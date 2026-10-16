"""Run the launcher with stub host services and inspect its QEMU arguments."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
BOOT_ID = "boot-123"
SOCKET_SUFFIX = f"/Library/Application Support/Vigil/breenix/boots/{BOOT_ID}/qmp.sock"


class BootInteractiveQmpTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="vigil-qmp-", dir="/tmp")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name).resolve()
        scripts = self.root / "scripts"
        scripts.mkdir()
        shutil.copyfile(ROOT / "scripts/boot-interactive.sh", scripts / "boot-interactive.sh")
        (scripts / "host-slots.sh").write_text("\n".join(
            f"{name}() {{ :; }}" for name in (
                "host_slots_start", "host_slot_acquire", "host_slot_serial",
                "host_slot_quiet", "host_slot_header")))
        lock = self.root / "docker/qemu/lib/qemu-host-lock.sh"
        lock.parent.mkdir(parents=True)
        lock.write_text("qemu_host_lock_acquire() { :; }\nqemu_host_lock_track_pid() { :; }\n")
        self.executable(scripts / "vigil-record.sh",
                        '#!/bin/sh\n[ "$1" != start ] || printf "%s" "$STUB_VIGIL_ID"\n')
        self.executable(scripts / "qemu-1x-app.sh", '#!/bin/sh\necho qemu-system-aarch64\n')
        (scripts / "qemu-arm64.py").write_text('print("default")\n')
        (scripts / "watch-qemu.py").write_text(
            'import json, os, pathlib, sys\n'
            'pathlib.Path(os.environ["CAPTURE"]).write_text(json.dumps(sys.argv[sys.argv.index("--") + 1:]))\n')
        kernel = self.root / "target/aarch64-breenix-kernel/release/kernel-aarch64"
        kernel.parent.mkdir(parents=True)
        kernel.touch()
        (self.root / "target/ext2-aarch64.img").touch()
        self.home = self.root / "home"

    @staticmethod
    def executable(path, text):
        path.write_text(text)
        path.chmod(0o755)

    def launch(self, *args, boot_id=BOOT_ID, home=None):
        env = dict(os.environ, HOME=str(home or self.home), STUB_VIGIL_ID=boot_id,
                   CAPTURE=str(self.root / "args.json"))
        result = subprocess.run(
            ["bash", str(self.root / "scripts/boot-interactive.sh"), "--no-build",
             "--mode", "probe", "--serial-log", str(self.root / "serial.txt"), *args],
            cwd=self.root, env=env, capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        argv = json.loads((self.root / "args.json").read_text())
        qmp = [argv[i + 1] for i, arg in enumerate(argv) if arg == "-qmp"]
        return qmp, result.stdout

    def test_registered_boot_uses_vigil_socket_with_either_display(self):
        for display in ("--display", "--no-display"):
            with self.subTest(display=display):
                qmp, _ = self.launch(display)
                self.assertEqual(qmp, [f"unix:{self.home}{SOCKET_SUFFIX},server=on,wait=off"])

    def test_unregistered_boot_has_no_socket(self):
        self.assertEqual(self.launch(boot_id="")[0], [])

    def test_explicit_socket_keeps_callers_path_with_or_without_registration(self):
        for boot_id in (BOOT_ID, ""):
            for path in ("caller.sock", str(self.root / "absolute.sock")):
                with self.subTest(boot_id=boot_id, path=path):
                    qmp, _ = self.launch("--qmp", path, boot_id=boot_id)
                    expected = self.root / path
                    self.assertEqual(qmp, [f"unix:{expected},server=on,wait=off"])

    def test_explicit_empty_socket_remains_disabled(self):
        self.assertEqual(self.launch("--qmp", "")[0], [])

    def test_103_byte_path_is_allowed_and_104_byte_path_is_not(self):
        home = "/" + "h" * (103 - len(SOCKET_SUFFIX.encode()) - 1)
        self.assertEqual(self.launch(home=home)[0],
                         [f"unix:{home}{SOCKET_SUFFIX},server=on,wait=off"])
        qmp, output = self.launch(home=home + "h")
        self.assertEqual(qmp, [])
        notices = [line for line in output.splitlines() if "Vigil QMP disabled" in line]
        self.assertEqual(len(notices), 1)
        self.assertIn("exceeds 103 bytes", notices[0])

    def test_path_limit_counts_utf8_bytes(self):
        home = "/" + "é" * ((103 - len(SOCKET_SUFFIX.encode())) // 2 + 1)
        self.assertLess(len(home + SOCKET_SUFFIX), 103)
        self.assertGreater(len((home + SOCKET_SUFFIX).encode()), 103)
        qmp, output = self.launch(home=home)
        self.assertEqual(qmp, [])
        self.assertIn("exceeds 103 bytes", output)


if __name__ == "__main__":
    unittest.main()
