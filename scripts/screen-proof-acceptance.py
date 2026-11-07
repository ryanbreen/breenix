#!/usr/bin/env python3
"""Score retained checkpoint or gallery captures without booting or approving.

--commit identifies the measured kernel and tools, independently of scorerCommit.
Native console and GPU are distinct companion boots. ARM GPU and BreenGL must
also be at that measured commit. Missing companions make proven false.
Gallery mode also requires pictured-case intent checkpoints and verified
candidates covering both ARM suites. U09 allows one combined boot and set.
Candidates are not approvals.
"""
import argparse
from contextlib import redirect_stdout
import hashlib
import io
import json
from pathlib import Path
import re
import runpy
import subprocess

ROOT = Path(__file__).resolve().parents[1]
REQUIRED = ('console', 'gpu', 'arm-gpu', 'arm-breengel')
RESET_SOURCES = {
    'submit/device-reset-recovers': 'userspace/programs/src/suite_gpu/submit.rs::device_reset_recovers',
    'textures/upload-visibility': 'userspace/programs/src/suite_gpu/textures.rs::upload_visibility',
    'isolation/killable-wait': 'userspace/programs/src/suite_gpu/isolation.rs::killable_wait',
}


def missing_captures(captures):
    return [f'missing {name} capture' for name in REQUIRED if not captures.get(name)]


def capture_identity(path, commit, suite, platform):
    def read(name):
        return (path / name).read_text().strip()
    metadata = path / 'metal-run.json'
    if platform != 'metal' or not metadata.exists():
        metadata = path / 'run.json'
    boot = json.loads(metadata.read_text())
    if (read('commit') != commit or boot.get('commit', commit) != commit):
        raise ValueError('capture commit differs from measured commit')
    if (read('dirty') != '0' or read('exit-status') != '0' or not read('ended')):
        raise ValueError('capture is dirty, unsuccessful or incomplete')
    selection = boot.get('suites') if platform == 'qemu' else None
    if platform == 'qemu' and 'suites' in boot:
        suite_matches = (type(selection) is list and bool(selection)
                         and all(type(s) is str for s in selection)
                         and len(set(selection)) == len(selection) and suite in selection
                         and ('suite' not in boot or boot['suite'] == selection[0]))
    else:
        suite_matches = boot.get('suite') == suite
    if (not suite_matches or boot.get('mode') != 'suite'
            or boot.get('platform') != platform
            or boot.get('arch') != ('x86_64' if platform == 'metal' else 'aarch64')):
        raise ValueError('capture target or suite differs')
    if platform == 'metal':
        if (boot.get('toolsCommit') != commit or boot.get('build') !=
                dict(commit=commit, profile='production', features=[])):
            raise ValueError('native build/tools differ from measured production commit')
        if boot.get('kind') != 'boot' or boot.get('evidenceEligible') is not True or boot.get('session'):
            raise ValueError('native capture is not an eligible production boot')
    return dict(boot, bootedCommit=commit)


def register_capture(seen, boot, name, path):
    """Native companions stay distinct; U09 may cover both ARM suites in one folder."""
    previous = seen.get(boot['id'])
    if previous is not None:
        other_name, other_path = previous
        if (set((name, other_name)) != {'arm-gpu', 'arm-breengel'}
                or path.resolve() != other_path
                or boot.get('platform') != 'qemu'
                or boot.get('suites') != ['gpu', 'breengel']):
            raise ValueError('companion captures must have distinct boot IDs outside combined ARM U09')
    else:
        seen[boot['id']] = (name, path.resolve())


def check_inventory(by_boot):
    inventory = {}
    for boot, rows in by_boot.items():
        for row in rows:
            if row['result'] != 'PASS':
                raise ValueError(f"GT 710 check failed: {boot} {row['id']}")
            if boot in inventory.setdefault(row['id'], []):
                raise ValueError(f"duplicate GT 710 check: {boot} {row['id']}")
            inventory[row['id']].append(boot)
    return inventory


def screen_inventory(suite, serial):
    pictures, declarations = {}, []
    prefix = f'SUITE {suite} SCREEN '
    for line in serial.splitlines():
        if not line.startswith(prefix):
            continue
        fields = line[len(prefix):].split()
        if len(fields) < 3:
            raise ValueError('malformed SCREEN record: ' + line)
        key = (fields[0], fields[1])
        if fields[2].startswith('sha256='):
            pictures[key] = fields[2][len('sha256='):]
        elif fields[2] == 'unobservable':
            declarations.append(key)
    return pictures, declarations


