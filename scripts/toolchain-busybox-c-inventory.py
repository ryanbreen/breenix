#!/usr/bin/env python3
"""Replay the pinned Mac BusyBox build's C compile records through host bcc."""

import argparse
import hashlib
from itertools import islice
import json
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile
from typing import NamedTuple

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "target/libc-busybox-reference/x86_64"
SRC = WORK / "busybox-1.37.0"
VENDOR = ROOT / "vendor/busybox"
BCC = ROOT / "toolchain/target/debug/bcc"
OUTPUT = ROOT / "toolchain/docs/busybox-c-inventory.json"
ARCHIVE_SHA256 = "3311dff32e746499f4df0d5df04d7eb396382d7e108bb9250e7b519b837043a4"

LOCATED = re.compile(
    r"(?P<file>.+?):(?P<line>[1-9]\d*):(?P<column>[1-9]\d*): (?:error|warning): (?P<message>.+)"
)
COMMAND = re.compile(r"cmd_(\S+) := (.+)")
CONFIG = re.compile(r"(CONFIG_\w+)=(.*)")
DISABLED = re.compile(r"# (CONFIG_\w+) is not set")
TYPEOF = rb"(?:__typeof__|__typeof|typeof)\s*\("
NAMED_VARARGS = rb"\s*#\s*define\s+\w+\([^)]*\b[A-Za-z_]\w*\s*$"

# Match compiler messages separately from the location. Some bcc diagnostics
# name only the expected syntax, so those rows also require a token at the
# reported byte column. The patterns describe GNU forms from the spec ledger.
CLASSES = {
    "statement expression": (
        r"^(?:unsupported )?statement expression\b", "25.11b statement expressions",
    ),
    "typeof": (
        r"^(?:unexpected token|unsupported) ['\"]?(?:__typeof__|__typeof|typeof)\b",
        "25.11b typeof",
    ),
    "attribute": (
        r"^(?:unsupported attribute\b|attribute (?:does not|requires)\b|"
        r"expected (?:an attribute name|.* attribute(?: list)?\b)|"
        r"unexpected token ['\"]__attribute__['\"])",
        "25.11b attributes",
    ),
    "builtin": (
        r"^(?:unknown builtin|undeclared identifier) ['\"]__builtin_\w+['\"]",
        "25.11b builtins",
    ),
    "GNU preprocessor": (
        r"^(?:unsupported directive ['\"]#include_next['\"]|unsupported GNU preprocessor\b)",
        "25.11b GNU preprocessing",
    ),
    "inline asm": (
        r"^(?:unexpected token|undeclared identifier|unsupported) ['\"]?(?:__asm__|__asm|asm)\b",
        "25.11b inline assembly",
    ),
    "unknown option": (r"^unknown option\b", "25.11b Kbuild driver options"),
    "other": (None, "unassigned"),
}


class Compile(NamedTuple):
    kind: str
    output: str
    source: str
    flags: list[str]


def source_at(diagnostic, source_root):
    """Read the line named by bcc without consulting headers outside the workload."""
    file = diagnostic["file"]
    if file.startswith("<"):
        return b""
    path = Path(file)
    if source_root is not None:
        path = (source_root / path).resolve()
        if not path.is_relative_to(source_root.resolve()):
            return b""
    try:
        with path.open("rb") as source:
            return next(islice(source, int(diagnostic["line"]) - 1, None), b"")
    except (FileNotFoundError, IsADirectoryError):
        # A #line directive can supply a logical file name.
        return b""


def classify_diagnostic(diagnostic, source_root=None):
    located = LOCATED.fullmatch(diagnostic)
    if located is None:
        raise ValueError(f"unlocated bcc diagnostic: {diagnostic}")
    message = located["message"]
    for name, (pattern, _) in CLASSES.items():
        if pattern and re.search(pattern, message):
            return name

    if message not in {
        "expected a valid type specifier",
        "expected an identifier, integer or string expression",
        "expected ')' after macro parameters",
    }:
        return "other"

    line = source_at(located, source_root)
    column = int(located["column"]) - 1
    before, at = line[:column], line[column:]
    if message == "expected a valid type specifier" and re.match(TYPEOF, at):
        return "typeof"
    if (message == "expected an identifier, integer or string expression"
            and at.startswith(b"{") and before.rstrip().endswith(b"(")):
        return "statement expression"
    if (message == "expected ')' after macro parameters"
            and at.startswith(b"...") and re.fullmatch(NAMED_VARARGS, before)):
        return "GNU preprocessor"
    return "other"


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def checksum_lines(paths):
    return "".join(f"{digest(path)}  {path}\n" for path in paths)


