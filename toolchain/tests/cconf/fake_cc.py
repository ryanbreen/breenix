#!/usr/bin/env python3
"""A test double for selftest.py: answers each fixture from its own expectation.

CCONF_FAKE selects the behaviour: `faithful` answers exactly as the fixture
expects, and each other value makes one kind of wrong answer that the runner
must reject. CCONF_FAKE_TARGET names the target whose fixture is answered.
It is not a compiler and never reads the source it is given.
"""

import json
import os
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

from run import load_manifest  # noqa: E402


def emit_diagnostic(diag, perturb, source):
    file, line, column = diag["file"], diag["line"], diag["column"]
    message = diag.get("contains", "cconf fake diagnostic")
    if perturb == "wrong-line":
        line += 1
    elif perturb == "wrong-column":
        column += 1
    elif perturb == "wrong-file":
        file = "cconf-wrong-file.c"
    elif perturb == "earlier-error":
        print(f"{source}:9999:1: error: an earlier, misplaced error", file=sys.stderr)
    severity = "warning" if perturb == "warning-exit-zero" else "error"
    print(f"{file}:{line}:{column}: {severity}: {message}", file=sys.stderr)
    return 0 if perturb == "warning-exit-zero" else 1


def main():
    argv = sys.argv[1:]
    perturb = os.environ.get("CCONF_FAKE", "faithful")
    target = os.environ.get("CCONF_FAKE_TARGET", "x86_64")
    source = argv[-1]
    mode = "E" if "-E" in argv else "types" if "-fcconf-dump-types" in argv else "syntax"
    cases = [case for case in load_manifest(HERE / "cases.json")
             if case["source"] == source and case["mode"] == mode
             and target in case.get("targets", ["x86_64", "aarch64"])]
    if len(cases) != 1:
        print(f"fake_cc: {len(cases)} fixtures for {source} {mode} {target}", file=sys.stderr)
        return 70
    case = cases[0]
    if "diagnostic" in case:
        return emit_diagnostic(case["diagnostic"], perturb, source)
    if mode == "E":
        text = (HERE / case["expect"][0]).read_text(encoding="utf-8")
        sys.stdout.write(text + ("\ncconf_extra_token\n" if perturb == "extra-token" else ""))
    elif mode == "types":
        records = json.loads((HERE / case["expect"][0]).read_text(encoding="utf-8"))["records"]
        if perturb == "wrong-type":
            records[0] = dict(records[0], type="long double")
        elif perturb == "missing-record":
            records = records[:-1]
        for record in records:
            print(json.dumps(record))
    if perturb == "spurious-error":
        print(f"{source}:1:1: error: a spurious error", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