def compare_pictures(native, arm):
    common = native.keys() & arm.keys()
    if not common:
        raise ValueError('no cross-target pictures compared')
    for key in sorted(common):
        if native[key] != arm[key]:
            raise ValueError(f'cross-target digest differs: {key[0]} {key[1]}')
    return len(common)


def reset_faults(serial, names):
    """Attribute by ordered case interval, not by the generic driver's reason.

    adapter.rs::begin_reset uses this fallback when the shared core asks for a
    reset of an otherwise healthy channel. These three cases arm WITHHOLD_ALL,
    observe RESET and judge fresh rendering afterwards. Their PASS and reset-ms
    witnesses are required; the generic fault label alone proves none of that.
    VALUE and native diagnostics have separate producers: either can arrive
    first, but both must belong to the same pending manifest case.
    """
    lines = serial.splitlines()
    result, completed, start = [], 0, 0
    for at, line in enumerate(lines):
        case = re.fullmatch(r'SUITE gpu CASE (\S+) (PASS|FAIL) .*', line)
        if case:
            if completed >= len(names) or case[1] != names[completed]:
                raise ValueError('fault attribution needs manifest-ordered cases')
            completed += 1
            start = at + 1
        if 'GT710_NATIVE_' in line and any(word in line for word in ('REFUSAL', 'REFUSED')):
            raise ValueError('native reset/reprobe refusal: ' + line)
        if not line.startswith('GT710_NATIVE_FAULT'):
            continue
        name = names[completed] if completed < len(names) else ''
        if (name not in RESET_SOURCES or not line.startswith(
                'GT710_NATIVE_FAULT reason=shared core requested reset instance=')
                or any(row['case'] == name for row in result)):
            raise ValueError('unexplained native fault: ' + line)
        end = next((i for i in range(at + 1, len(lines))
                    if lines[i].startswith('SUITE gpu CASE ')), len(lines))
        if end == len(lines) or not lines[end].startswith(f'SUITE gpu CASE {name} PASS '):
            raise ValueError('reset case did not pass: ' + name)
        values = [l for l in lines[start:end] if re.fullmatch(
            rf'SUITE gpu VALUE {re.escape(name)} reset-ms=[1-9][0-9]*ms(?: expect=\d+\.\.\d+ms)?', l)]
        if len(values) != 1:
            raise ValueError('reset lacks its case reset-ms witness: ' + name)
        result.append(dict(case=name, line=at + 1, record=line,
                           source=RESET_SOURCES[name],
                           cause='case arms WITHHOLD_ALL; shared-core watchdog requests device reset',
                           attribution='manifest-ordered case interval and reset-ms/PASS witnesses',
                           witnesses=[l for l in lines[start:at] if l.startswith(f'SUITE gpu WAIT {name} ')]
                                     + values + [lines[end]]))
    if {r['case'] for r in result} != set(RESET_SOURCES):
        raise ValueError('native capture lacks the three intentional reset observations')
    return result


def check_runtime_display(serial, native):
    preferred = native['native_raster'](native['board_inventory'](serial))
    return native['runtime_display'](serial, (preferred['width'], preferred['height']))


def checkpoint_inventory(suite, serial, ends, phase, platform='qemu'):
    expected = list(ends)
    # Phase two's intent frames come from ARM proof captures; native proof mode is phase three (U19).
    if phase == 'gallery' and platform != 'metal':
        expected += [[m[1], 'intent'] for m in re.finditer(
            rf'^SUITE {re.escape(suite)} CASE (\S+) (?:PASS|FAIL) [^\n]*\bsha256=[0-9a-f]{{64}}(?: |$)',
            serial, re.M)]
    return sorted(expected)


def check_gallery_set(path, suite, boot_id, commit, serial):
    importer = runpy.run_path(str(ROOT / 'scripts/screens/approve.py'))
    candidate_id, rows = importer['candidates'](path)
    for key, row in rows.items():
        if key[0] not in ('gpu', 'breengel'):
            raise ValueError('candidates inventory contains an unrelated suite')
        if row['boot_id'] != boot_id or row['commit'] != commit:
            raise ValueError('candidates capture provenance differs')
    rows = {key: row for key, row in rows.items() if key[0] == suite}
    screens = runpy.run_path(str(ROOT / 'scripts/screen-verdict.py'))
    expected = {}
    for line in serial.splitlines():
        if not line.startswith(f'SUITE {suite} SCREEN '):
            continue
        match = screens['RECORD'].fullmatch(line)
        if not match:
            raise ValueError('malformed SCREEN record: ' + line)
        if match['sha']:
            key = (suite, match['case'], match['checkpoint'])
            if key in expected:
                raise ValueError('duplicate SCREEN checkpoint')
            expected[key] = match
    if not expected or rows.keys() != expected.keys():
        raise ValueError('candidates inventory differs from capture SCREEN pictures')
    for key, row in rows.items():
        record = expected[key]
        if any(row[field] != record[source] for field, source in
               [('mode', 'mode'), ('binding', 'binding'), ('sha256', 'sha')]):
            raise ValueError('candidate differs from capture SCREEN record')
    return dict(id=candidate_id, rows=len(rows), boot=boot_id, commit=commit)


