"""Exercise the POSIX BusyBox build driver with the pinned workload."""

import json
import os
from pathlib import Path
import runpy
import shutil
import subprocess
import sys
import tempfile

BMAKE = Path(sys.argv[2]).resolve()
ROOT = Path(__file__).resolve().parents[3]
DRIVER = ROOT / "toolchain/corpus/busybox.sh"
VENDOR = "vendor/busybox"
ARCHIVE = "busybox-1.37.0.tar.bz2"
REFERENCE = ROOT / "target/libc-busybox/x86_64/applets.json"


def run_driver(fixture, output, env):
    return subprocess.run(
        ["/bin/sh", str(DRIVER), str(fixture), str(output), "x86_64"],
        env=env, text=True, capture_output=True, timeout=60,
    )


def refuses_digest_mismatch(directory):
    fixture = directory / "fixture"
    vendor = fixture / VENDOR
    shutil.copytree(ROOT / VENDOR, vendor)
    for name in (ARCHIVE, "patches/series", "breenix-libc.config"):
        path = vendor / name
        original = path.read_bytes()
        path.write_bytes(original + b"tampered\n")
        output = directory / "build"
        result = run_driver(fixture, output, {"PATH": os.defpath + ":/sbin", "LC_ALL": "C"})
        assert result.returncode == 1, result.stderr
        assert f"{name}: SHA-256 mismatch" in result.stderr, result.stderr
        assert not output.exists(), "digest failure created a build tree"
        path.write_bytes(original)


def mac_hosted_build_lists_same_applets(directory):
    reference = runpy.run_path(str(ROOT / "scripts/toolchain-busybox-c-inventory.py"))
    clang, _ = reference["verify_inputs"]()
    expected = json.loads(REFERENCE.read_text())
    assert expected["compile_ok"] and expected["core"]["links"] and expected["applets"]
    fixture = directory / "fixture"
    shutil.copytree(ROOT / VENDOR, fixture / VENDOR)
    bundle = fixture / "bundle"
    tools = bundle / "bin"
    tools.mkdir(parents=True)
    for name, package in {"cc": "bcc", "as": "bas", "ld": "bld", "ar": "bar", "nm": "bnm"}.items():
        shutil.copy2(BMAKE.with_name(package), tools / name)
    shutil.copy2(BMAKE, tools / "make")
    shutil.copytree(ROOT / "toolchain/include", bundle / "include-support")
    shutil.copytree(ROOT / "libs/libbreenix-libc/include", bundle / "include")
    shutil.copy2(ROOT / "userspace/programs/linker.ld", bundle / "linker.ld")
    (bundle / "lib").mkdir()
    shutil.copy2(
        ROOT / "libs/libbreenix-libc/target/x86_64-breenix-c/release/libc.a",
        bundle / "lib/libc.a",
    )
    output = directory / "build"
    result = run_driver(fixture, output, {
        "PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "LC_ALL": "C", "HOSTCC": str(clang),
    })
    print(result.stdout, end="")
    assert result.returncode == 0, result.stderr
    report = json.loads((output / "applets.json").read_text())
    assert report == expected, (
        f"BusyBox applet report differs from {REFERENCE}: "
        f"missing={sorted(expected['applets'].keys() - report['applets'].keys())}, "
        f"extra={sorted(report['applets'].keys() - expected['applets'].keys())}"
    )
    assert (output / "busybox.elf").is_file(), "completed build has no BusyBox ELF"


with tempfile.TemporaryDirectory(prefix="breenix-busybox-check-") as temporary:
    globals()[sys.argv[1]](Path(temporary))