def verify_inputs():
    archive = VENDOR / f"{SRC.name}.tar.bz2"
    if digest(archive) != ARCHIVE_SHA256:
        raise ValueError("BusyBox archive SHA-256 mismatch")
    series = VENDOR / "patches/series"
    names = [line for line in series.read_text().splitlines() if line and not line.startswith("#")]
    if len(names) != len(set(names)) or any(
        not re.fullmatch(r"[A-Za-z0-9_.-]+", name) or ".." in name for name in names
    ):
        raise ValueError("invalid BusyBox patch series")

    # Recompute the build's two receipts as written by build-libc-busybox.sh.
    # The build fingerprint binds the command records to the checked-in inputs.
    patches = checksum_lines(sorted(p for p in series.parent.rglob("*") if p.is_file()))
    if hashlib.sha256(patches.encode()).hexdigest() != (SRC / ".extracted").read_text().strip():
        raise ValueError("existing BusyBox tree has a different patch digest")
    build = ROOT / "scripts/build-libc-busybox.sh"
    cc = ROOT / "scripts/breenix-cc"
    tools = subprocess.check_output([str(cc), "--arch", "x86_64", "--tool-dir"], text=True).strip()
    clang = Path(tools) / "clang"
    libc = ROOT / "libs/libbreenix-libc/target/x86_64-breenix-c/release/libc.a"
    config = VENDOR / "breenix-libc.config"
    files = [
        libc, archive, config, cc, build, VENDOR / "required-applets",
        ROOT / "scripts/install-libc-busybox.sh",
    ]
    headers = sorted((ROOT / "libs/libbreenix-libc/include").rglob("*.h"))
    fingerprint = checksum_lines(files) + patches + checksum_lines(headers)
    fingerprint += subprocess.check_output([str(clang), "--version"], text=True)
    if hashlib.sha256(fingerprint.encode()).hexdigest() != (WORK / "inputs.sha256").read_text().strip():
        raise ValueError("existing BusyBox build fingerprint differs from current inputs")

    actual = {}
    for line in (SRC / ".config").read_text().splitlines():
        if setting := CONFIG.fullmatch(line):
            actual[setting[1]] = setting[2]
        elif setting := DISABLED.fullmatch(line):
            actual[setting[1]] = "n"
    for line in config.read_text().splitlines():
        setting = CONFIG.fullmatch(line.strip())
        if setting and actual.get(setting[1], "n") != setting[2]:
            raise ValueError(f"existing BusyBox configuration differs at {setting[1]}")
    if not json.loads((WORK / "applets.json").read_text())["compile_ok"]:
        raise ValueError("the existing BusyBox build did not finish compiling")

    return clang, {
        "archive": ARCHIVE_SHA256,
        "series": digest(series),
        "config": digest(config),
        "patches": {name: digest(series.parent / name) for name in names},
    }


def flag_names(args):
    operands = iter(args)
    for arg in operands:
        if arg in ("-I", "-D", "-include", "-o", "-MF"):
            next(operands)
            yield arg
        elif arg.startswith("-Wp,-MD,"):
            yield "-Wp,-MD,FILE"
        elif arg.startswith(("-I", "-D")):
            yield arg[:2]
        elif arg.startswith("-"):
            yield arg


def compile_commands(clang):
    target_cc = WORK / "bin/breenix-x86_64-clang"
    compilers = {str(target_cc): "target", str(clang): "host"}
    objects = {Path(path) for path in (WORK / "objects.txt").read_text().splitlines()}
    seen = set()
    covered = set()
    commands = []
    for record in sorted(SRC.rglob(".*.cmd")):
        for line in record.read_text().splitlines():
            match = COMMAND.fullmatch(line)
            if match is None:
                continue
            output, command = match.groups()
            args = shlex.split(command)
            if not args or args[0] not in compilers:
                continue
            kind = compilers[args[0]]
            # Kbuild also saves archive/relocatable link recipes. A host
            # generator can compile and link its C input in a single command.
            sources = [arg for arg in args[1:] if arg.endswith((".c", ".S", ".s"))]
            if not sources:
                continue
            if len(sources) != 1 or not (SRC / sources[0]).is_file():
                raise ValueError(f"expected one existing source for {output}")
            source = sources[0]
            key = (kind, Path(output))
            if key in seen or "-o" not in args or Path(args[args.index("-o") + 1]) != key[1]:
                raise ValueError(f"duplicate or mismatched compile output for {output}")
            seen.add(key)
            if kind == "target":
                covered.add(key[1])
            if source.endswith(".c"):
                commands.append(Compile(kind, output, source, args[1:]))
    if covered != objects:
        raise ValueError(f"target compile receipts differ from objects.txt: {sorted(covered ^ objects)}")
    if {cmd.kind for cmd in commands} != {"host", "target"}:
        raise ValueError("missing host or target C compile records")
    return sorted(commands)