def capture_facts(path, suite=None):
    """Keep serial evidence in the report even when boot eligibility fails."""
    serial = (path / 'serial.txt').read_text()
    name = re.escape(suite) if suite else r'\S+'
    return dict(exitStatus=(path / 'exit-status').read_text().strip(),
                serialSHA256=hashlib.sha256((path / 'serial.txt').read_bytes()).hexdigest(),
                done=re.findall(rf'^SUITE {name} DONE .*$', serial, re.M),
                memory=re.findall(rf'^SUITE {name} MEMORY reclaimed=.*$', serial, re.M),
                failedCases=re.findall(rf'^SUITE {name} CASE \S+ FAIL .*$', serial, re.M))


def accept(commit, captures, phase='checkpoints', candidates=None):
    if phase not in ('checkpoints', 'gallery'):
        raise ValueError('unknown acceptance phase')
    candidates = candidates or {}
    native = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))
    suites = runpy.run_path(str(ROOT / 'scripts/suite-verdict.py'))
    screens = runpy.run_path(str(ROOT / 'scripts/screen-verdict.py'))
    table = json.loads((ROOT / 'docs/suites/frames/screens.json').read_text())
    inventories = json.loads((ROOT / 'docs/suites/frames/checkpoints.json').read_text())
    report = dict(measuredCommit=commit, scorerCommit=subprocess.check_output(
        ['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
        phase=phase, proven=False, captures={}, failures=missing_captures(captures))
    if phase == 'gallery':
        report['candidateSets'] = {}
        report['failures'] += [f'missing {suite} candidates' for suite in ('gpu', 'breengel')
                               if not candidates.get(suite)]
    native_rows, seen_boots, pictures_by_capture = {}, {}, {}
    for name in REQUIRED:
        path = captures.get(name)
        if not path:
            continue
        suite = name.removeprefix('arm-')
        platform = 'qemu' if name.startswith('arm-') else 'metal'
        row = report['captures'][name] = dict(path=str(path))
        try:
            row.update(capture_facts(path, suite))
            manifest = json.loads((ROOT / f'docs/suites/{suite}.json').read_text())
            if platform == 'qemu':
                # Record the suite's own result even if the combined boot failed its sibling.
                row['suitePassed'], row['suiteVerdict'] = suites['verdict'](
                    manifest, path / 'serial.txt', [path / 'serial.txt'])
            boot = capture_identity(path, commit, suite, platform)
            register_capture(seen_boots, boot, name, path)
            row.update(id=boot['id'], commit=commit, toolsCommit=boot.get('toolsCommit'))
            if phase == 'gallery' and platform == 'qemu' and boot.get('screenProof') is not True:
                raise ValueError('gallery requires a screen-proof capture')
            if platform == 'metal':
                events = [json.loads(l) for l in (path / 'events.jsonl').read_text().splitlines()]
                if (not any(e.get('event') == 'power-off' and e.get('off') is True for e in events)
                        or not events or events[-1].get('event') != 'closed'
                        or any(e.get('event') in ('fatal', 'console-gap') for e in events)):
                    raise ValueError('native capture lacks clean close and confirmed power-off')
                serial = native['capture_serial'](path)
            else:
                serial = (path / 'serial.txt').read_text()
            row['serialSHA256'] = hashlib.sha256((path / 'serial.txt').read_bytes()).hexdigest()
            names = [c['id'] + '/' + t['id'] for c in manifest['categories'] for t in c['cases']]
            if platform == 'metal' and suite == 'console':
                row['handoffToDoneMS'] = native['resources_console'](serial, boot, events)
            elif platform == 'metal':
                native['resources_row'](serial)
                with redirect_stdout(io.StringIO()):
                    measured = native['resources_suite'](serial, boot, native['STEP_SELECTIONS']['performance'])
                if any(c['result'] != 'PASS' for c in measured['cases']):
                    raise ValueError('phase one requires all 170 GPU cases, not a step subset')
                check_runtime_display(serial, native)
                row['resetFaults'] = reset_faults(serial, names)
            else:
                if not row['suitePassed']:
                    raise ValueError(row['suiteVerdict'])
            row['passedCases'] = len(names)
            row['done'] = re.findall(rf'^SUITE {suite} DONE .*$', serial, re.M)
            row['memory'] = re.findall(rf'^SUITE {suite} MEMORY reclaimed=.*$', serial, re.M)
            if suite != 'console':
                # resources_suite already ran the SCREEN validator on native GPU.
                if platform != 'metal':
                    screens['score'](manifest, serial, table, run=boot)
                actual = [list(m.groups()) for m in re.finditer(
                    rf'^SUITE {suite} SCREEN (\S+) (\S+) ', serial, re.M)]
                expected = checkpoint_inventory(suite, serial, inventories[suite], phase, platform)
                if sorted(actual) != expected:
                    raise ValueError('SCREEN inventory differs: missing=' + str(
                        [v for v in expected if v not in actual]) + ' extra=' + str(
                        [v for v in actual if v not in expected]))
                pictures, declarations = screen_inventory(suite, serial)
                pictures_by_capture[name] = pictures
                row['screenRecords'] = len(actual)
                row['screenPictures'] = len(pictures)
                row['screenDeclarations'] = [l for l in serial.splitlines()
                                             if l.startswith(f'SUITE {suite} SCREEN ') and ' unobservable ' in l]
                row['unobservableRecords'] = len(declarations)
                if phase == 'gallery' and platform == 'qemu' and candidates.get(suite):
                    report['candidateSets'][suite] = check_gallery_set(
                        candidates[suite], suite, boot['id'], commit, serial)
            if platform == 'metal':
                checked = []
                for category in native['capture_categories'](boot):
                    if category == 'core':
                        continue  # Separate diagnostic companion; not a startup check.
                    rows = native['cases'](category, serial)
                    records = dict(boot=boot['id'], commit=commit,
                                   milestone='metal-gt710/' + category, cases=rows)
                    native['score'](json.loads((ROOT / 'docs/suites/gt710.json').read_text()),
                                    json.loads((ROOT / 'docs/efforts/metal.json').read_text()),
                                    records, serial, boot, commit,
                                    json.loads((ROOT / 'docs/boot-path.json').read_text())['fatal'])
                    checked.extend(rows)
                native_rows[boot['id']] = checked
                row['gt710Checks'] = [r['id'] for r in checked]
                if suite == 'console' and 'GT710_NATIVE_FAULT' in serial:
                    raise ValueError('unexplained native fault in console capture')
        except (OSError, ValueError, KeyError, TypeError) as error:
            row['failure'] = str(error)
            report['failures'].append(name + ': ' + str(error))
    if 'gpu' in pictures_by_capture and 'arm-gpu' in pictures_by_capture:
        try:
            report['crossTargetPictures'] = compare_pictures(
                pictures_by_capture['gpu'], pictures_by_capture['arm-gpu'])
        except ValueError as error:
            report['failures'].append(str(error))
    report['gt710Checks'] = check_inventory(native_rows)
    report['distinctGT710Checks'] = len(report['gt710Checks'])
    report['gt710CheckObservations'] = sum(map(len, native_rows.values()))
    step = json.loads((ROOT / 'docs/gui/gt710-step-9-1.json').read_text())
    gpu = json.loads((ROOT / 'docs/suites/gpu.json').read_text())
    gpu_names = {c['id'] + '/' + t['id'] for c in gpu['categories'] for t in c['cases']}
    report['step91'] = dict(suite=step['suite'], selected=len(step['cases']),
                           gpuNameOverlap=sorted(gpu_names.intersection(step['cases'])),
                           appliedToGPU=False)
    report['proven'] = not report['failures']
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--commit', required=True, help='measured runtime commit, not scorer commit')
    for name in REQUIRED:
        parser.add_argument('--' + name, type=Path)
    parser.add_argument('--phase', choices=('checkpoints', 'gallery'), default='checkpoints')
    for suite in ('gpu', 'breengel'):
        parser.add_argument('--' + suite + '-candidates', type=Path)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    if not re.fullmatch('[0-9a-f]{40}', args.commit):
        parser.error('--commit requires a full commit SHA')
    report = accept(args.commit, {n: getattr(args, n.replace('-', '_')) for n in REQUIRED},
                    args.phase, {s: getattr(args, s + '_candidates') for s in ('gpu', 'breengel')})
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print('SCREEN ACCEPTANCE ' + ('PASS' if report['proven'] else 'FAIL'))
    print(f"GT710 distinct={report['distinctGT710Checks']} observations={report['gt710CheckObservations']}")
    for error in report['failures']:
        print(error)
    return 0 if report['proven'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
