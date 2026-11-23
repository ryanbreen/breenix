#!/usr/bin/env python3
"""Run bcc development checks in the x86-64 Linux build container.

Pass the Cargo-built bld binary and liblinux_test.a. For the sealed-object
check pass the reference corpus directory (receipts and objs/*.o outputs).
For --corpus also pass the Cargo-built bcc and bas binaries.
"""

import argparse
import hashlib
import json
from pathlib import Path
import platform
import subprocess
import sys
from tempfile import TemporaryDirectory


ROOT = Path(__file__).resolve().parents[3]
CASES = ("byte-output", "literal-output")
# The development list adds aggregate-copy, bitfield-values and varargs in 25.7.
CORPUS = (
    "aggregate-copy",
    "alignment",
    "array-index",
    "atomics",
    "bitfield-values",
    "bitwise-shifts",
    "boolean-tokens",
    "byte-output",
    "compound-literal",
    "conditional",
    "designated-init",
    "floating-conversion",
    "function-pointer",
    "generic",
    "goto",
    "integer-arithmetic",
    "integer-promotions",
    "literal-output",
    "loops",
    "macros",
    "pointer-distance",
    "recursion",
    "return-status",
    "scope-shadow",
    "short-circuit",
    "static-duration",
    "switch-enum",
    "unsigned-wrap",
    "usual-conversions",
    "varargs",
)


def reference_object(args, name):
    obj = args.reference_corpus / "objs" / f"{name}.o"
    receipt = json.loads((args.reference_corpus / f"{name}.inventory.json").read_text())
    if hashlib.sha256(obj.read_bytes()).hexdigest() != receipt["object"]["sha256"]:
        raise ValueError(f"{obj.name} differs from the sealed reference object")
    return obj


def run_tool(name, command):
    result = subprocess.run(command, capture_output=True, timeout=10)
    if result.returncode:
        stderr = result.stderr.decode(errors="replace").strip()
        raise ValueError(f"{name} exited {result.returncode}: {stderr}")


def compile_object(args, name, work):
    source = ROOT / "toolchain/corpus" / f"{name}.c"
    assembly = work / f"{name}.s"
    obj = work / f"{name}.o"
    run_tool("bcc", [args.bcc, "--target=x86_64", "-S",
                     "-I", ROOT / "toolchain/include",
                     "-I", ROOT / "libs/libbreenix-libc/include",
                     "-o", assembly, source])
    run_tool("bas", [args.bas, "-o", obj, assembly])
    return obj


def check(args, name, work):
    obj = compile_object(args, name, work) if args.corpus else reference_object(args, name)
    executable = work / name
    run_tool(
        "bld",
        [args.bld, "-T", ROOT / "userspace/programs/linker.ld", "--gc-sections",
         "-o", executable, obj, args.runtime],
    )
    actual = subprocess.run([executable], capture_output=True, timeout=10)
    status, _, stdout = (ROOT / "toolchain/corpus" / f"{name}.expect").read_bytes().partition(b"\n")
    if (actual.returncode, actual.stdout) != (int(status), stdout):
        raise ValueError(
            f"expected status {int(status)} and stdout {stdout!r}, "
            f"got status {actual.returncode} and stdout {actual.stdout!r}; "
            f"stderr={actual.stderr!r}"
        )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus", action="store_true")
    for option in ("bld", "runtime", "reference-corpus", "bcc", "bas"):
        parser.add_argument(f"--{option}", required=option in ("bld", "runtime"),
                            type=lambda path: Path(path).resolve())
    args = parser.parse_args()
    if (platform.system(), platform.machine()) != ("Linux", "x86_64"):
        parser.error("run in the x86-64 Linux build container")
    if args.corpus and (args.bcc is None or args.bas is None):
        parser.error("--corpus requires --bcc and --bas")
    if not args.corpus and args.reference_corpus is None:
        parser.error("the sealed-object check requires --reference-corpus")

    cases = CORPUS if args.corpus else CASES
    passed = 0
    with TemporaryDirectory(prefix="bcc-linux-dev-") as work:
        for name in cases:
            try:
                check(args, name, Path(work))
            except (OSError, ValueError, KeyError, subprocess.TimeoutExpired) as error:
                print(f"linux_dev::{name} FAIL ({error})", file=sys.stderr)
            else:
                print(f"linux_dev::{name} PASS")
                passed += 1
    print(f"linux_dev: {passed}/{len(cases)} passed")
    return int(passed != len(cases))


if __name__ == "__main__":
    sys.exit(main())
