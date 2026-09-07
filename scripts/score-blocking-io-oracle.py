#!/usr/bin/env python3
"""Score a saved oracle boot; arguments: architecture, serial directory, arms."""
import pathlib, re, sys
arch, directory, *arms = sys.argv[1:]
text = '\n'.join(p.read_text(errors='replace') for p in pathlib.Path(directory).glob('*.txt'))
# Remove only the two known GDT initialization messages, not whole lines:
# a fault banner on the same line must still fail the gate.
crash_text = re.sub(
    r'(?:TSS IST\[0\] \(double fault stack\):|Updated IST\[0\] \(double fault stack\) to) 0x[0-9a-f]+\b',
    '', text)
assert not re.search(r'KERNEL PANIC|panic!|DATA_ABORT|INSTRUCTION_ABORT|Unhandled sync exception|DOUBLE FAULT|TRIPLE FAULT|soft lockup detected', crash_text, re.I), 'kernel crash'
results = re.findall(r'\[PIPE_WRITE_RESULT:([^\]]+)\]', text)
assert results == [f'{arch}:status=0'], f'missing/duplicate/nonzero reaped result: {results}'
assert ':verdict=FAIL' not in text, 'oracle arm failed'
records = re.findall(r'\[PIPE_WRITE_ORACLE:([^\]]+)\]', text)
for kind in ('pipe', 'fifo'):
    for arm in arms:
        prefix = f'{arch}:{kind}:{arm}:verdict=PASS:bytes='
        matches = [r for r in records if r.startswith(prefix)]
        assert matches, f'missing {kind}/{arm}'
        for record in matches:
            counts = re.fullmatch(re.escape(prefix) + r'(\d+):expected=(\d+)', record)
            assert counts and counts[1] == counts[2], f'byte tally: {record}'
summary = f'[PIPE_WRITE_SUMMARY:{arch}:passed={2 * len(arms)}:failed=0]'
assert summary in text, 'missing complete arm tally'
print(f'PASS: {arch}, {2 * len(arms)} arms, exact byte tallies and worker reaped')
