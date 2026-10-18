"""Score the milestones measurable by one boot shape, using that boot's definition."""
import copy
import json
import metal_attended
from pathlib import Path


def boot_shape(disk):
    return {'install': 'install', 'root': 'disk-root', None: 'ramdisk'}[disk]


def measures(milestone, shape):
    # Commits preceding shape metadata describe the original RAM-disk boots.
    return shape in milestone.get('bootShapes', ['ramdisk'])


def attended_definition(definition):
    """Stage lists wait for kernel and USB producers; console-login is the attended Challenge's, which also reads
    a prompt still waiting for its newline."""
    definition = copy.deepcopy(definition)
    for milestone in definition['milestones']:
        if milestone['id'] == 'console-login':
            milestone['stages']['console'] = []
    return definition


def score(directory):
    directory = Path(directory)
    read = lambda name: json.loads((directory / name).read_text())
    run = read('run.json')
    if run.get('kind') == 'console-session':
        raise ValueError('operator console sessions are never scored')
    definition = read('definition/docs/metal/boot-path.json')
    attended = run.get('attended', False)
    if attended:
        definition = attended_definition(definition)
    catalog = {stage['name']: stage for stage in read(
        'definition/tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json')['stages']}
    shape = boot_shape(run.get('disk'))
    lines = (directory / 'serial.txt').read_text().splitlines()
    events = {json.loads(line)['event'] for line in (directory / 'events.jsonl').read_text().splitlines()}
    host = {'console-ready': 'console-ready', 'power-on': 'power-on', 'firmware-text': 'firmware-text',
            'post': 'post-code', 'image-complete': 'image-complete', 'handoff': 'handoff'}
    results = {}
    for milestone in definition['milestones']:
        if milestone.get('stub') or not measures(milestone, shape):
            continue
        if attended and milestone['id'] == 'console-login':
            challenge = metal_attended.Challenge(metal_attended.stages(definition))
            for line in lines:
                challenge.feed(line)
            results[milestone['id']] = challenge.verdict()
            continue
        lists = milestone['stages']
        stages = lists.get(run['mode'], lists.get(definition['gateArch'], []) if milestone.get('kernel') else [])
        if not stages:
            continue
        cursor, reached = 0, 0
        for stage in stages:
            spec = catalog[stage] if isinstance(stage, str) else stage
            if spec.get('input') and spec['input'] != run.get('input'):
                continue
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
    return {'bootShape': shape, 'milestones': results, 'captureComplete': 'console-gap' not in events}
