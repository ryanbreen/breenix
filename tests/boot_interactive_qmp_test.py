"""Run the launcher with stub host services and inspect the QEMU command line it builds."""

import hashlib
import json
import os
from pathlib import Path
import pwd
import re
import shlex
import shutil
import struct
import subprocess
import tempfile
import unittest
import uuid
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[1]
BOOT_ID = "boot-123"
BOOTS = "Library/Application Support/Vigil/breenix/boots"
SOCKET_SUFFIX = f"/{BOOTS}/{BOOT_ID}/qmp.sock"
ARM_K5_TWIN = os.environ.get("BREENIX_ARM_K5_TWIN")
ARM_K5_RECORDS = ROOT / "toolchain/rust-host"
ARM_K5_TARGET = ROOT / "userspace/programs/target"
PAGE_BYTES = 4096


def sha256(path):
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def rounded_frames(size):
    return (size + PAGE_BYTES - 1) // PAGE_BYTES


def elf_frames(path, *, dso=False):
    """Measure PT_LOAD, and conservatively account for a loader-held DSO file view."""
    loads = 0
    shared_file_pages = set()
    with path.open("rb") as stream:
        header = stream.read(64)
        if header[:6] != b"\x7fELF\x02\x01" or struct.unpack_from("<H", header, 18)[0] != 183:
            raise ValueError(f"{path}: expected little-endian AArch64 ELF64")
        stream.seek(struct.unpack_from("<Q", header, 32)[0])
        entry_size, count = struct.unpack_from("<HH", header, 54)
        if entry_size != 56:
            raise ValueError(f"{path}: unexpected program-header size {entry_size}")
        for _ in range(count):
            kind, flags, offset, address, _, filesz, memsz, _ = struct.unpack("<II6Q", stream.read(entry_size))
            if kind != 1 or not memsz:
                continue
            loads += rounded_frames(address % PAGE_BYTES + memsz)
            if dso and filesz and not flags & 2:
                # Writable file pages can become private through relocation or writes.
                # Also reserve a private copy for map_load's zeroed file tail.
                end = offset + filesz
                shared_end = end // PAGE_BYTES if filesz < memsz else rounded_frames(end)
                shared_file_pages.update(range(offset // PAGE_BYTES, shared_end))
    if not loads:
        raise ValueError(f"{path}: missing loadable segments")
    extra = rounded_frames(path.stat().st_size) - len(shared_file_pages) if dso else 0
    return loads, extra


class BootInteractiveQmpTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="vq-", dir="/tmp")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name).resolve()
        scripts = self.root / "scripts"
        scripts.mkdir()
        shutil.copyfile(ROOT / "scripts/boot-interactive.sh", scripts / "boot-interactive.sh")
        shutil.copyfile(ROOT / "scripts/qemu-smb-forward.sh", scripts / "qemu-smb-forward.sh")
        shutil.copyfile(ROOT / "scripts/suite-sequence.py", scripts / "suite-sequence.py")
        (scripts / "lib").mkdir()
        shutil.copy2(ROOT / "scripts/lib/root-fs.sh", scripts / "lib/root-fs.sh")
        shutil.copytree(ROOT / "docs", self.root / "docs")
        (scripts / "boot-commit.sh").write_text("boot_commit() { echo test; }\n")
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
        (scripts / "qemu-evidence.py").write_text(
            'import json, os, pathlib, signal, sys, time\n'
            'args = sys.argv[1:]\n'
            'if args[0] == "serve":\n'
            ' d = pathlib.Path(args[args.index("--dir") + 1]); d.mkdir(parents=True, exist_ok=True)\n'
            ' pathlib.Path(os.environ["RECEIVER_PID"]).write_text(str(os.getpid()))\n'
            ' (d / "route.json").write_text(json.dumps({"pid": os.getpid()}))\n'
            ' while True: time.sleep(1)\n'
            'else:\n'
            ' image, d = map(pathlib.Path, args[1:3]); route = json.loads((d / "route.json").read_text())\n'
            ' os.kill(route["pid"], 0); image.write_text("route=" + str(route["pid"]))\n')
        self.executable(scripts / "check-host-renderer.py", '#!/bin/sh\nexit 0\n')
        (scripts / "check-host-renderer.py").write_text('')
        (scripts / "qemu-arm64.py").write_text('print("default")\n')
        (scripts / "watch-qemu.py").write_text(
            'import json, os, pathlib, sys\n'
            'pathlib.Path(os.environ["CAPTURE"] + ".watch").write_text(json.dumps(sys.argv))\n'
            'disk = pathlib.Path(sys.argv[sys.argv.index("--disk") + 1])\n'
            'with disk.open("rb") as f: data = f.read(4096)\n'
            'pathlib.Path(os.environ["CAPTURE"] + ".disk").write_bytes(data)\n'
            'if data.startswith(b"route="): os.kill(int(data[6:]), 0)\n'
            'serial = pathlib.Path(sys.argv[sys.argv.index("--serial") + 1])\n'
            'suites = sys.argv[sys.argv.index("--suite") + 1]\n'
            'serial.write_text("".join("SUITE " + s + " DONE passed=1 failed=0 skipped=0 total=1\\n" for s in suites.split(",") if s))\n'
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
               mode=("--mode", "probe"), expected_code=0, build=False, timeout=10):
        """Boots with HOME and Vigil's home in different folders; vigil_home=None leaves VIGIL_TEST_HOME unset."""
        vigil_home = self.vigil_home if vigil_home == "default" else vigil_home
        boot_home = vigil_home if boot_home == "default" else boot_home
        env = {k: v for k, v in os.environ.items() if k not in ("VIGIL_TEST_HOME", "VIGIL_BOOT_ID")}
        env.update(HOME=str(self.home), STUB_VIGIL_ID=boot_id, STUB_BOOT_HOME=str(boot_home or ""),
                   CALLS=str(self.calls), CAPTURE=str(self.root / "args.json"),
                   RECEIVER_PID=str(self.root / "receiver.pid"), BREENIX_SUITE_ATTEMPT="1",
                   BREENIX_QEMU_GL_BINARY="qemu-system-aarch64")
        if vigil_home is not None:
            env["VIGIL_TEST_HOME"] = str(vigil_home)
        self.calls.unlink(missing_ok=True)
        result = subprocess.run(
            ["bash", str(self.root / "scripts/boot-interactive.sh"), *([] if build else ["--no-build"]),
             *mode, "--serial-log", str(self.root / "serial.txt"), *args],
            cwd=self.root, env=env, capture_output=True, text=True, timeout=timeout)
        self.assertEqual(result.returncode, expected_code, result.stdout + result.stderr)
        if expected_code:
            return [], result.stderr
        return json.loads((self.root / "args.json").read_text()), result.stdout

    def test_undeclared_suite_memory_keeps_512_mib(self):
        args, _ = self.launch("--no-display", mode=("--mode", "suite", "--suite", "smoke"))
        self.assertEqual([args[i + 1] for i, arg in enumerate(args) if arg == "-m"], ["512"])

    def test_cargo_selection_stages_hosted_runner_at_declared_memory(self):
        scripts = self.root / "scripts"
        shutil.copy2(ROOT / "scripts/write-boot-target.sh", scripts / "write-boot-target.sh")
        stage_call = self.root / "stage-call.json"
        (scripts / "rust_host_stage.py").write_text(
            "import json, sys\nfrom pathlib import Path\n"
            f"Path({str(stage_call)!r}).write_text(json.dumps(sys.argv[1:]))\n"
        )
        hosted = self.root / "userspace/programs/target/hosted/aarch64-breenix-hosted/release"
        hosted.mkdir(parents=True)
        binary = hosted / "suite-cargo"
        binary.write_bytes(b"hosted cargo suite")
        disk_root = self.root / "disk-root"
        (disk_root / "sbin").mkdir(parents=True)
        (disk_root / "etc").mkdir()
        shutil.copyfile(binary, disk_root / "sbin/suite-cargo")
        path = os.pathsep.join(("/opt/homebrew/opt/e2fsprogs/sbin", os.environ["PATH"]))
        with patch.dict(os.environ, PATH=path):
            subprocess.run(["mke2fs", "-q", "-t", "ext2", "-F", "-d", disk_root,
                            self.root / "target/ext2-aarch64.img", "8M"], check=True)
            args, _ = self.launch("--no-display", mode=("--mode", "suite", "--suite", "cargo"))
        manifest = json.loads((self.root / "docs/suites/cargo.json").read_text())
        minimum = manifest["minimumGuestMemory"]["mib"]
        self.assertEqual([args[i + 1] for i, arg in enumerate(args) if arg == "-m"], [str(minimum)])
        stage_args = json.loads(stage_call.read_text())
        self.assertEqual(stage_args[:3], ["--arch", "aarch64", "--image"])
        self.assertEqual(stage_args[4:], ["--suite", "cargo"])

    def test_rust_link_sequence_verifies_the_hosted_suite_binaries(self):
        scripts = self.root / "scripts"
        shutil.copy2(ROOT / "scripts/write-boot-target.sh", scripts / "write-boot-target.sh")
        (scripts / "rust_host_stage.py").write_text("")
        programs = self.root / "userspace/programs"
        hosted = programs / "target/hosted/aarch64-breenix-hosted/release"
        hosted.mkdir(parents=True)
        sbin = self.root / "disk-root/sbin"
        sbin.mkdir(parents=True)
        (sbin.parent / "etc").mkdir()
        for suite in ("rust-link", "llvm"):
            binary = f"suite-{suite}"
            (hosted / binary).write_bytes(f"{suite} suite binary".encode())
            shutil.copyfile(hosted / binary, sbin / binary)
        path = os.pathsep.join(("/opt/homebrew/opt/e2fsprogs/sbin", os.environ["PATH"]))
        with patch.dict(os.environ, PATH=path):
            subprocess.run(["mke2fs", "-q", "-t", "ext2", "-F", "-d", sbin.parent,
                            self.root / "target/ext2-aarch64.img", "8M"], check=True)
            args, _ = self.launch("--no-display", mode=("--mode", "suite", "--suite", "rust-link,llvm"))
        self.assertEqual([args[i + 1] for i, arg in enumerate(args) if arg == "-m"], ["512"])

    def test_suite_memory_uses_largest_declared_minimum(self):
        for suite, size in (("smoke", 1024), ("rust-std", 2048)):
            path = self.root / f"docs/suites/{suite}.json"
            manifest = json.loads(path.read_text())
            manifest["minimumGuestMemory"] = {"mib": size, "derivation": "Launcher test input."}
            path.write_text(json.dumps(manifest))
        args, _ = self.launch("--no-display", mode=("--mode", "suite", "--suite", "smoke,rust-std"))
        self.assertEqual([args[i + 1] for i, arg in enumerate(args) if arg == "-m"], ["2048"])

    @unittest.skipUnless(ARM_K5_TWIN, "set BREENIX_ARM_K5_TWIN to the built ARM K5 twin cache")
    def test_arm_rustc_dynamic_stages_recorded_files(self):
        scripts = self.root / "scripts"
        for name in ("write-boot-target.sh", "rust_host_stage.py", "lib/root-fs.sh",
                     "metal/metal_root.py", "metal/metal_probe.py", "metal/transfer_fixture.py"):
            destination = scripts / name
            destination.parent.mkdir(exist_ok=True)
            shutil.copy2(ROOT / "scripts" / name, destination)
        for name, script in (("cargo", "exit 0"), ("check-kernel-no-neon.sh", "exit 0"),
                             ("build-background.sh", 'exec "$@"')):
            self.executable(scripts / name, f"#!/bin/sh\n{script}\n")
        self.executable(scripts / "create_ext2_disk.sh", '''#!/bin/sh
set -eu
mkdir -p disk-root/etc
mke2fs -q -t ext2 -b 4096 -F -d disk-root target/ext2-aarch64.img 16M
''')
        programs = self.root / "userspace/programs"
        programs.mkdir(parents=True)
        (programs / "target").symlink_to(ARM_K5_TARGET, target_is_directory=True)
        self.executable(programs / "build.sh", '#!/bin/sh\nprintf "%s\\n" "$*" >> build-calls\n')
        records = self.root / "toolchain/rust-host"
        records.parent.mkdir()
        records.symlink_to(ARM_K5_RECORDS, target_is_directory=True)
        for name in ("aarch64-breenix.json", "aarch64-breenix-hosted.json", "rust-toolchain.toml"):
            shutil.copy2(ROOT / name, self.root / name)
        cache = Path(ARM_K5_TWIN).expanduser().resolve()
        kd = ARM_K5_TARGET / "kd/aarch64"
        suite = ARM_K5_TARGET / "hosted/aarch64-breenix-hosted/release/suite-rustc"
        k5 = json.loads((records / "k5/aarch64.json").read_text())
        pin = json.loads((records / "hosted-libc-aarch64.json").read_text())
        path = os.pathsep.join((str(scripts), "/opt/homebrew/opt/e2fsprogs/sbin", os.environ["PATH"]))
        rustup_home = os.environ.get("RUSTUP_HOME", str(Path.home() / ".rustup"))
        with patch.dict(os.environ, PATH=path, BREENIX_RUSTC_PORT_BUILDS=str(cache),
                        BREENIX_K5_SYSROOT=str(cache / "sysroot"), BREENIX_KD_BUILD_DIR=str(kd),
                        RUSTUP_HOME=rustup_home):
            args, _ = self.launch("--no-display", mode=("--mode", "suite", "--suite", "rustc,dynamic"),
                                  build=True, timeout=35)
            self.assertEqual([args[i + 1] for i, arg in enumerate(args) if arg == "-m"], ["1024"])
            calls = (self.root / "build-calls").read_text().splitlines()
            for command in ("--arch aarch64 --hosted-bin suite-rustc --features rustc",
                            "--arch aarch64 --rustc", "--arch aarch64 --dynamic"):
                self.assertIn(command, calls)
            host = "/usr/libexec/breenix/rustc"
            dynamic = "/usr/libexec/breenix/dynamic"
            entries = {
                f"{host}/bin/rustc": k5["hosted_rustc_sha256"],
                f"{host}/lib/{k5['hosted_link']['driver']}": k5["hosted_driver_sha256"],
                "/lib/libc.so": pin["sha256"]["libc.so"],
                "/lib/ld-breenix.so": sha256(kd / "ld-breenix.so"),
                "/sbin/suite-rustc": sha256(suite),
                "/sbin/suite-dynamic": sha256(kd / "suite-dynamic"),
            }
            self.assertEqual(entries["/lib/libc.so"], k5["hosted_link"]["libc_sha256"])
            for name, digest in k5["sysroot"]["files_sha256"].items():
                if name.startswith("lib/rustlib/aarch64-breenix/"):
                    entries[f"{host}/sysroot/{name}"] = digest
            for name in ("aarch64-breenix.json", "aarch64-breenix-hosted.json"):
                entries[f"{host}/targets/{name}"] = sha256(ROOT / name)
            for name in ("expect.json", "hello.rs", "type-error.rs", "uses-std.rs"):
                source = "expect-aarch64.json" if name == "expect.json" else name
                entries[f"{host}/corpus/{name}"] = sha256(records / "k5/corpus" / source)
            for name in ("dynamic-probe", "missing-interp", "libkd-probe.so",
                         "dynamic-panic-probe", "libkd-panic.so"):
                entries[f"{dynamic}/{name}"] = sha256(kd / name)
            image = self.root / "ext2-writable.img"
            for guest, digest in entries.items():
                with self.subTest(guest=guest), subprocess.Popen(
                        ["debugfs", "-R", f"cat {guest}", image], stdout=subprocess.PIPE,
                        stderr=subprocess.DEVNULL) as readback:
                    self.assertEqual(hashlib.file_digest(readback.stdout, "sha256").hexdigest(), digest)
                    self.assertEqual(readback.wait(), 0)

    @unittest.skipUnless(ARM_K5_TWIN, "set BREENIX_ARM_K5_TWIN to the built ARM K5 twin cache")
    def test_arm_rustc_and_dynamic_fit_declared_guest_memory(self):
        cache = Path(ARM_K5_TWIN).expanduser().resolve()
        compiler = cache / "build/aarch64-apple-darwin/stage1/bin/rustc"
        records = ARM_K5_RECORDS / "k5"
        k5 = json.loads((records / "aarch64.json").read_text())
        self.assertEqual(sha256(compiler), k5["mac_rustc_sha256"])
        corpus = records / "corpus"
        timed = ["/usr/sbin/taskpolicy", "-b", "/usr/bin/time", "-l", compiler]
        compile_args = [
            "--edition=2021",
            "--sysroot", cache / "sysroot", "-Dwarnings", "--emit=obj",
            "--target", ROOT / "aarch64-breenix.json",
        ]
        cases = {
            "compiler-version": (["-vV"], 0, 0),
            "identity-and-sysroot": (["uses-std.rs"], 0, 88_424_448),
            "hello-object-equals-mac": (["--crate-type=lib", "hello.rs"], 0, 61_685_760),
            "diagnostic-located": (["--error-format=json", "type-error.rs"], 1, 75_055_104),
        }
        rss = {}
        for case, (args, status, recorded_rss) in cases.items():
            out = self.root / f"{case}.o"
            invocation = args if case == "compiler-version" else [*compile_args, *args, "-o", out]
            result = subprocess.run(
                [*timed, *invocation],
                cwd=corpus, env={"RUST_TARGET_PATH": str(ROOT)}, capture_output=True,
                text=True, timeout=10,
            )
            self.assertEqual(result.returncode, status, result.stderr)
            measured = int(re.search(r"(?m)^\s*(\d+)\s+maximum resident set size", result.stderr)[1])
            rss[case] = rounded_frames(max(measured, recorded_rss))

        kd = ARM_K5_TARGET / "kd/aarch64"
        rustc = ARM_K5_TARGET / "rustc/aarch64"
        suite = ARM_K5_TARGET / "hosted/aarch64-breenix-hosted/release/suite-rustc"
        interpreter = kd / "ld-breenix.so"
        libc = kd / "libc.so"
        driver = rustc / k5["hosted_link"]["driver"]
        inputs = {
            case: ([suite, rustc / "rustc", interpreter], [driver, libc]) for case in cases
        }
        dynamic = kd / "suite-dynamic"
        probe = ([dynamic, kd / "dynamic-probe", interpreter], [kd / "libkd-probe.so", libc])
        inputs.update({
            "interp-runs-program": probe,
            "missing-interp-fails": ([dynamic, kd / "missing-interp"], []),
            "dso-relocs-and-tls": probe,
            "panic-across-dso": ([dynamic, kd / "dynamic-panic-probe", interpreter], [kd / "libkd-panic.so", libc]),
        })
        totals = {}
        for case, (programs, dsos) in inputs.items():
            elf = [elf_frames(path) for path in programs] + [elf_frames(path, dso=True) for path in dsos]
            loads, private_and_cache = map(sum, zip(*elf))
            totals[case] = loads + private_and_cache + rss.get(case, 0)
            print(f"{case}: PT_LOAD={loads} extra_DSO={private_and_cache} Mac_RSS={rss.get(case, 0)} "
                  f"total={totals[case]} frames")

        manifest = json.loads((ROOT / "docs/suites/rustc.json").read_text())
        minimum = manifest.get("minimumGuestMemory", {}).get("mib", 512)
        # Hold fixed the frames unavailable in main's 512 MiB boot 20261127T045753Z-qemu-76081.
        frames_per_mib = (1024 * 1024) // PAGE_BYTES
        floor = 88_532 + (minimum - 512) * frames_per_mib
        peak = max(totals.values())
        self.assertLessEqual(peak * 4, floor * 3, f"{peak} frames with a 25% reserve exceed {floor}")
        self.assertGreater(peak * 4, (floor - minimum // 2 * frames_per_mib) * 3)
        self.assertEqual(minimum & (minimum - 1), 0, "guest size must be a power of two")

    def test_cxx_runtime_build_keeps_manifest_and_verifies_scratch_image(self):
        scripts = self.root / "scripts"
        for name in ("userspace-manifest.py", "write-boot-target.sh", "rust_host_stage.py",
                     "lib/root-fs.sh", "metal/metal_root.py", "metal/metal_probe.py",
                     "metal/transfer_fixture.py"):
            destination = scripts / name
            destination.parent.mkdir(exist_ok=True)
            shutil.copy2(ROOT / "scripts" / name, destination)
        programs = self.root / "userspace/programs"
        (programs / "aarch64").mkdir(parents=True)
        (programs / "aarch64/suite-smoke.elf").write_bytes(b"smoke suite")
        (self.root / ".gitignore").write_text("/userspace/programs/target/\n")
        subprocess.run(["git", "init", "-q", str(self.root)], check=True)
        self.executable(programs / "build.sh", '''#!/bin/bash
set -eu
cd "$(dirname "$0")"
case "${3:-}" in
    '')
        rm -f aarch64/suite-cxx-runtime.elf
        python3 ../../scripts/userspace-manifest.py write --arch "$2" ;;
    --hosted-bin)
        out=target/hosted/aarch64-breenix-hosted/release
        mkdir -p "$out"
        printf '\\177ELFcxx suite' > "$out/suite-cxx-runtime" ;;
    --cxx-runtime)
        out=target/cxx-runtime/aarch64
        mkdir -p "$out"
        for name in static-init-order guarded-static containers-threads; do
            printf '\\177ELF%s' "$name" > "$out/$name"
        done ;;
    *) exit 2 ;;
esac
''')
        self.executable(scripts / "create_ext2_disk.sh", '''#!/bin/bash
set -eu
python3 scripts/userspace-manifest.py check "$@"
mkdir -p disk-root/etc disk-root/sbin
cp userspace/programs/aarch64/suite-smoke.elf disk-root/sbin/suite-smoke
mke2fs -q -t ext2 -F -d disk-root target/ext2-aarch64.img 8M
''')
        for name, script in (("cargo", "exit 0"), ("check-kernel-no-neon.sh", "exit 0"),
                             ("build-background.sh", 'exec "$@"')):
            self.executable(scripts / name, f"#!/bin/sh\n{script}\n")
        path = os.pathsep.join((str(scripts), "/opt/homebrew/opt/e2fsprogs/sbin", os.environ["PATH"]))
        with patch.dict(os.environ, PATH=path):
            for suites in ("cxx-runtime", "smoke,cxx-runtime", "cxx-runtime,smoke"):
                with self.subTest(suites=suites):
                    self.launch("--no-display", mode=("--mode", "suite", "--suite", suites), build=True)
                    for image, expected in (("target/ext2-aarch64.img", b""),
                                            ("ext2-writable.img", b"\x7fELFcxx suite")):
                        result = subprocess.check_output(
                            ["debugfs", "-R", "cat /sbin/suite-cxx-runtime", self.root / image],
                            stderr=subprocess.DEVNULL)
                        self.assertEqual(result, expected)

    def test_llvm_build_keeps_manifest_and_verifies_scratch_image(self):
        scripts = self.root / "scripts"
        for name in ("userspace-manifest.py", "write-boot-target.sh", "rust_host_stage.py",
                     "lib/root-fs.sh", "metal/metal_root.py", "metal/metal_probe.py",
                     "metal/transfer_fixture.py"):
            destination = scripts / name
            destination.parent.mkdir(exist_ok=True)
            shutil.copy2(ROOT / "scripts" / name, destination)
        corpus = self.root / "toolchain/rust-host/k4/corpus"
        shutil.copytree(ROOT / "toolchain/rust-host/k4/corpus", corpus)
        programs = self.root / "userspace/programs"
        (programs / "aarch64").mkdir(parents=True)
        (programs / "linker.ld").write_text("linker")
        for name in ("ipc", "rust-std"):
            (programs / f"aarch64/suite-{name}.elf").write_bytes(name.encode())
        (self.root / ".gitignore").write_text("/userspace/programs/target/\n")
        subprocess.run(["git", "init", "-q", str(self.root)], check=True)
        self.executable(programs / "build.sh", '''#!/bin/bash
set -eu
cd "$(dirname "$0")"
printf '%s\n' "$*" >> ../../build-calls
case "${3:-}" in
    '') python3 ../../scripts/userspace-manifest.py write --arch "$2" ;;
    --hosted-bin)
        out=target/hosted/aarch64-breenix-hosted/release
        mkdir -p "$out"
        printf '\177ELF%s suite' "$4" > "$out/$4" ;;
    --llvm)
        out=target/llvm/aarch64
        mkdir -p "$out"
        for name in llc llvm-mc bld; do printf '\\177ELF%s' "$name" > "$out/$name"; done
        printf archive > "$out/libc.a" ;;
    --cxx-runtime)
        out=target/cxx-runtime/aarch64
        mkdir -p "$out"
        for name in static-init-order guarded-static containers-threads; do
            printf '\\177ELF%s' "$name" > "$out/$name"
        done ;;
    *) exit 2 ;;
esac
''')
        self.executable(scripts / "create_ext2_disk.sh", '''#!/bin/bash
set -eu
python3 scripts/userspace-manifest.py check "$@"
mkdir -p disk-root/etc disk-root/sbin
for name in ipc rust-std; do cp "userspace/programs/aarch64/suite-$name.elf" "disk-root/sbin/suite-$name"; done
mke2fs -q -t ext2 -F -d disk-root target/ext2-aarch64.img 8M
''')
        for name, script in (("cargo", "exit 0"), ("check-kernel-no-neon.sh", "exit 0"),
                             ("build-background.sh", 'exec "$@"')):
            self.executable(scripts / name, f"#!/bin/sh\n{script}\n")
        path = os.pathsep.join((str(scripts), "/opt/homebrew/opt/e2fsprogs/sbin", os.environ["PATH"]))
        with patch.dict(os.environ, PATH=path):
            for suites in ("llvm", "ipc,llvm", "llvm,cxx-runtime,ipc,rust-std"):
                with self.subTest(suites=suites):
                    calls = self.root / "build-calls"
                    calls.unlink(missing_ok=True)
                    self.launch("--no-display", mode=("--mode", "suite", "--suite", suites), build=True)
                    lines = calls.read_text().splitlines()
                    hosted = lines.index("--arch aarch64 --hosted-bin suite-llvm --features llvm")
                    self.assertEqual(lines[hosted + 1], "--arch aarch64 --llvm")
                    for image, expected in (("target/ext2-aarch64.img", b""),
                                            ("ext2-writable.img", b"\x7fELFsuite-llvm suite")):
                        result = subprocess.check_output(
                            ["debugfs", "-R", "cat /sbin/suite-llvm", self.root / image],
                            stderr=subprocess.DEVNULL)
                        self.assertEqual(result, expected)

    def test_ui_testing_receiver_and_route_are_limited_to_standalone_suite(self):
        for suite, case in (("ui-testing", None), ("ui-testing", "proof"),
                            ("window-manager", None), ("gpu", None),
                            ("console,ui-testing", None)):
            with self.subTest(suite=suite, case=case):
                pidfile = self.root / "receiver.pid"
                pidfile.unlink(missing_ok=True)
                extra = ("--case", case) if case else ()
                self.launch(*extra, "--no-display", "--gpu-gl", mode=("--mode", "suite", "--suite", suite))
                disk = (self.root / "args.json.disk").read_text()
                if suite == "ui-testing":
                    self.assertTrue(pidfile.exists(), "receiver must start before QEMU")
                    pid = int(pidfile.read_text())
                    self.assertEqual(disk, f"route={pid}")
                    with self.assertRaises(ProcessLookupError):
                        os.kill(pid, 0)
                else:
                    self.assertFalse(pidfile.exists())
                    self.assertEqual(disk, "")

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

    def test_multi_suite_boot_uses_its_registered_folder(self):
        argv, output = self.launch(boot_id="boot-a", mode=("--mode", "suite", "--suite", "smoke,crypto"))
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
