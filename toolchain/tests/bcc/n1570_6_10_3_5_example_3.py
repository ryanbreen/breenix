#!/usr/bin/env python3
"""Check bcc's rescanning against the independent N1570 6.10.3.5 EXAMPLE 3 fixture."""

import argparse
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "cconf"))
import run as cconf


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cc", type=lambda path: Path(path).resolve())
    args = parser.parse_args()
    args.mode = "E"
    args.cc_arg = []
    cases = cconf.load_manifest(cconf.HERE / "cases.json")
    case = next(case for case in cases if case["id"] == "macro-scope-example3")
    name = "pp::n1570_6_10_3_5_example_3"
    failure = cconf.run_case(case, args)
    if failure:
        sys.exit(f"{name} FAIL ({failure})")
    print(f"{name} PASS")


if __name__ == "__main__":
    main()
