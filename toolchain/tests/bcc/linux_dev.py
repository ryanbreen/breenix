#!/usr/bin/env python3
"""Run bcc development checks in the Linux build container.

Use --arch aarch64 in the linux/arm64 container (Linux/aarch64).
These checks are development only and are never Breenix evidence.

Pass the Cargo-built bld binary and liblinux_test.a. For the sealed-object
check pass the reference corpus directory (receipts and objs/*.o outputs).
For compiler checks also pass the Cargo-built bcc and bas binaries.
"""

import argparse
import hashlib
from importlib.machinery import SourceFileLoader
from importlib.util import module_from_spec, spec_from_loader
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
from tempfile import TemporaryDirectory

import records


ROOT = Path(__file__).resolve().parents[3]
CASES = ("byte-output", "literal-output")
# The development list adds aggregate-copy, bitfield-values and varargs in 25.7.
# Alignment, atomics, compound-literal, designated-init and generic wait for 25.9.
CORPUS = (
    "aggregate-copy",
    "array-index",
    "bitfield-values",
    "bitwise-shifts",
    "boolean-tokens",
    "byte-output",
    "conditional",
    "floating-conversion",
    "function-pointer",
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
# A4 adds the record cases before it adds variadic definitions.
AARCH64_CORPUS = tuple(name for name in CORPUS if name != "varargs")
ABI_CASES = ("aggregates-exhaustion", "sse-float")


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


def compile_object(args, source, work):
    assembly = work / source.with_suffix(".s").name
    obj = work / source.with_suffix(".o").name
    run_tool("bcc", [args.bcc, f"--target={args.arch}", "-S",
                     "-I", ROOT / "toolchain/include",
                     "-I", ROOT / "libs/libbreenix-libc/include",
                     "-o", assembly, source])
    assemble(args, assembly, obj)
    return obj


def assemble(args, assembly, obj):
    target = ["--target", "aarch64"] if args.arch == "aarch64" else []
    run_tool("bas", [args.bas, *target, "-o", obj, assembly])


def execute(args, executable, *objects):
    run_tool(
        "bld",
        [args.bld, "-T", ROOT / "userspace/programs/linker.ld", "--gc-sections",
         "-o", executable, *objects, args.runtime],
    )
    return subprocess.run([executable], capture_output=True, timeout=10)


def check_output(expect, actual, *, require_empty_stderr=False):
    status, _, stdout = expect.read_bytes().partition(b"\n")
    if ((actual.returncode, actual.stdout) != (int(status), stdout)
            or (require_empty_stderr and actual.stderr)):
        raise ValueError(
            f"expected status {int(status)} and stdout {stdout!r}, "
            f"got status {actual.returncode} and stdout {actual.stdout!r}; "
            f"stderr={actual.stderr!r}"
        )


def check(args, name, work):
    source = ROOT / "toolchain/corpus" / f"{name}.c"
    obj = compile_object(args, source, work) if args.corpus else reference_object(args, name)
    check_output(source.with_suffix(".expect"), execute(args, work / name, obj))


def aapcs64_abi_case(args, work):
    fixtures = ROOT / "toolchain/corpus/abi-aarch64"
    name = args.abi_case
    scripts = ROOT / "scripts"
    sys.path.insert(0, str(scripts))
    loader = SourceFileLoader("toolchain_ref_cc", str(scripts / "toolchain-ref-cc"))
    reference = module_from_spec(spec_from_loader(loader.name, loader))
    loader.exec_module(reference)
    clang, _ = reference.pinned_tool(Path(os.environ["BREENIX_TOOLCHAIN_CLANG"]),
                                     os.environ["BREENIX_TOOLCHAIN_CLANG_SHA256"], work)
    sealed = {}
    for half in ("caller", "callee"):
        directory = work / half
        directory.mkdir()
        source = fixtures / f"{name}-{half}.c"
        sealed[half], _, _ = reference.compile_c(args.arch, clang, source, (fixtures,), directory)

    passed = 0
    for bcc_half, clang_half in (("caller", "callee"), ("callee", "caller")):
        label = f"abi::{name} bcc-{bcc_half}/clang-{clang_half}"
        try:
            obj = compile_object(args, fixtures / f"{name}-{bcc_half}.c", work)
            actual = execute(args, work / f"bcc-{bcc_half}", obj, sealed[clang_half])
            check_output(fixtures / f"{name}.expect", actual, require_empty_stderr=True)
        except (OSError, ValueError, subprocess.SubprocessError) as error:
            print(f"{label} FAIL ({error})", file=sys.stderr)
        else:
            print(f"{label} PASS")
            passed += 1
    if passed != 2:
        raise ValueError(f"{passed}/2 mixed links matched {name}.expect")


def aapcs64_scalar_calls(args, work):
    fixtures = Path(__file__).with_name("aapcs64")
    probe = work / "probe.o"
    assemble(args, fixtures / "probe.s", probe)
    caller = compile_object(args, fixtures / "scalar.c", work)
    actual = execute(args, work / "scalar-calls", caller, probe)
    if (actual.returncode, actual.stdout, actual.stderr) != (42, b"", b""):
        raise ValueError(f"expected exit 42 with empty streams, got exit {actual.returncode}, "
                         f"stdout={actual.stdout!r}, stderr={actual.stderr!r}")


def aapcs64_records(args, work):
    for name, contents, status in (("records", records.SOURCE, 31),
                                   ("record-layout", records.AAPCS64_LAYOUT, 42)):
        source = work / f"{name}.c"
        source.write_text(contents)
        actual = execute(args, work / name, compile_object(args, source, work))
        if (actual.returncode, actual.stdout, actual.stderr) != (status, b"", b""):
            raise ValueError(f"{name}: expected exit {status} with empty streams, "
                             f"got exit {actual.returncode}, stdout={actual.stdout!r}, "
                             f"stderr={actual.stderr!r}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--corpus", action="store_true")
    mode.add_argument("--aapcs64-scalar-calls", action="store_true")
    mode.add_argument("--aapcs64-records", action="store_true")
    mode.add_argument("--abi-case", choices=ABI_CASES,
                      help="cross-call with BREENIX_TOOLCHAIN_CLANG and its SHA-256 pin")
    parser.add_argument("--arch", choices=("x86_64", "aarch64"), default="x86_64")
    for option in ("bld", "runtime", "reference-corpus", "bcc", "bas"):
        parser.add_argument(f"--{option}", required=option in ("bld", "runtime"),
                            type=lambda path: Path(path).resolve())
    args = parser.parse_args()
    if (platform.system(), platform.machine()) != ("Linux", args.arch):
        parser.error(f"run in a Linux/{args.arch} build container")
    aapcs64_check = (aapcs64_scalar_calls if args.aapcs64_scalar_calls else
                     aapcs64_records if args.aapcs64_records else
                     aapcs64_abi_case if args.abi_case else None)
    if aapcs64_check and args.arch != "aarch64":
        parser.error("AAPCS64 checks require --arch aarch64")
    if args.corpus or aapcs64_check:
        if args.bcc is None or args.bas is None:
            parser.error("compiler checks require --bcc and --bas")
    elif args.reference_corpus is None:
        parser.error("the sealed-object check requires --reference-corpus")

    if aapcs64_check:
        label = f"codegen::{aapcs64_check.__name__}"
        with TemporaryDirectory(prefix="bcc-linux-dev-") as work:
            try:
                aapcs64_check(args, Path(work))
            except (KeyError, OSError, ValueError, subprocess.SubprocessError) as error:
                print(f"{label} FAIL ({error})", file=sys.stderr)
                return 1
        print(f"{label} PASS")
        return 0

    cases = CASES
    if args.corpus:
        cases = AARCH64_CORPUS if args.arch == "aarch64" else CORPUS
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
