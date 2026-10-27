"""Score the milestones measurable by one boot shape and mode, using that boot's definition."""
import json
import re
import runpy
from pathlib import Path


# The test runner names the marker each test will print before the test runs; that promise reaches no stage.
ANNOUNCEMENT = 'kernel::test_exec:'
# Host records whose milestone is not <suite>/<category> (scripts/metal/server-suite, MILESTONES).
HOST_MILESTONES = {'gt710-completion.json': 'metal-gt710/completion', 'gt710-copy.json': 'metal-gt710/copy', 'gt710-channel.json': 'metal-gt710/channel', 'gt710-lifetime.json': 'metal-gt710/lifetime', 'gt710-aperture.json': 'metal-gt710/aperture', 'gt710-vm.json': 'metal-gt710/vm', 'gt710-inventory.json': 'metal-gt710/inventory', 'gt710-flips.json': 'metal-gt710/flips', 'gt710-memory.json': 'metal-gt710/memory', 'users-operator.json': 'users/operator-login',
                   'gt710-native.json': 'metal-gt710/native', 'gt710-switching.json': 'metal-gt710/switching',
                   'gt710-board.json': 'metal-gt710/board', 'gt710-firmware.json': 'metal-gt710/firmware', 'gt710-device.json': 'metal-gt710/device', 'gt710-console.json': 'metal-gt710/console'}


def boot_shape(disk):
    return {'install': 'install', 'root': 'disk-root', 'persistent': 'persistent', None: 'ramdisk'}[disk]


def measures(milestone, shape, mode):
    # Commits preceding shape metadata describe the original RAM-disk boots.
    return (shape in milestone.get('bootShapes', ['ramdisk'])
            and ('bootModes' not in milestone or mode in milestone['bootModes']))


def score(directory):
    directory = Path(directory)
    read = lambda name: json.loads((directory / name).read_text())
    run = read('run.json')
    if run.get('kind') == 'fault-test' or run.get('inject') in ('xhci-reset-device', 'xhci-short-read'):
        raise ValueError('fault tests are never milestone evidence')
    if run.get('kind') == 'console-session':
        raise ValueError('operator console sessions are never scored')
    if run.get('mode') == 'tests':
        return score_tests(directory, run, read)
    definition = read('definition/docs/metal/boot-path.json')
    catalog = {stage['name']: stage for stage in read(
        'definition/tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json')['stages']}
    shape = boot_shape(run.get('disk'))
    lines = (directory / 'serial.txt').read_text().splitlines()
    events = {json.loads(line)['event'] for line in (directory / 'events.jsonl').read_text().splitlines()}
    host = {'console-ready': 'console-ready', 'power-on': 'power-on', 'firmware-text': 'firmware-text',
            'post': 'post-code', 'image-complete': 'image-complete', 'handoff': 'handoff'}
    results = {}
    for milestone in definition['milestones']:
        if milestone.get('stub') or not measures(milestone, shape, run['mode']):
            continue
        lists = milestone['stages']
        stages = lists.get(run['mode'], lists.get(definition['gateArch'], []) if milestone.get('kernel') else [])
        if not stages:
            continue
        cursor, reached = 0, 0
        for stage in stages:
            spec = catalog[stage] if isinstance(stage, str) else stage
            if 'host' in spec:
                reached += host[spec['host']] in events
                continue
            alternatives = spec['marker'].split('|')
            for index in range(cursor, len(lines)):
                if (lines[index] in alternatives if spec.get('match') == 'line'
                        else any(marker in lines[index] for marker in alternatives)):
                    reached += 1
                    cursor = index + 1
                    break
        results[milestone['id']] = {'reached': reached, 'total': len(stages), 'passed': reached == len(stages)}
    # Host-driven cases are captured while the persistent boot holds its lease, then
    # scored again from the completed capture without rerunning them. Each measured
    # category's records are server-<category>.json, or users-operator.json for the
    # users suite's host-run category, beside its transcript.
    found = sorted(directory.glob('server-*.json')) + sorted(directory.glob('users-operator.json')) + sorted(directory.glob('gt710-*.json'))
    for host_records in found:
        suite, category = host_records.name[:-len('.json')].split('-', 1)
        if category.endswith('-transcript'):
            continue
        milestone = HOST_MILESTONES.get(host_records.name, suite + '/' + category)
        root = directory / 'definition'
        try:
            host = runpy.run_path(str(root / ('scripts/metal/gt710-suite' if suite == 'gt710' else 'scripts/metal/server-suite')))
            manifest = read('definition/docs/suites/' + suite + '.json')
            records = read(host_records.name)
            if records.get('milestone') != milestone:
                raise ValueError(host_records.name + ' records another milestone')
            fatal = read('definition/docs/boot-path.json')['fatal'] + [r'KERNEL PANIC:']
            effort = 'metal.json' if suite == 'gt710' else 'path.json'
            sha = (directory / 'commit').read_text().strip() if suite == 'gt710' else run.get('bootedCommit')
            if suite == 'gt710' and (not (directory / 'ended').exists()
                    or (directory / 'exit-status').read_text().strip() != '0'
                    or (directory / 'dirty').read_text().strip() != '0' or 'console-gap' in events):
                raise ValueError('GT 710 cases need a completed clean successful capture')
            reason = host['score'](manifest, read('definition/docs/efforts/' + effort), records,
                                   (directory / 'serial.txt').read_text(), run, sha, fatal)
            total = len(host['CATEGORIES'][category])
            results[milestone] = dict(reached=total, total=total, passed=True, reason=reason)
        except (OSError, ValueError, KeyError, StopIteration, TypeError, AttributeError) as error:
            results[milestone] = dict(reached=0, total=1, passed=False, reason=str(error))
    return {'bootShape': shape, 'milestones': results, 'captureComplete': 'console-gap' not in events}


