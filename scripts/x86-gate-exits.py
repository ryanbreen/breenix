#!/usr/bin/env python3
"""Classify parseable nonzero records and reject incomplete exit accounting."""
import pathlib
import re
import sys
import importlib.util


def classify_serial(serial, allowlist):
    contracts = []
    for line in allowlist.read_text().splitlines():
        if not line.strip() or line.lstrip().startswith('#'):
            continue
        fields, sep, reason = line.partition('#')
        if not sep or not reason.strip():
            raise ValueError(f'allowlist entry needs a contract reason: {line}')
        name, status = fields.split()
        # A child suffix matches only the decimal PID that fork appends.
        pattern = re.escape(name).replace(r'\*', r'[0-9]+')
        contract = (pattern, int(status), reason.strip())
        if any(c[:2] == contract[:2] for c in contracts):
            raise ValueError(f'duplicate allowlist entry: {line}')
        contracts.append(contract)
    spec = importlib.util.spec_from_file_location("sigkill_score", pathlib.Path(__file__).with_name("score-sigkill-teardown.py"))
    scorer = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(scorer)
    sigkill_starved = scorer.score(serial)[0] == 2
    lines = serial.splitlines()
    if any(re.search(r'\bSKIP\b', line) for line in lines):
        raise ValueError('completed boot contains a skipped check')
    tallies = [line for line in lines if 'TEST_TALLY:' in line]
    if not tallies:
        raise ValueError('TEST_TALLY was absent; no completed exit accounting')
    match = re.search(r'TEST_TALLY: exited=(\d+) nonzero=(\d+) failed=\[([^\r\n]*)', tallies[-1])
    if not match:
        raise ValueError('last TEST_TALLY line is malformed')
    exited, nonzero = map(int, match.group(1, 2))
    errors = []
    field, closed, tail = match[3].partition(']')
    if not closed:
        errors.append('failure list was truncated or malformed: missing closing bracket')
    failures = []
    for item in filter(None, field.split(',')):
        entry = re.fullmatch(r'([^,:\s]+):(-?\d+)', item)
        if not entry:
            errors.append(f'failure list was truncated or malformed: {item}')
            continue
        name, status = entry[1], int(entry[2])
        if status == 0:
            errors.append(f'zero status in nonzero list: {item}')
            continue
        reason = next((why for pattern, code, why in contracts
                       if code == status and re.fullmatch(pattern, name)), None)
        if name == 'sigkill_teardown_test' and status == 2 and sigkill_starved:
            reason = 'STARVED window verified from counters; boot must be rerun'
        failures.append((name, status, reason))
    started = re.search(r'\bstarted=(\d+)', tail)
    require_publications = True
    if require_publications and started and int(started[1]) != exited:
        errors.append(f'published {started[1]} processes but only {exited} exited')
    if require_publications and not started:
        errors.append('published process count is absent')
    real = 0
    for name, status, reason in failures:
        label = f'EXPECTED - {reason}' if reason else 'FAIL - asserted contract does not permit this exit'
        print(f'TEST_EXIT: program={name} status={status} {label}')
        real += reason is None
    if nonzero > exited or len(failures) != nonzero:
        errors.append(f'nonzero={nonzero} but failed=[...] contains {len(failures)} named failures (exited={exited})')
    print(f'TEST_EXITS: nonzero={nonzero} expected={len(failures)-real} failures={real}')
    if errors:
        raise ValueError("; ".join(errors))
    return real == 0


def classify(logs, allowlist):
    return classify_serial("\n".join(path.read_text(errors="replace") for path in logs), allowlist)


if __name__ == '__main__':
    try:
        ok = classify([pathlib.Path(p) for p in sys.argv[1:]],
                      pathlib.Path(__file__).with_name('x86-gate-allowlist.txt'))
    except (OSError, ValueError) as error:
        print(f'TEST_ACCOUNTING: ERROR - {error}')
        sys.exit(1)
    sys.exit(0 if ok else 1)
