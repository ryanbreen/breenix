#!/usr/bin/env python3
"""Score retained console-panel companions, preserving runtime and scorer identities.

This command never boots. A passing collection is only one part of acceptance:
both captures need successful driver closure and an unchanged runtime at head.
"""
import argparse
from decimal import Decimal
import hashlib
import json
from pathlib import Path
import re
import runpy
import subprocess

ROOT = Path(__file__).resolve().parents[2]
NATIVE = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))
HOST_SCORERS = {'scripts/metal/console-status-acceptance.py',
                'scripts/metal/gt710-suite', 'scripts/metal/metal_score.py'}


def retained_diff(capture, head):
    """Allow host replay; guest cases and build dependencies remain runtime code."""
    if any(re.fullmatch('[a-f0-9]{40}', sha) is None for sha in (capture, head)):
        raise ValueError('capture and head require full commits')
    subprocess.run(['git', 'merge-base', '--is-ancestor', capture, head], cwd=ROOT, check=True)
    names = subprocess.check_output(
        ['git', 'diff', '--no-renames', '--name-only', '-z', capture, head], cwd=ROOT).decode().split('\0')[:-1]
    runtime = [name for name in names if name not in HOST_SCORERS
               and not name.startswith('tests/')
               and not re.fullmatch(r'docs/gui/gt710-step-9-[1-4]\.json', name)]
    if runtime:
        raise ValueError('retained capture differs from head in runtime or contract files: ' + ', '.join(runtime))
    return names


def console_duration(path, events):
    """Time this suite's DONE, rather than a later aggregate final marker."""
    serial = (path / 'serial.txt').read_bytes()
    done = list(re.finditer(rb'^SUITE console DONE passed=88 failed=0 skipped=0 total=88\r?$', serial, re.M))
    handoff = [Decimal(str(e['t'])) for e in events if e.get('event') == 'handoff']
    stamps = [(Decimal(t), int(offset)) for t, offset in
              (line.split() for line in (path / 'serial.ts').read_text().splitlines())]
    times = [t for t, offset in stamps if len(done) == 1 and offset == done[0].start()]
    if len(handoff) != 1 or len(times) != 1:
        raise ValueError('console DONE requires one exact byte-offset timestamp and one handoff')
    seconds = times[0] - handoff[0]
    if not seconds.is_finite() or not 0 < seconds <= 330:
        raise ValueError('console handoff-to-DONE must be positive and at most 330 seconds')
    return int(seconds * 1000)


def inspect_capture(path, suite, head):
    """Report historical observations even when they cannot establish acceptance."""
    errors = []
    report = dict(suite=suite, errors=errors, passed=False)
    try:
        sha = (path / 'commit').read_text().strip()
        boot = json.loads((path / 'run.json').read_text())
        report.update(id=boot['id'], commit=sha, toolsCommit=boot.get('toolsCommit'), build=boot.get('build'))
        try:
            report['hostOnlyDiff'] = retained_diff(sha, head)
        except (ValueError, subprocess.CalledProcessError) as error:
            errors.append(str(error))
        capture = NATIVE['completed_suite_capture'](path, sha, suite)
        report['exitStatus'] = capture['exit_status']
        if capture['exit_status'] != 0:
            errors.append('capture exit is nonzero; a passing suite window does not override shared exit status')
        serial = capture['serial']
        report['done'] = re.findall(r'^SUITE ' + suite + r' DONE .*$', serial, re.M)
        report['memory'] = re.findall(r'^SUITE ' + suite + r' MEMORY reclaimed=.*$', serial, re.M)
        if suite == 'console':
            NATIVE['core_console_verdict'](serial)
            report['consoleHandoffToDoneMS'] = console_duration(path, capture['events'])
            categories = [c for c in NATIVE['capture_categories'](capture['boot']) if c != 'core']
            # Select console/acceptance for an aggregate console window without
            # rewriting its suite metadata (gt710-suite::resources_identity).
            if 'console' not in categories:
                categories.append('console')
            rows = [row for category in categories for row in NATIVE['cases'](category, serial)]
            report['notMeasured'] = {'gpu-core': 'requires separate production and diagnostic companions'}
        else:
            rows = NATIVE['cases']('breengel-full', '', {'breengel': capture})
        report['checks'] = rows
        if not rows or any(row['result'] != 'PASS' for row in rows):
            errors.append('one or more scorer checks failed')
        if len(report['memory']) != 1 or not report['memory'][0].startswith(
                'SUITE ' + suite + ' MEMORY reclaimed=PASS '):
            errors.append('suite memory reclamation is not PASS')
        raw = (path / 'serial.txt').read_text()
        report['infoByteArrayPaths'] = len(re.findall(
            r'^\[ INFO\].*(?:syscall::fs|sys_link|linkat).*\[[0-9]+, [0-9]+', raw, re.M))
        if report['infoByteArrayPaths']:
            errors.append('INFO filesystem path byte arrays remain')
        report['artifacts'] = {str(p.relative_to(path)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in sorted(path.rglob('*')) if p.is_file() and 'definition' not in p.relative_to(path).parts
            and (p.name in ('run.json', 'commit', 'dirty', 'ended', 'ended-by', 'exit-status',
                            'serial.txt', 'serial.ts', 'events.jsonl') or p.suffix == '.rle')}
    except (OSError, ValueError, KeyError) as error:
        errors.append(str(error))
    report['passed'] = not errors
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--console', type=Path, required=True)
    parser.add_argument('--breengel', type=Path, required=True)
    parser.add_argument('--sha', required=True, help='current head whose acceptance is being assessed')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    if head != args.sha or subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT, text=True).strip():
        raise ValueError('acceptance requires the clean committed scorer at the requested head')
    reports = [inspect_capture(args.console, 'console', head), inspect_capture(args.breengel, 'breengel', head)]
    proof = dict(head=head, scorerCommit=head, passed=all(r['passed'] for r in reports), captures=reports,
                 scorerSources={name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest()
                                for name in sorted(HOST_SCORERS)})
    args.output.write_text(json.dumps(proof, indent=2) + '\n')
    for report in reports:
        print(f"{report['suite']} {report.get('id', '?')} runtime={report.get('commit', '?')} "
              f"exit={report.get('exitStatus', '?')} acceptance={'PASS' if report['passed'] else 'FAIL'}")
        for error in report['errors']:
            print('  ' + error)
    print('acceptance=' + ('PASS' if proof['passed'] else 'FAIL') + '; proof=' + str(args.output))
    return 0 if proof['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
