"""Exercise the POSIX BusyBox build driver with the pinned workload."""

import hashlib
import json
import os
import re
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


def run_driver(fixture, output, env, phase=None, arch="x86_64"):
    args = ["/bin/sh", str(DRIVER), str(fixture), str(output), arch]
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


def mac_fixture(directory, arch="x86_64"):
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
    libc_target = {"x86_64": "x86_64-breenix-c", "aarch64": "aarch64-breenix"}[arch]
    shutil.copy2(ROOT / f"libs/libbreenix-libc/target/{libc_target}/release/libc.a",
                 bundle / "lib/libc.a")
    return fixture, mac_environment(), expected


def mac_environment():
    return {"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "LC_ALL": "C",
            "HOSTCC": str(Path(os.environ["BREENIX_TOOLCHAIN_CLANG"]).with_name("clang")),
            "BUSYBOX_JOBS": "8"}


def pass_phase(fixture, output, env, phase=None, arch="x86_64"):
    result = run_driver(fixture, output, env, phase, arch)
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


def prepare_object_group_fixture(directory, arch):
    fixture, env, _ = mac_fixture(directory, arch)
    output = directory / "build"
    for phase in ("unpack", "patch", "generators", "configure"):
        pass_phase(fixture, output, env, phase, arch)
    assert not (output / "applets.goals").exists(), "configure already ran the inventory"
    header = output / "busybox-1.37.0/include/applet_tables.h"
    stat = header.stat()
    original = header.read_bytes()
    header.write_bytes(original + b"\n/* changed after configure */\n")
    altered = run_driver(fixture, output, env, "inventory", arch)
    assert altered.returncode == 1 and "predecessor configure" in altered.stderr, altered.stderr
    header.write_bytes(original)
    os.utime(header, ns=(stat.st_atime_ns, stat.st_mtime_ns))
    pass_phase(fixture, output, env, "inventory", arch)
    baseline = directory / "configured"
    shutil.copytree(output, baseline, symlinks=True)


def compare_object_group_chain(directory, arch):
    fixture = directory / "fixture"
    output = directory / "build"
    baseline = directory / "configured"
    env = mac_environment()
    source = output / "busybox-1.37.0"
    boundaries = {
        "x86_64": {"libbb": (32, 64, 96, 128), "applets": (32, 64, 79, 94)},
        "aarch64": {"libbb": (34, 70, 100, 128), "applets": (26, 55, 76, 94)},
    }[arch]
    for kind, ends in boundaries.items():
        goals = (output / f"{kind}.goals").read_text().splitlines()
        assert len(goals) == ends[-1]
        for index, expected_end in enumerate(ends, start=1):
            if kind == "applets" and index == 4:
                early_link = run_driver(fixture, output, env, "link-and-list", arch)
                assert early_link.returncode == 1 and "predecessor build-applets-4" in early_link.stderr, early_link.stderr
            phase = f"build-{kind}-{index}"
            pass_phase(fixture, output, env, phase, arch)
            produced = [goal for goal in goals if (source / goal).is_file()]
            assert produced == goals[:expected_end], (phase, len(produced), expected_end)

    def directory_outputs():
        goals = []
        for kind, ends in boundaries.items():
            kind_goals = (output / f"{kind}.goals").read_text().splitlines()
            assert len(kind_goals) == ends[-1]
            goals.extend(kind_goals)
        directories = {Path(goal).parent for goal in goals}
        paths = {source / goal for goal in goals}
        paths.update(source / subdir / "built-in.o" for subdir in directories)
        paths.update(path for subdir in directories if (path := source / subdir / "lib.a").is_file())
        return {str(path.relative_to(source)): hashlib.sha256(path.read_bytes()).hexdigest() for path in paths}

    expected = directory_outputs()
    # Replay from clean inputs at the same path so compile-time paths and
    # recorded build options stay fixed.
    output.rename(directory / "split")
    baseline.rename(output)
    pass_phase(fixture, output, env, "build-libbb", arch)
    pass_phase(fixture, output, env, "build-applets", arch)
    actual = directory_outputs()
    assert actual == expected, sorted(name for name, digest in expected.items() if actual.get(name) != digest)


def object_group_chain_equals_directory_goal(directory):
    prepare_object_group_fixture(directory, "x86_64")
    compare_object_group_chain(directory, "x86_64")


def audit_phases_equal_driver_phases(directory):
    arms = []
    body = DRIVER.read_text().split("run_phase() {", 1)[1].split("\n}\n", 1)[0]
    for line in body.splitlines():
        arm = re.match(r"\s+([a-z][a-z0-9-]*)\)(.*)", line)
        if arm:
            arms.append((arm.group(1), arm.group(2)))
    names = [name for name, _ in arms]
    chain = [
        name for name, rest in arms
        if ";" not in rest.strip().removesuffix(";;") and not any(re.fullmatch(re.escape(name) + r"-\d+", other) for other in names)
    ]
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    audit = runpy.run_path(str(Path(__file__).resolve().parent / "audit.py"), run_name="audit_phases")
    assert list(audit["PHASES"]) == chain, (list(audit["PHASES"]), chain)


if __name__ == "__main__":
    BMAKE = Path(sys.argv[2]).resolve()
    if sys.argv[1] in {"prepare_object_group_fixture", "compare_object_group_chain"}:
        # Keep one absolute build path across the split and unsplit compiles.
        globals()[sys.argv[1]](Path(sys.argv[4]).resolve(strict=True), sys.argv[3])
    else:
        with tempfile.TemporaryDirectory(prefix="breenix-busybox-check-") as temporary:
            globals()[sys.argv[1]](Path(temporary))
