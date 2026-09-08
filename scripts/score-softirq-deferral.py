#!/usr/bin/env python3
"""Score issue 891's daemon callback evidence; exit 2 is host starvation."""
import pathlib
import re
import sys

PATTERN = re.compile(
    r"\[SOFTIRQ_DEFERRAL_ORACLE:arch=(x86|aarch64):cpu=(\d+):budget_ticks=(\d+)"
    r":wait_ticks=(\d+):wait_ns=(\d+):dispatches=(\d+):iterations=(\d+):verdict=(ok|starved|lost)\]"
)


def score(text):
    lines = [line for line in text.splitlines() if "[SOFTIRQ_DEFERRAL_ORACLE:" in line]
    if len(lines) != 1:
        return 1, f"softirq deferral: FAIL: expected one oracle, found {len(lines)}"
    match = PATTERN.search(lines[0])
    if not match or lines[0][match.end():].strip():
        return 1, "softirq deferral: FAIL: malformed oracle"
    arch, cpu, budget, ticks, ns, dispatches, iterations, verdict = match.groups()
    if int(budget) != 250:
        return 1, "softirq deferral: FAIL: unexpected tick budget"
    if verdict == "ok" and (int(dispatches) == 0 or int(iterations) < 25):
        return 1, "softirq deferral: FAIL: ok without daemon completion"
    if verdict == "starved" and int(ticks) >= int(budget):
        return 1, "softirq deferral: FAIL: guest execution mislabeled starved"
    if verdict == "starved" and int(ns) < 15_000_000_000:
        return 1, "softirq deferral: FAIL: starvation backstop not elapsed"
    return {"ok": 0, "lost": 1, "starved": 2}[verdict], lines[0]


if __name__ == "__main__":
    status, message = score("\n".join(pathlib.Path(p).read_text(errors="replace") for p in sys.argv[1:]))
    print(message)
    sys.exit(status)
