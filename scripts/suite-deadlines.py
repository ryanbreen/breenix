#!/usr/bin/env python3
"""Calculate suite DONE deadlines from the guest runner's case allowances."""

import argparse
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]


def suite_deadlines(suites, arch):
    runner = (ROOT / "libs/libbreenix/src/suite.rs").read_text()
    default = int(re.search(r"DEFAULT_CASE_LIMIT_MS: u64 = ([\d_]+);", runner)[1].replace("_", ""))
    names = ["NAMED_CASE_ALLOWANCES"]
    if arch == "aarch64":
        names.insert(0, "ARM_BUSYBOX_CASE_ALLOWANCES")
    elif arch != "x86_64":
        raise ValueError(f"unknown architecture {arch}")
    allowances = {}
    for name in names:
        table = runner.split("pub const " + name, 1)[1].split("];", 1)[0]
        for suite, case, ms in re.findall(r'\("([^\"]+)", "([^\"]+)", ([\d_]+)\)', table):
            allowances.setdefault((suite, case), int(ms.replace("_", "")))
    result = {}
    for suite in suites:
        manifest = json.loads((ROOT / "docs/suites" / f"{suite}.json").read_text())
        total = sum(allowances.get((suite, f'{category["id"]}/{case["id"]}'), default)
                    for category in manifest["categories"] for case in category["cases"])
        result[suite] = (total * 5 + 3999) // 4000
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--arch", choices=("aarch64", "x86_64"), required=True)
    parser.add_argument("suites", help="comma-separated suite IDs")
    args = parser.parse_args()
    print(json.dumps(suite_deadlines(args.suites.split(","), args.arch), separators=(",", ":")))