def tests_verdict(directory, serial):
    """Use the boot commit's validator and contracts, never the deployed tools' checkout."""
    root = Path(directory) / 'definition'
    try:
        validate = runpy.run_path(str(root / 'scripts/test-report-verdict.py'))['completion']
        return validate(serial, root) or (False, 'userspace test report is incomplete')
    except (OSError, ValueError) as error:
        return False, f'test report validation failed: {error}'


def score_tests(directory, run, read):
    """A testing-kernel boot is scored as every testing boot is: each milestone of the commit's docs/boot-path.json by
    its x86_64 stage list, a stage reached when one of its marker's alternatives appears on a serial line."""
    definition = read('definition/docs/boot-path.json')
    catalog = {stage['name']: stage for stage in read(
        'definition/tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json')['stages']}
    serial = (directory / 'serial.txt').read_text()
    passed, reason = tests_verdict(directory, serial)
    for pattern in definition.get('fatal', []) + ['KERNEL PANIC:']:
        if re.search(pattern, serial):
            passed, reason = False, 'fatal kernel output'
    lines = [line for line in serial.splitlines() if ANNOUNCEMENT not in line]
    events = {json.loads(line)['event'] for line in (directory / 'events.jsonl').read_text().splitlines()}
    results = {}
    for milestone in definition['milestones']:
        stages = milestone['stages'].get('x86_64', [])
        if milestone.get('stub') or not stages:
            continue
        missing = []
        for stage in stages:
            spec = catalog[stage] if isinstance(stage, str) else stage
            alternatives = spec['marker'].split('|')
            if not any(marker in line for line in lines for marker in alternatives):
                missing.append(spec['name'])
        results[milestone['id']] = {'reached': len(stages) - len(missing), 'total': len(stages),
                                    'passed': not missing, 'missing': missing}
    return {'bootShape': boot_shape(run.get('disk')), 'definition': 'docs/boot-path.json',
            'milestones': results, 'captureComplete': 'console-gap' not in events,
            'testReport': {'passed': passed, 'reason': reason}}


def falls_short(verdict):
    """A finished boot falls short on incomplete capture, a failed report or any missing measured stage."""
    return (not verdict.get('testReport', {}).get('passed', True)
            or not verdict['captureComplete']
            or any(not result['passed'] for result in verdict['milestones'].values()))
