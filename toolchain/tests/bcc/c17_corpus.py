#!/usr/bin/env python3
"""Check the independent C17 guest corpus in toolchain/corpus/c17.

Plan v2 25.10b's c17 category runs corrections, o2-expect and rounding-math,
and checks both the -O0 and the -O2 builds against hand-written expectations.
This check asserts the corpus format and the clause citations. It checks one
corrections line for each WG14 N2244 report that 25.10a1 ledgered in
toolchain/docs/spec-ledger.md. It also checks that the sealed reference compile
in scripts/toolchain-ref-cc accepts each program for both ELF targets at
-std=c17 -O0 and at -std=c17 -O2 -frounding-math. It never runs a program:
the expectations come from the standard, not from compiler output.

Needs BREENIX_TOOLCHAIN_CLANG and BREENIX_TOOLCHAIN_CLANG_SHA256, the pinned
stage-0 Clang that the other reference checks use.
"""

from importlib.machinery import SourceFileLoader
from importlib.util import module_from_spec, spec_from_loader
import os
from pathlib import Path
import re
import sys
import tempfile


sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[3]
CORPUS = ROOT / "toolchain/corpus/c17"
LEDGER = ROOT / "toolchain/docs/spec-ledger.md"
PROGRAMS = ("corrections", "o2-expect", "rounding-math")
MACHINES = {"x86_64": 62, "aarch64": 183}
BUILDS = {
    "O0": ({"std": "c17", "optimize": "O0", "rounding_math": False}, ("-std=c17", "-O0")),
    "O2": ({"std": "c17", "optimize": "O2", "rounding_math": True},
           ("-std=c17", "-O2", "-frounding-math")),
}

# Each citation names one pinned document and its own reference form.
DOCUMENTS = {
    "N1570": re.compile(r"(?:[1-7]|[A-K])(?:\.\d+)*(?:p\d+(?:-\d+)?)?"),
    "C17": re.compile(r"(?:[1-7]|[A-K])(?:\.\d+)*(?:p\d+(?:-\d+)?)?"),
    "N2244": re.compile(r"DR \d{3}"),
    "IEEE 754-2019": re.compile(r"\d+(?:\.\d+)*"),
}
MODES = ("nearest", "downward", "upward", "towardzero")
FLAGS = ("invalid", "divbyzero", "overflow", "underflow", "inexact")
O2_TOPICS = ("constant", "dead", "cse", "volatile", "spill")


def ledgered_reports():
    text = LEDGER.read_text(encoding="utf-8")
    section = text.split("### C17 mode and warnings (25.10a1)", 1)
    if len(section) != 2:
        raise AssertionError("spec ledger has no 25.10a1 C17 section")
    body = section[1].split("\n#", 1)[0]
    reports = re.findall(r"^\| (DR \d{3}),", body, re.MULTILINE)
    if not reports or len(reports) != len(set(reports)):
        raise AssertionError(f"spec ledger C17 table lists no distinct reports: {reports}")
    return reports


def header_citations(name, source):
    """Return {document: [references]} from the leading comment's Clauses: list."""
    match = re.match(r"/\*(.*?)\*/", source, re.DOTALL)
    if not match:
        raise AssertionError(f"{name}.c does not start with a header comment")
    lines = [re.sub(r"^\s*\*?\s?", "", line) for line in match.group(1).splitlines()]
    text = "\n".join(lines)
    clauses = re.search(r"^Clauses:(.*?)(?=^[A-Z][A-Za-z ]*:|\Z)", text, re.DOTALL | re.MULTILINE)
    if not clauses:
        raise AssertionError(f"{name}.c header has no Clauses: list")
    if not re.search(r"^Expectations: .*by hand", text, re.MULTILINE):
        raise AssertionError(f"{name}.c header does not say its expectations are by hand")
    cited = {}
    for group in " ".join(clauses.group(1).split()).rstrip(".").split(";"):
        group = group.strip()
        document = next((doc for doc in DOCUMENTS if group.startswith(doc + " ")), None)
        if document is None:
            raise AssertionError(f"{name}.c cites an unpinned document: {group!r}")
        for reference in group[len(document) + 1:].split(","):
            reference = reference.strip()
            if not DOCUMENTS[document].fullmatch(reference):
                raise AssertionError(f"{name}.c: malformed {document} reference {reference!r}")
            cited.setdefault(document, []).append(reference)
    return cited


def expectation(name):
    data = (CORPUS / f"{name}.expect").read_bytes()
    if not data.endswith(b"\n") or not data.isascii() or b"\r" in data:
        raise AssertionError(f"{name}.expect must be ASCII text ending in a newline")
    status, _, stdout = data.decode("ascii").partition("\n")
    if not re.fullmatch(r"0|[1-9][0-9]{0,2}", status) or int(status) > 255:
        raise AssertionError(f"{name}.expect: invalid exit status line {status!r}")
    lines = stdout.splitlines()
    if not lines:
        raise AssertionError(f"{name}.expect has no stdout lines")
    return int(status), lines


