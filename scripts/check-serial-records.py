#!/usr/bin/env python3
"""Judge the unmodified UART capture of serial-record_test (#2411)."""
from collections import Counter
from pathlib import Path
import re
import sys


def check(path):
    # Account for the UART's CR-LF translation at line endings.
    lines = [line.removesuffix(b'\r') for line in Path(path).read_bytes().split(b'\n')]
    begin = b'SERIAL_RACE BEGIN writers=2 records=96 deaths=128'
    assert lines.count(begin) == 1, 'missing or duplicate BEGIN'
    assert lines.count(b'SERIAL_RACE DONE') == 1, 'missing or duplicate DONE'
    start, end = lines.index(begin), lines.index(b'SERIAL_RACE DONE')
    assert start < end, 'DONE precedes BEGIN'
    expected = Counter()
    for writer in range(2):
        for seq in range(96):
            size = (80, 1024, 4096)[seq % 3]
            header = f'SERIAL_RECORD writer={writer} seq={seq:03} '.encode()
            expected[header + bytes([ord('A') + writer]) * (size - 1 - len(header))] += 1
    seen = Counter(line for line in lines[start:end] if line.startswith(b'SERIAL_RECORD'))
    assert seen == expected, (
        f'record corruption: {sum((expected - seen).values())} missing/intact, '
        f'{sum((seen - expected).values())} malformed/duplicate')
    victims = [int(m[1]) for line in lines[start:end]
               if (m := re.fullmatch(rb'SERIAL_VICTIM pid=(\d+)', line))]
    assert len(victims) == len(set(victims)) == 128, 'missing/duplicate victim receipt'
    deaths = [(i, int(m[1])) for i, line in enumerate(lines[start:end], start)
              if (m := re.fullmatch(
                  rb'\[signal\] Process (\d+) \([^\r\n]*serial-record_test\) '
                  rb'terminated by signal 15 \(SIGTERM\)', line))]
    assert Counter(pid for _, pid in deaths) == Counter(victims), 'lost/split/duplicate signal evidence'
    positions = [i for i, line in enumerate(lines) if line in expected]
    assert any(min(positions) < i < max(positions) for i, _ in deaths), 'no signal/write overlap'
    print('PASS: 192 intact single-write records (80/1024/4096 bytes), 128 signal lines, overlap observed')


if __name__ == '__main__':
    try:
        check(sys.argv[1])
    except (AssertionError, IndexError) as error:
        sys.exit(f'FAIL: {error}')
