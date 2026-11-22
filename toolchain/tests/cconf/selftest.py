#!/usr/bin/env python3
"""Check the cconf fixture corpus and its runner before any C compiler exists.

The runner is exercised with fake_cc.py, a test double that answers from the
fixture expectations. A faithful double must pass every fixture, and each
perturbed double must fail every fixture its perturbation reaches, so no
fixture or comparison is vacuous. The pp-token checks follow N1570 6.4.
"""

import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile


HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

# N1570 clause 6.10 EXAMPLEs, listed from the standard's text independently of
# cases.json, with the least number of fixtures each needs.  6.10.3.5 EXAMPLE 6
# has one valid sequence and four invalid redefinitions.
REQUIRED_EXAMPLES = {
    "6.10p8 EXAMPLE": 1,
    "6.10.2p7 EXAMPLE 1": 1,
    "6.10.2p8 EXAMPLE 2": 1,
    "6.10.3.3p4 EXAMPLE": 1,
    "6.10.3.4p4 EXAMPLE": 1,
    "6.10.3.5p3 EXAMPLE 1": 1,
    "6.10.3.5p4 EXAMPLE 2": 1,
    "6.10.3.5p5 EXAMPLE 3": 1,
    "6.10.3.5p6 EXAMPLE 4": 1,
    "6.10.3.5p7 EXAMPLE 5": 1,
    "6.10.3.5p8 EXAMPLE 6": 5,
    "6.10.3.5p9 EXAMPLE 7": 1,
    "6.10.9p2 EXAMPLE": 1,
}
PHASE_TOPICS = {"trigraphs", "splices", "splice-inside-token", "comments", "pp-tokens",
                "locations"}
DIRECTIVE_TOPICS = {"macro-replacement", "stringize", "paste", "rescan",
                    "recursion-suppression", "variadic", "conditional-width", "include",
                    "line", "pragma-operator"}
BASES = {"constraint", "syntax", "error-directive", "profile"}
CITATION = re.compile(r"^N1570 \d+(\.\d+)*(p\d+)?( EXAMPLE( \d+)?| footnote \d+)?$")


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def runner(*args, env=None):
    return subprocess.run([sys.executable, str(HERE / "run.py"), *map(str, args)],
                          capture_output=True, text=True, timeout=55, env=env)


def citations(case):
    clause = case["clause"]
    return clause if isinstance(clause, list) else [clause]


def check_absent_compiler():
    import run  # noqa: F401  (the runner module must exist)
    with tempfile.TemporaryDirectory(prefix="cconf-absent-") as scratch:
        plain = Path(scratch) / "not-executable"
        plain.write_text("")
        for missing in (Path(scratch) / "no-such-cc", Path(scratch), plain):
            for mode in ("E", "syntax", "types"):
                result = runner("--cc", missing, "--mode", mode, "--types-flag", "-x")
                require(result.returncode == 2,
                        f"{missing.name} {mode}: exit {result.returncode}, wanted 2")
                require("Traceback" not in result.stderr, f"{missing.name}: traceback")
                require("compiler not found or not executable" in result.stderr,
                        f"{missing.name}: no clean message: {result.stderr!r}")
                require("PASS" not in result.stdout and "FAIL" not in result.stdout,
                        f"{missing.name}: fixtures ran without a compiler")


