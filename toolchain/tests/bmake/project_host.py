#!/usr/bin/env python3
"""Check the project recipes and ELF using host Cargo builds of the E25 tools."""

import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys
from tempfile import TemporaryDirectory


ROOT = Path(__file__).resolve().parents[3]
FIXTURE = ROOT / "toolchain/corpus/project"
INPUTS = ("Makefile", "main.c", "util.c", "util.h", "value.s", "project.expect")
OUTPUTS = ("main.o", "util.o", "value.o", "project")
COMPILE = (b"cc -c -o main.o main.c", b"cc -c -o util.o util.c")
ASSEMBLE = b"as -o value.o value.s"
LINK = b"cc -o project main.o util.o value.o"


def run(command, cwd, env):
    result = subprocess.run(command, cwd=cwd, env=env, capture_output=True, timeout=10)
    if result.returncode:
        raise ValueError(
            f"{command[0]} exited {result.returncode}: "
            f"{result.stdout!r} {result.stderr!r}"
        )
    return result.stdout


def check_recipes(case, stdout, expected):
    actual = stdout.splitlines()
    if actual != list(expected):
        raise ValueError(f"{case}: expected recipes {expected!r}, got {actual!r}")


def snapshot(project, name):
    path = project / name
    return path.read_bytes(), path.stat().st_mtime_ns


def check(bin_dir, libc, root):
    project = root / "src"
    project.mkdir()
    for name in INPUTS:
        shutil.copyfile(FIXTURE / name, project / name)

    bundle = root / "dev"
    bin_path = bundle / "bin"
    bin_path.mkdir(parents=True)
    for package, name in (("bmake", "make"), ("bcc", "cc"), ("bas", "as"), ("bld", "ld")):
        shutil.copy2(bin_dir / package, bin_path / name)
    shutil.copytree(ROOT / "libs/libbreenix-libc/include", bundle / "include")
    shutil.copytree(ROOT / "toolchain/include", bundle / "include-support")
    shutil.copyfile(ROOT / "userspace/programs/linker.ld", bundle / "linker.ld")
    (bundle / "lib").mkdir()
    shutil.copyfile(libc, bundle / "lib/libc.a")

    env = {"PATH": f"{bin_path}:/bin"}
    make = [bin_path / "make"]
    check_recipes("clean build", run(make, project, env), (*COMPILE, ASSEMBLE, LINK))
    before = {name: snapshot(project, name) for name in OUTPUTS}
    run([bin_dir / "elfcheck", "--exec", "project"], project, env)
    print("clean build: three objects and an elfcheck-clean static executable")

    check_recipes("no-op rebuild", run(make, project, env), ())
    for name in OUTPUTS:
        if snapshot(project, name) != before[name]:
            raise ValueError(f"no-op rebuild changed bytes or mtime of {name}")
    print("no-op rebuild: no recipes and unchanged output bytes and mtimes")

    # Pin the baseline before changing just the header, without sleeping.
    baseline = 946684800  # 2000-01-01 UTC
    for name in (*INPUTS, *OUTPUTS):
        os.utime(project / name, (baseline, baseline))
    before = {name: snapshot(project, name) for name in OUTPUTS}
    newer = baseline + 86400
    os.utime(project / "util.h", (newer, newer))
    if (project / "util.h").stat().st_mtime_ns <= max(mtime for _, mtime in before.values()):
        raise ValueError("util.h is not newer than the build outputs")

    check_recipes("header rebuild", run(make, project, env), (*COMPILE, LINK))
    for name in ("main.o", "util.o", "project"):
        if (project / name).stat().st_mtime_ns <= before[name][1]:
            raise ValueError(f"header rebuild did not update {name}")
    if snapshot(project, "value.o") != before["value.o"]:
        raise ValueError("header rebuild changed bytes or mtime of value.o")
    print("header rebuild: main.o, util.o and project only")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", type=Path, required=True, help="host Cargo output directory")
    parser.add_argument("--libc", type=Path, required=True, help="Breenix x86-64 C ABI libc.a")
    args = parser.parse_args()
    try:
        with TemporaryDirectory(prefix="bmake-project-") as scratch:
            check(args.bin_dir.resolve(), args.libc.resolve(), Path(scratch).resolve())
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        print(f"project_host: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
