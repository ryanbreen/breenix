"""Score the milestones measurable by one boot shape, using that boot's definition."""
import json
from pathlib import Path


def boot_shape(disk):
    return {'install': 'install', 'root': 'disk-root', None: 'ramdisk'}[disk]


def measures(milestone, shape):
    return shape in milestone['bootShapes']


def score(directory):
    directory = Path(directory)
    read = lambda name: json.loads((directory / name).read_text())
    definition = read('definition/docs/metal/boot-path.json')
    catalog = {stage['name']: stage for stage in read(
        'definition/tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json')['stages']}
    run = read('run.json')
    shape = boot_shape(run.get('disk'))
    lines = (directory / 'serial.txt').read_text().splitlines()
    events = {json.loads(line)['event'] for line in (directory / 'events.jsonl').read_text().splitlines()}
    host = {'console-ready': 'console-ready', 'power-on': 'power-on', 'firmware-text': 'firmware-text',
            'post': 'post-code', 'image-complete': 'image-complete', 'handoff': 'handoff'}
    results = {}
    for milestone in definition['milestones']:
        if milestone.get('stub') or not measures(milestone, shape):
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
    return {'bootShape': shape, 'milestones': results, 'captureComplete': 'console-gap' not in events}