def check_pp_tokens():
    from run import output_items, pp_tokens
    table = [
        # N1570 6.4p6 EXAMPLE 2: longest match, x ++ ++ + y.
        ("x+++++y", ["x", "++", "++", "+", "y"]),
        # 6.4p5 EXAMPLE 1 and 6.4.8: pp-numbers absorb e/E/p/P signs and letters.
        ("1Ex 1E1 0xe+1 0x1p-2 .5.e+x 1.2.3", ["1Ex", "1E1", "0xe+1", "0x1p-2", ".5.e+x",
                                             "1.2.3"]),
        ("1-1 0x1p", ["1", "-", "1", "0x1p"]),
        # 6.4.7p4 EXAMPLE, first line: < is not a header name outside #include.
        ("0x3<1/a.h>1e2", ["0x3", "<", "1", "/", "a", ".", "h", ">", "1e2"]),
        # 6.4.6: digraphs and the longest punctuators.
        ("%:%: %:% <::> <%%> ... .. <<= >>= ->", ["%:%:", "%:", "%", "<:", ":>", "<%",
                                                   "%>", "...", ".", ".", "<<=", ">>=",
                                                   "->"]),
        # 6.4.4.4, 6.4.5: escapes and encoding prefixes stay inside one token.
        (r'"a\"b" u8"x" L"y" U"z" u"w" L' "'a' '\\'' 'ab'",
         ['"a\\"b"', 'u8"x"', 'L"y"', 'U"z"', 'u"w"', "L'a'", "'\\''", "'ab'"]),
        ("u8 L x8\"s\"", ["u8", "L", "x8", '"s"']),
        # 6.4p1: a character that is no other pp-token stands alone.
        ("a@b \\ `", ["a", "@", "b", "\\", "`"]),
    ]
    for text, expected in table:
        got = pp_tokens(text)
        require(got == expected, f"{text!r}: {got} != {expected}")
    items = output_items('# 1 "a.c"\n#line 7 "b.c"\nx y\n  #  pragma  p  "q"\n'
                         '# include < f . h >\n%:pragma r\nz\n')
    require(items == ["x", "y", ("#pragma", ("p", '"q"')), "#", "include", "<", "f",
                      ".", "h", ">", ("#pragma", ("r",)), "z"], f"output items {items}")


def load_cases():
    from run import load_manifest
    cases = load_manifest(HERE / "cases.json")
    require(cases, "cases.json lists no fixtures")
    ids = [case["id"] for case in cases]
    require(len(ids) == len(set(ids)), "duplicate fixture ids")
    return cases


def check_citations_and_examples():
    cases = load_cases()
    for case in cases:
        for cite in citations(case):
            require(CITATION.match(cite), f"{case['id']}: malformed citation {cite!r}")
        require(case["mode"] in ("E", "syntax", "types"), f"{case['id']}: mode")
        require(case["stage"] in ("a1", "a2", "b1"), f"{case['id']}: stage")
        require((HERE / case["source"]).is_file(), f"{case['id']}: no source")
        for expect in case.get("expect", []):
            require((HERE / expect).is_file(), f"{case['id']}: no {expect}")
        require(("expect" in case) + ("diagnostic" in case) + ("accept" in case) == 1,
                f"{case['id']}: needs exactly one of expect, diagnostic, accept")
        require(case["mode"] != "types" or "expect" in case, f"{case['id']}: types")
    for example, least in REQUIRED_EXAMPLES.items():
        found = [case["id"] for case in cases if f"N1570 {example}" in citations(case)]
        require(len(found) >= least, f"{example}: {len(found)} fixtures, need {least}")


def check_topics():
    cases = load_cases()
    for stage, topics in (("a1", PHASE_TOPICS), ("a2", DIRECTIVE_TOPICS)):
        present = {topic for case in cases if case["stage"] == stage
                   for topic in case.get("topics", [])}
        require(topics <= present, f"{stage} lacks topics {sorted(topics - present)}")
    located = [case for case in cases if case["stage"] == "a1" and "diagnostic" in case
               and "locations" in case.get("topics", [])]
    require(len(located) >= 2, "a1 needs located diagnostics after splices and trigraphs")
    for kind in ("expect", "accept", "diagnostic"):
        require(any(case["stage"] == "b1" and kind in case for case in cases),
                f"b1 lacks a fixture with {kind}")
    require(any(case["mode"] == "types" for case in cases), "no hello types fixture")


def check_negative_anchors():
    cases = load_cases()
    negatives = [case for case in cases if "diagnostic" in case]
    require(negatives, "no negative fixtures")
    for case in negatives:
        diag = case["diagnostic"]
        for field in ("line", "column"):
            require(isinstance(diag[field], int) and diag[field] >= 1,
                    f"{case['id']}: {field}")
        require(diag.get("at"), f"{case['id']}: no anchor text")
        require(case.get("basis") in BASES, f"{case['id']}: basis {case.get('basis')}")
        require(case["basis"] != "profile" or case.get("profile_reason"),
                f"{case['id']}: profile without a reason")
        physical = case.get("physical", {"file": diag["file"], "line": diag["line"]})
        lines = (HERE / physical["file"]).read_bytes().split(b"\n")
        require(physical["line"] <= len(lines), f"{case['id']}: line beyond the file")
        text = lines[physical["line"] - 1]
        require(text[diag["column"] - 1:].startswith(diag["at"].encode()),
                f"{case['id']}: {physical['file']}:{physical['line']}:{diag['column']}"
                f" is {text[diag['column'] - 1:][:20]!r}, not {diag['at']!r}")


