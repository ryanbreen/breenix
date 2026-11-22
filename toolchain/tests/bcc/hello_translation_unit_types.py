#!/usr/bin/env python3
"""Check the hello translation unit's types against the independent N1570 records."""

import argparse
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "cconf"))
import run as cconf


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cc", type=lambda path: Path(path).resolve())
    parser.add_argument("--target", choices=cconf.TARGETS, default="x86_64")
    args = parser.parse_args()
    args.mode = "types"
    args.types_flag = "--dump-types"
    args.cc_arg = [f"--target={args.target}"]
    cases = cconf.load_manifest(cconf.HERE / "cases.json")
    case = next(case for case in cases if case["id"] == "hello-types")
    name = "sema::hello_translation_unit_types"
    failure = cconf.run_case(case, args)
    if failure:
        sys.exit(f"{name} FAIL ({failure})")
    print(f"{name} PASS")


if __name__ == "__main__":
    main()
