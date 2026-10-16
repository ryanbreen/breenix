#!/usr/bin/env python3
"""Reject watch holds that consume a suite's completion or serial-idle budget."""
import argparse
import json
from pathlib import Path


def validate(suite, pace, cases, deadline, idle):
    if suite != "breengel":
        raise ValueError("--pace is only supported by the breengel showcase")
    if pace < 0 or pace > 4294967295:
        raise ValueError("pace must be unsigned 32-bit milliseconds")
    if deadline <= 0 or idle < 0:
        raise ValueError("deadline must be positive and idle exit nonnegative")
    if pace and pace + 1000 >= 120_000:
        raise ValueError("pace must leave a second before the 120-second classified-progress watchdog")
    if pace and idle and pace + 1000 >= idle * 1000:
        raise ValueError("pace needs at least one second before the serial idle exit")
    # Keep a minute for boot, execution, rendering and the finale. A longer run
    # needs an explicitly larger deadline; watch time never silently extends it.
    if pace and pace * cases + 60_000 >= deadline * 1000:
        raise ValueError("total pace holds leave less than 60 seconds in the suite deadline")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--suite", required=True)
    parser.add_argument("--pace", type=int, required=True)
    parser.add_argument("--cases", default="")
    parser.add_argument("--gate-timeout", type=float, required=True)
    parser.add_argument("--idle-exit", type=float, required=True)
    args = parser.parse_args()
    try:
        manifest = json.loads((Path(__file__).resolve().parents[1] / "docs/suites" / f"{args.suite}.json").read_text())
        count = int(args.cases) if args.cases else sum(len(c["cases"]) for c in manifest["categories"])
        validate(args.suite, args.pace, count, args.gate_timeout, args.idle_exit)
    except (ValueError, KeyError, OSError) as error:
        parser.error(str(error))