def inventory(bcc, clang, inputs):
    classes = {
        name: {"count": 0, "examples": [], "assignment": assignment}
        for name, (_, assignment) in CLASSES.items()
    }
    by_kind = {kind: {"commands": 0, "passed": 0} for kind in ("host", "target")}
    commands = compile_commands(clang)
    headers = [
        "-nostdinc", "-I", str(ROOT / "libs/libbreenix-libc/include"),
        "-I", str(ROOT / "toolchain/include"),
    ]
    with tempfile.TemporaryDirectory() as scratch:
        for cmd in commands:
            by_kind[cmd.kind]["commands"] += 1
            # Replace -c (and the host commands' default link stage) with
            # syntax checking. Preserve the other recorded options and route
            # their dependency side effect to scratch.
            flags = [flag for flag in cmd.flags if flag != "-c"]
            if any(flag == "-MD" or flag.startswith("-Wp,-MD,") for flag in flags):
                flags += ["-MF", str(Path(scratch) / "unit.d")]
            if cmd.kind == "host" and not any(flag.startswith("-std=") for flag in flags):
                # The reference HOSTCC defaults to a GNU dialect. bcc's
                # default is strict C11; use its supported GNU C11 mode.
                flags.insert(0, "-std=gnu11")
            flags += headers
            if cmd.kind == "target":
                # These are part of the reference target wrapper, not the
                # saved Kbuild command line. bcc supplies no Linux defines.
                flags.append("-D__breenix__=1")
            result = subprocess.run(
                [str(bcc), "-fsyntax-only", "--target=x86_64", *flags],
                cwd=SRC, capture_output=True, text=True, timeout=10, check=False,
            )
            if result.returncode == 0:
                by_kind[cmd.kind]["passed"] += 1
                continue
            diagnostic = next((line for line in result.stderr.splitlines() if LOCATED.fullmatch(line)), None)
            if result.returncode != 1 or diagnostic is None:
                raise ValueError(f"{cmd.output}: bcc exited {result.returncode}: {result.stderr.strip()}")
            name = classify_diagnostic(diagnostic, SRC)
            bucket = classes[name]
            bucket["count"] += 1
            if len(bucket["examples"]) < 5:
                located = LOCATED.fullmatch(diagnostic)
                file = Path(located["file"])
                if file.is_absolute():
                    file = file.relative_to(SRC) if file.is_relative_to(SRC) else Path(file.name)
                bucket["examples"].append({
                    "translation_unit": cmd.source,
                    "kind": cmd.kind,
                    "location": f"{file}:{located['line']}",
                    "diagnostic": f"{file}{diagnostic[located.end('file'):]}",
                })
    return {
        "inputs": inputs,
        "commands": len(commands),
        "passed": sum(kind["passed"] for kind in by_kind.values()),
        "by_kind": by_kind,
        "flags": {
            kind: sorted({flag for cmd in commands if cmd.kind == kind for flag in flag_names(cmd.flags)})
            for kind in by_kind
        },
        "classes": classes,
        "scope": (
            "First located bcc diagnostic per failed clang command replay. "
            "Replay selects syntax checking, routes dependency files to scratch, and uses the "
            "Breenix toolchain and libc headers for target commands. No Mac SDK headers are added. "
            "An earlier driver or header failure can mask constructs in the same translation unit. "
            "Zero counts mean no first diagnostic of that class was observed."
        ),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bcc", type=Path, default=BCC, help="Cargo-built host bcc (default: toolchain/target/debug/bcc)")
    args = parser.parse_args()
    clang, inputs = verify_inputs()
    report = inventory(args.bcc.resolve(), clang, inputs)
    with tempfile.TemporaryDirectory(dir=OUTPUT.parent) as work:
        temporary = Path(work) / OUTPUT.name
        temporary.write_text(json.dumps(report, indent=2) + "\n")
        temporary.replace(OUTPUT)
    print(f"{report['commands']} commands, {report['passed']} passed")
    for name, entry in report["classes"].items():
        print(f"  {name}: {entry['count']}")
    print(f"Wrote {OUTPUT.relative_to(ROOT)}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        sys.exit(f"inventory: {error}")
