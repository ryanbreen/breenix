#!/usr/bin/env python3
"""Score issue 847's transmitted records, including orphaned payload fragments."""
import re
import sys
from collections import defaultdict


def score(data):
    rows = defaultdict(set)
    corrupted = 0
    count = 0
    for line in data.replace(b'\r', b'').splitlines():
        if not re.search(rb'SERIAL_INTERLEAVE|:seq=|:payload=|[A-H]{32}', line):
            continue
        match = re.fullmatch(rb'\[SERIAL_INTERLEAVE:cpu=([0-7]):seq=([0-9]+):payload=([A-H]{768})\]', line)
        if match is None:
            corrupted += 1
            continue
        cpu, seq = int(match[1]), int(match[2])
        if seq >= 200 or seq in rows[cpu] or match[3] != bytes([65 + cpu]) * 768:
            corrupted += 1
            continue
        rows[cpu].add(seq)
        count += 1
    passed = corrupted == 0 and count == 400 and len(rows) == 2 and all(len(s) == 200 for s in rows.values())
    return passed, count, corrupted


if __name__ == '__main__':
    passed, count, corrupted = score(open(sys.argv[1], 'rb').read())
    print(f'SERIAL_INTERLEAVE: records={count} corrupted={corrupted} verdict={"PASS" if passed else "FAIL"}')
    sys.exit(0 if passed else 1)
