#!/usr/bin/env python3
"""Judge a targeted present-rate capture and report its texture disposal costs."""
import ast
import json
from pathlib import Path
import re
import sys


def check(directory):
    serial = (directory / 'serial.txt').read_text()
    bulk = (directory / 'gt710-final.rle').read_text()
    values = {key: int(value) for key, value in re.findall(
        r'VALUE composition/present-rate cadence-([\w-]+)=(\d+)', serial)}
    composed = values['end-count'] - values['start-count']
    elapsed = values['end-ns'] - values['start-ns']
    vblanks = (values['vblank-1-1'] - values['vblank-0-1']) & 0xffff
    samples = {}
    for seed, stamps in re.findall(r'^FIXTURE_CADENCE seed=(\d+) stamps=(.+)$', bulk, re.M):
        points = ast.literal_eval(stamps)
        samples[int(seed)] = points[1][0] - points[0][0]
    assert set(samples) == set(range(300)), 'missing producer disposal samples'
    report = {'composites': composed, 'vblanks': vblanks, 'elapsed_ns': elapsed,
              'rate_millihertz': composed * 10**12 // elapsed,
              'disposal_ns': {str(i): samples[i] for i in range(300)}}
    print(json.dumps(report, indent=2))
    assert 'CASE composition/present-rate PASS ' in serial, 'present-rate case failed'
    assert elapsed > 0 and composed * 10**9 >= 60 * elapsed, 'rate below 60 Hz'
    assert 'DONE passed=1 failed=0 skipped=0 total=1 only=composition/present-rate' in serial


if __name__ == '__main__':
    check(Path(sys.argv[1]))
