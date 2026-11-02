#!/usr/bin/env python3
"""Re-score a retained A2 boot store and save host-only admission receipts.

The input is the private run's retained boots/ directory, not Vigil's suite
tallies alone. Keep that store and this command's JSON together under the
Vigil boot's host-evidence directory before deleting the disposable VM run.
Head applicability requires an ancestor capture with only this auditor and its
host check changed afterward. This measures the five ARM server categories only. Console timing and GT 710
graphics need separate captures. No VM is launched and no lease is acquired.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import runpy
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
RUNNER = ROOT / 'scripts/metal/server-suite'
CATEGORIES = ('ssh-keys', 'services', 'logs', 'time', 'console-input')
HOST_SCORING_PATHS = {'scripts/check-arm-a2-capture.py', 'tests/arm_a2_capture_test.py'}


def capture_delta(captured, head, repository=ROOT):
    """Establish the narrow host-scoring exception from actual Git history."""
    def git(*args):
        return subprocess.check_output(['git', '-C', str(repository), *args], text=True)

    ancestor = subprocess.run(['git', '-C', str(repository), 'merge-base',
                               '--is-ancestor', captured, head],
                              capture_output=True, text=True)
    if ancestor.returncode not in (0, 1):
        raise ValueError('cannot establish capture ancestry: ' + ancestor.stderr.strip())
    changed = sorted(filter(None, git('diff', '--name-only', '-z', captured, head).split('\0')))
    clean = not git('status', '--porcelain').strip()
    return dict(ancestor=ancestor.returncode == 0, changedPaths=changed,
                workingTreeClean=clean,
                runtimeUnchanged=ancestor.returncode == 0 and clean
                and set(changed) <= HOST_SCORING_PATHS)


def hashes(run):
    return {str(p.relative_to(run)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in sorted((run / 'boots').rglob('*')) if p.is_file()}


def audit(run, sha, ended_boot, previous_boot):
    run = Path(run)
    if (run / 'boot.lock').exists() or any(p.is_socket() for p in run.rglob('*')):
        raise ValueError('refusing active run; supply an offline retained boot store')
    if not re.fullmatch('[a-f0-9]{40}', sha):
        raise ValueError('capture commit must be a full SHA')
    for boot in (ended_boot, previous_boot):
        if not re.fullmatch(r'\d{8}T\d{6}Z-qemu', boot):
            raise ValueError('invalid QEMU boot ID')
    before = hashes(run)
    host = runpy.run_path(str(RUNNER))
    report = dict(scope='ARM E16 A2 server categories; console and graphics not measured',
                  captureCommit=sha, scorerCommit=subprocess.check_output(
                      ['git', '-C', str(ROOT), 'rev-parse', 'HEAD'], text=True).strip(),
                  runnerSha256=hashlib.sha256(RUNNER.read_bytes()).hexdigest(),
                  boots=[], newBoots=[], categories=[], controls=[],
                  captureSha256=before, reasons=[])
    try:
        report['captureDelta'] = capture_delta(sha, report['scorerCommit'])
        if not report['captureDelta']['runtimeUnchanged']:
            report['reasons'].append('runtime changed or capture ancestry/clean worktree not established; '
                                     'retained results do not establish acceptance at scorer head')
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        report['reasons'].append('capture provenance: ' + str(error))
    reasons = report['reasons']
    for category in CATEGORIES:
        candidates = []
        for path in sorted((run / 'boots').glob('*/server-' + category + '.json')):
            records = json.loads(path.read_text())
            if records.get('commit') == sha:
                candidates.append((path, records))
        if len(candidates) != 1:
            reasons.append(f'{category}: expected one record at capture commit, found {len(candidates)}')
            continue
        path, records = candidates[0]
        directory = path.parent
        row = dict(category=category, boot=directory.name, passed=0, proven=False)
        report['categories'].append(row)
        try:
            boot = json.loads((directory / 'run.json').read_text())
            if boot.get('id') != directory.name or boot.get('target') != 'qemu':
                raise ValueError('capture identity does not match its QEMU directory')
            definition = directory / 'definition'
            manifest = json.loads((definition / 'docs/suites/server.json').read_text())
            effort = json.loads((definition / 'docs/efforts/path.json').read_text())
            fatal = json.loads((definition / 'docs/boot-path.json').read_text())['fatal'] + [r'KERNEL PANIC:']
            if records.get('milestone') != 'server/' + category:
                raise ValueError('category record names another milestone')
            verdict = host['score'](manifest, effort, records, (directory / 'serial.txt').read_text(),
                                    boot, sha, fatal)
            row.update(passed=len(host['CATEGORIES'][category]), proven=True, verdict=verdict)
        except (OSError, ValueError, KeyError) as error:
            row['error'] = str(error)
            reasons.append(category + ': ' + str(error))

    # This offline replay checks inactive-run refusal, not the ended-marker
    # predicate: no lease or live QMP exists here. The ended guard is exercised
    # in tests/server_target_test.py with the other lease conditions satisfied.
    # Use genuine retained identities. A mismatched request SHA is not a
    # substitute for a preparation actually produced by another commit.
    try:
        ended = run / 'boots' / ended_boot
        prior = run / 'boots' / previous_boot
        identity = json.loads((ended / 'run.json').read_text())
        old = json.loads((prior / 'run.json').read_text())
        preparation = json.loads((prior / 'logs-prepared.json').read_text())
        if not (ended / 'ended').read_text().strip() or identity.get('bootedCommit') != sha:
            raise ValueError('ended control needs an ended boot at the capture commit')
        if (not (prior / 'ended').read_text().strip() or old.get('id') != previous_boot
                or not re.fullmatch('[a-f0-9]{40}', old.get('bootedCommit', ''))
                or old.get('bootedCommit') == sha or old.get('toolsCommit') != old.get('bootedCommit')
                or old.get('mode') != 'persistent' or old.get('evidenceEligible') is not True
                or preparation.get('boot') != previous_boot
                or preparation.get('commit') != old.get('bootedCommit')):
            raise ValueError('cross-commit preparation identity is inconsistent')
        for name, category, extra, expected in (
                ('offline-inactive-run', 'ssh-keys', [], 'this boot does not hold the machine lease'),
                ('cross-commit', 'logs', ['--previous-boot', previous_boot],
                 'previous log preparation must be a completed persistent boot at this commit')):
            args = [sys.executable, str(RUNNER), '--target', 'qemu', '--run-dir', str(run),
                    '--boot', ended_boot, '--sha', sha, '--milestone', 'server/' + category, *extra]
            result = subprocess.run(args, capture_output=True, text=True, timeout=15)
            passed = result.returncode == 1 and result.stderr.rstrip().endswith('ValueError: ' + expected)
            receipt = dict(control=name, command=args, exitStatus=result.returncode,
                           stdout=result.stdout, stderr=result.stderr, passed=passed,
                           replay='offline host admission; no live guest')
            if name == 'cross-commit':
                receipt.update(previousBoot=previous_boot, previousCommit=old['bootedCommit'])
            report['controls'].append(receipt)
            if not passed:
                reasons.append(name + ': expected admission refusal was not observed')
    except (OSError, ValueError, KeyError, subprocess.TimeoutExpired) as error:
        reasons.append('controls: ' + str(error))
    if hashes(run) != before:
        reasons.append('retained capture changed during scoring')
    report['boots'] = sorted({row['boot'] for row in report['categories']})
    report['proven'] = not reasons
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('run', type=Path)
    parser.add_argument('--sha', required=True, help='actual captured commit, not a later scorer head')
    parser.add_argument('--ended-boot', required=True)
    parser.add_argument('--previous-boot', required=True, help='genuine different-commit logs preparation')
    parser.add_argument('--output', type=Path, required=True, help='new receipt file beside retained boots/')
    args = parser.parse_args()
    # Reserve the receipt before doing work so a repeated invocation cannot
    # overwrite the first score and its refusal transcripts.
    with args.output.open('x') as output:
        report = audit(args.run, args.sha, args.ended_boot, args.previous_boot)
        output.write(json.dumps(report, indent=2) + '\n')
    for row in report['categories']:
        print(row.get('verdict', row.get('error', 'missing verdict')))
    for receipt in report['controls']:
        print(receipt['control'] + ': ' + ('PASS' if receipt['passed'] else 'FAIL'))
    for reason in report['reasons']:
        print('FAIL: ' + reason)
    print('receipt=' + str(args.output))
    return 0 if report['proven'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