def scored(result):
    lines = [line for line in result.stdout.splitlines() if line.startswith("cconf::")]
    passed = {line.split()[0][7:] for line in lines if " PASS" in line}
    failed = {line.split()[0][7:] for line in lines if " FAIL" in line}
    return passed, failed


def fake_run(mode, perturb, target="x86_64"):
    env = dict(os.environ, CCONF_FAKE=perturb, CCONF_FAKE_TARGET=target)
    return runner("--cc", HERE / "fake_cc.py", "--mode", mode, "--target", target,
                  "--types-flag", "-fcconf-dump-types", env=env)


def check_faithful_compiler_passes():
    cases = load_cases()
    for target in ("x86_64", "aarch64"):
        for mode in ("E", "syntax", "types"):
            wanted = {case["id"] for case in cases if case["mode"] == mode
                      and target in case.get("targets", ["x86_64", "aarch64"])}
            result = fake_run(mode, "faithful", target)
            passed, failed = scored(result)
            require(result.returncode == 0 and not failed and passed == wanted,
                    f"{mode}/{target}: exit {result.returncode}, failed {sorted(failed)},"
                    f" missing {sorted(wanted - passed)}\n{result.stderr}")


def check_wrong_compilers_fail():
    cases = load_cases()
    x86 = [case for case in cases if "x86_64" in case.get("targets", ["x86_64"])]
    positive = {mode: {c["id"] for c in x86 if c["mode"] == mode and "diagnostic" not in c}
                for mode in ("E", "syntax", "types")}
    negative = {mode: {c["id"] for c in x86 if c["mode"] == mode and "diagnostic" in c}
                for mode in ("E", "syntax")}
    perturbations = [
        ("E", "extra-token", positive["E"]),
        ("E", "spurious-error", positive["E"]),
        ("syntax", "spurious-error", positive["syntax"]),
        ("types", "wrong-type", positive["types"]),
        ("types", "missing-record", positive["types"]),
    ]
    for mode in ("E", "syntax"):
        for perturb in ("wrong-line", "wrong-column", "wrong-file", "warning-exit-zero",
                        "earlier-error"):
            perturbations.append((mode, perturb, negative[mode]))
    for mode, perturb, reached in perturbations:
        require(reached, f"{mode}/{perturb} reaches no fixture")
        result = fake_run(mode, perturb)
        passed, failed = scored(result)
        require(result.returncode == 1, f"{mode}/{perturb}: exit {result.returncode}")
        require(failed == reached,
                f"{mode}/{perturb}: passed {sorted(reached - failed)} that should fail")


CHECKS = [
    ("cconf::absent_compiler_fails_cleanly", check_absent_compiler),
    ("cconf::pp_tokens_follow_n1570_6_4", check_pp_tokens),
    ("cconf::every_6_10_example_has_a_fixture", check_citations_and_examples),
    ("cconf::phase_and_directive_topics_covered", check_topics),
    ("cconf::negative_anchors_point_at_their_text", check_negative_anchors),
    ("cconf::faithful_compiler_passes_every_fixture", check_faithful_compiler_passes),
    ("cconf::wrong_compilers_fail_every_reached_fixture", check_wrong_compilers_fail),
]


def main():
    failures = 0
    for name, check in CHECKS:
        try:
            check()
        except (ImportError, KeyError, OSError, TypeError, ValueError, json.JSONDecodeError,
                subprocess.SubprocessError) as error:
            failures += 1
            print(f"{name} FAIL ({type(error).__name__}: {error})", flush=True)
        else:
            print(f"{name} PASS", flush=True)
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
