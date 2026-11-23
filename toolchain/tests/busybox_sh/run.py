"""Exercise the POSIX BusyBox build driver with the pinned workload."""

import hashlib
import json
import os
from pathlib import Path
import runpy
import shutil
import signal
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
    args = ["/bin/sh", str(DRIVER), str(fixture), str(output), "x86_64"]
    with subprocess.Popen(
        args, env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        start_new_session=True,
    ) as child:
        try:
            stdout, stderr = child.communicate(timeout=57)
        except subprocess.TimeoutExpired as timeout:
            os.killpg(child.pid, signal.SIGTERM)
            try:
                stdout, stderr = child.communicate(timeout=1)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                stdout, stderr = child.communicate()
            print(stdout, end="")
            for name in ("config.log", "compile.log", "link.log"):
                log = output / name
                if log.is_file():
                    print(f"{name}:\n" + "\n".join(log.read_text().splitlines()[-12:]),
                          file=sys.stderr)
            raise TimeoutError(f"BusyBox driver exceeded its deadline\n{stderr}") from timeout
        return subprocess.CompletedProcess(args, child.returncode, stdout, stderr)


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
    pin = os.environ["BREENIX_TOOLCHAIN_CLANG_SHA256"]
    compiler = Path(os.environ["BREENIX_TOOLCHAIN_CLANG"])
    assert compiler.is_absolute() and hashlib.sha256(compiler.read_bytes()).hexdigest() == pin
    os.environ["BREENIX_CC_CLANG"] = str(compiler)
    reference = runpy.run_path(str(ROOT / "scripts/toolchain-busybox-c-inventory.py"))
    clang, _ = reference["verify_inputs"]()
    assert clang.samefile(compiler), "the reference and Mac build selected different Clangs"
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
        "BUSYBOX_JOBS": "8",
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
    print(f"Matched {len(report['applets'])} reference applets")


with tempfile.TemporaryDirectory(prefix="breenix-busybox-check-") as temporary:
    globals()[sys.argv[1]](Path(temporary))