def check_corrections(source, cited, lines):
    reports = ledgered_reports()
    if cited.get("N2244") != reports:
        raise AssertionError(f"corrections.c cites N2244 {cited.get('N2244')}, ledger has {reports}")
    if "C17" not in cited:
        raise AssertionError("corrections.c cites no corrected C17 clause")
    for report in reports:
        if len(re.findall(rf"/\* {report}:", source)) != 1:
            raise AssertionError(f"corrections.c needs exactly one '/* {report}:' check")
    labels = [line.split(":", 1)[0] for line in lines]
    if labels != reports or any(not re.fullmatch(r"DR \d{3}:( \S+)+", line) for line in lines):
        raise AssertionError(f"corrections.expect lines {labels} differ from the ledger {reports}")


def check_o2(source, lines):
    if [line.split(" ", 1)[0] for line in lines] != list(O2_TOPICS):
        raise AssertionError(f"o2-expect.expect lines must be {O2_TOPICS} in order")
    if "volatile" not in source or "setjmp" not in source:
        raise AssertionError("o2-expect.c must make volatile accesses observable")


def check_rounding(source, cited, lines):
    if "#pragma STDC FENV_ACCESS ON" not in source:
        raise AssertionError("rounding-math.c must set FENV_ACCESS ON")
    if "IEEE 754-2019" not in cited or not any(ref.startswith("F.") for ref in cited.get("N1570", [])):
        raise AssertionError("rounding-math.c must cite N1570 Annex F and IEEE 754-2019")
    flags = re.compile(r"none|" + r"(?:" + "|".join(FLAGS) + r")(?:\|(?:" + "|".join(FLAGS) + r"))*")
    table = {}
    for line in lines:
        fields = line.split(" ")
        if (len(fields) != 4 or fields[0] not in MODES
                or not re.fullmatch(r"[0-9a-f]{16}|nan", fields[2]) or not flags.fullmatch(fields[3])):
            raise AssertionError(f"rounding-math.expect: malformed line {line!r}")
        named = fields[3].split("|")
        if fields[3] != "none" and named != [flag for flag in FLAGS if flag in named]:
            raise AssertionError(f"rounding-math.expect: flags out of order in {line!r}")
        table.setdefault(fields[0], []).append(fields[1])
    if list(table) != list(MODES) or any(table[mode] != table[MODES[0]] for mode in MODES):
        raise AssertionError("rounding-math.expect needs the same operations under each mode, in order")


def reference_compiler(work):
    scripts = ROOT / "scripts"
    sys.path.insert(0, str(scripts))
    loader = SourceFileLoader("toolchain_ref_cc", str(scripts / "toolchain-ref-cc"))
    reference = module_from_spec(spec_from_loader(loader.name, loader))
    loader.exec_module(reference)
    clang, _ = reference.pinned_tool(
        Path(os.environ["BREENIX_TOOLCHAIN_CLANG"]),
        os.environ["BREENIX_TOOLCHAIN_CLANG_SHA256"], work,
    )
    return reference, clang


def check_compiles(failures):
    with tempfile.TemporaryDirectory(prefix="c17-corpus-") as temp:
        work = Path(temp).resolve()
        reference, clang = reference_compiler(work)
        for name in PROGRAMS:
            for arch, machine in MACHINES.items():
                for build, (options, expected_flags) in BUILDS.items():
                    case = work / f"{name}-{arch}-{build}"
                    case.mkdir()
                    try:
                        obj, _, flags = reference.compile_c(
                            arch, clang, CORPUS / f"{name}.c", (CORPUS,), case, **options)
                    except Exception as error:  # report every build, not only the first
                        failures.append(f"{name}.c {arch} {build}: {type(error).__name__}: {error}")
                        continue
                    data = obj.read_bytes()
                    if (data[:4] != b"\x7fELF"
                            or int.from_bytes(data[18:20], "little") != machine):
                        failures.append(f"{name}.c {arch} {build}: not an ELF for machine {machine}")
                    missing = [flag for flag in expected_flags if flag not in flags]
                    if missing or "-std=c11" in flags:
                        failures.append(f"{name}.c {arch} {build}: flags {flags} lack {missing}")


def main():
    failures = []
    found = {path.name for path in CORPUS.iterdir()} if CORPUS.is_dir() else set()
    wanted = {f"{name}.{suffix}" for name in PROGRAMS for suffix in ("c", "expect")}
    if found != wanted:
        sys.exit(f"toolchain/corpus/c17 holds {sorted(found)}, expected {sorted(wanted)}")
    for name in PROGRAMS:
        try:
            source = (CORPUS / f"{name}.c").read_text(encoding="ascii")
            cited = header_citations(name, source)
            status, lines = expectation(name)
            if status != 0:
                raise AssertionError(f"{name}.expect: a passing program exits 0, not {status}")
            if name == "corrections":
                check_corrections(source, cited, lines)
            elif name == "o2-expect":
                check_o2(source, lines)
            else:
                check_rounding(source, cited, lines)
        except (AssertionError, UnicodeDecodeError) as error:
            failures.append(str(error))
    check_compiles(failures)
    if failures:
        sys.exit("\n".join(failures))
    print(f"c17_corpus: {len(PROGRAMS)} programs, format, citations and "
          f"{len(PROGRAMS) * len(MACHINES) * len(BUILDS)} reference compiles PASS")


if __name__ == "__main__":
    main()
