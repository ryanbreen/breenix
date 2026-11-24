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

ROOT = Path(__file__).resolve().parents[3]
DRIVER = ROOT / "toolchain/corpus/busybox.sh"
VENDOR = "vendor/busybox"
ARCHIVE = "busybox-1.37.0.tar.bz2"
REFERENCE = ROOT / "target/libc-busybox-reference/x86_64/applets.json"


def run_driver(fixture, output, env, phase=None):
    args = ["/bin/sh", str(DRIVER), str(fixture), str(output), "x86_64"]
    if phase is not None:
        args.insert(2, phase)
    return run_phase_command(args, output, env)


def run_phase_command(args, output, env):
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


def refuses_failed_patch(directory):
    fixture = directory / "fixture"
    shutil.copytree(ROOT / VENDOR, fixture / VENDOR)
    tools = fixture / "bundle/bin"
    tools.mkdir(parents=True)
    for tool in ("cc", "as", "ld", "ar", "nm", "make"):
        (tools / tool).symlink_to("/bin/sh")
    image_bin = fixture / "bin"
    image_bin.mkdir()
    patch = image_bin / "patch"
    patch.write_text(
        "#!/bin/sh\n"
        "if [ ! -e ../first-patch-failed ]; then\n"
        "    touch ../first-patch-failed\n"
        "    echo 'injected first patch failure' >&2\n"
        "    exit 1\n"
        "fi\n"
        "exec /usr/bin/patch \"$@\"\n"
    )
    patch.chmod(0o755)
    output = directory / "build"
    env = {"PATH": os.defpath, "LC_ALL": "C"}
    pass_phase(fixture, output, env, "unpack")
    result = run_driver(fixture, output, env, "patch")
    assert (output / "first-patch-failed").is_file()
    assert result.returncode == 1, result
    assert "injected first patch failure" in result.stderr, result.stderr
    assert not (output / "patch.sha256").exists(), "failed patch published a receipt"


def mac_fixture(directory):
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
    env = {"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "LC_ALL": "C",
           "HOSTCC": str(clang), "BUSYBOX_JOBS": "8"}
    return fixture, env, expected


def pass_phase(fixture, output, env, phase=None):
    result = run_driver(fixture, output, env, phase)
    print(result.stdout, end="")
    assert result.returncode == 0, result.stderr


def mac_hosted_build_lists_same_applets(directory):
    fixture, env, expected = mac_fixture(directory)
    output = directory / "build"
    pass_phase(fixture, output, env)
    report = json.loads((output / "applets.json").read_text())
    assert report == expected, (
        f"BusyBox applet report differs from {REFERENCE}: "
        f"missing={sorted(expected['applets'].keys() - report['applets'].keys())}, "
        f"extra={sorted(report['applets'].keys() - expected['applets'].keys())}"
    )
    assert (output / "busybox.elf").is_file(), "completed build has no BusyBox ELF"
    print(f"Matched {len(report['applets'])} reference applets")


def phases_chain_from_predecessor_hashes(directory):
    fixture, env, expected = mac_fixture(directory)
    output = directory / "build"
    for phase in (
        "unpack", "patch", "generators", "configure", "inventory",
        "build-libbb-1", "build-libbb-2", "build-libbb-3", "build-libbb-4",
        "build-applets-1", "build-applets-2", "build-applets-3", "build-applets-4", "link-and-list",
    ):
        pass_phase(fixture, output, env, phase)
    assert json.loads((output / "applets.json").read_text()) == expected
    assert (output / "busybox.elf").stat().st_size > 0


def phase_refuses_missing_or_altered_predecessor(directory):
    fixture = directory / "fixture"
    shutil.copytree(ROOT / VENDOR, fixture / VENDOR)
    tools = fixture / "bundle/bin"
    tools.mkdir(parents=True)
    for tool in ("cc", "as", "ld", "ar", "nm", "make"):
        (tools / tool).symlink_to("/bin/sh")
    env = {"PATH": os.defpath, "LC_ALL": "C"}
    output = directory / "build"
    missing = run_driver(fixture, output, env, "patch")
    assert missing.returncode == 1 and "predecessor unpack" in missing.stderr, missing.stderr
    pass_phase(fixture, output, env, "unpack")
    source = output / "busybox-1.37.0/Makefile"
    with source.open("ab") as handle:
        handle.write(b"# tampered\n")
    altered = run_driver(fixture, output, env, "patch")
    assert altered.returncode == 1 and "predecessor unpack" in altered.stderr, altered.stderr
    assert not (output / "patch.log").exists()


def object_group_chain_equals_directory_goal(directory):
    fixture, env, _ = mac_fixture(directory)
    output = directory / "build"
    for phase in ("unpack", "patch", "generators", "configure"):
        pass_phase(fixture, output, env, phase)
    assert not (output / "applets.goals").exists(), "configure already ran the inventory"
    header = output / "busybox-1.37.0/include/applet_tables.h"
    stat = header.stat()
    original = header.read_bytes()
    header.write_bytes(original + b"\n/* changed after configure */\n")
    altered = run_driver(fixture, output, env, "inventory")
    assert altered.returncode == 1 and "predecessor configure" in altered.stderr, altered.stderr
    header.write_bytes(original)
    os.utime(header, ns=(stat.st_atime_ns, stat.st_mtime_ns))
    pass_phase(fixture, output, env, "inventory")
    baseline = directory / "configured"
    shutil.copytree(output, baseline, symlinks=True)
    for phase in ("build-libbb-1", "build-libbb-2", "build-libbb-3", "build-libbb-4"):
        pass_phase(fixture, output, env, phase)
    for phase in ("build-applets-1", "build-applets-2", "build-applets-3"):
        pass_phase(fixture, output, env, phase)
    source = output / "busybox-1.37.0"
    goals = (output / "applets.goals").read_text().splitlines()
    assert len(goals) == 94
    assert all((source / goal).is_file() for goal in goals[:79])
    assert not any((source / goal).exists() for goal in goals[79:])
    early_link = run_driver(fixture, output, env, "link-and-list")
    assert early_link.returncode == 1 and "predecessor build-applets-4" in early_link.stderr, early_link.stderr
    pass_phase(fixture, output, env, "build-applets-4")

    def directory_outputs():
        libbb = (output / "libbb.goals").read_text().splitlines()
        assert len(libbb) == 128
        directories = {Path(goal).parent for goal in libbb + goals}
        paths = {source / goal for goal in libbb + goals}
        paths.update(source / subdir / "built-in.o" for subdir in directories)
        paths.update(path for subdir in directories if (path := source / subdir / "lib.a").is_file())
        return {str(path.relative_to(source)): hashlib.sha256(path.read_bytes()).hexdigest() for path in paths}

    expected = directory_outputs()
    # Replay from clean inputs at the same path so compile-time paths and
    # recorded build options stay fixed.
    output.rename(directory / "split")
    baseline.rename(output)
    pass_phase(fixture, output, env, "build-libbb")
    pass_phase(fixture, output, env, "build-applets")
    actual = directory_outputs()
    assert actual == expected, sorted(name for name, digest in expected.items() if actual.get(name) != digest)


if __name__ == "__main__":
    BMAKE = Path(sys.argv[2]).resolve()
    with tempfile.TemporaryDirectory(prefix="breenix-busybox-check-") as temporary:
        globals()[sys.argv[1]](Path(temporary))
