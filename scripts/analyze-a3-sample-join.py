#!/usr/bin/env python3
"""Offline A.3 trace-gap inventory with aggregate sample-window joins."""
import argparse
from collections import Counter, defaultdict
from datetime import datetime
import hashlib
import json
from pathlib import Path
import re


def sample_counts(text):
    result = {}
    thread = None
    stack = []
    roles = {}
    for line in text.split('Total number in stack')[0].splitlines():
        header = re.match(r'\s+(\d+) Thread_(\w+)', line)
        if header:
            thread = header[2]
        elif 'hvf_cpu_thread_fn' in line:
            roles[thread] = 'vcpu'
        elif 'qemu_main_loop' in line:
            roles[thread] = 'loop'
    thread = None

    def finish():
        if not stack:
            return
        node = stack.pop()
        count = node['count'] - node['children']
        assert count >= 0, node
        if stack:
            stack[-1]['children'] += node['count']
        if count == 0:
            return
        path = ' '.join(n['frame'] for n in stack) + ' ' + node['frame']
        role = roles.get(thread)
        if role == 'vcpu':
            if 'bql_lock_impl' in path:
                state = 'bql'
            elif 'mutexwait' in path:
                state = 'mutex-other'
            elif 'wait_for_interrupt' in path or 'qemu_cond_wait' in path:
                state = 'idle'
            elif 'hv_vcpu_run' in path or 'hv_trap' in path:
                state = 'guest'
            else:
                state = 'other'
        elif role == 'loop':
            if 'resolveLink' in path or 'MainLinkLoadEvent' in path:
                state = 'link'
            elif 'compile' in path.lower() or 'Compile' in path:
                state = 'compile'
            elif 'vrend_draw_vbo' in path:
                state = 'draw'
            elif 'transfer' in path.lower():
                state = 'transfer'
            elif 'virgl' in path or 'vrend_' in path:
                state = 'virgl-other'
            else:
                state = 'other'
        else:
            return
        row = result.setdefault(thread, {'role': role, 'total': 0, 'states': Counter(), 'lines': []})
        row['total'] += count
        row['states'][state] += count
        if count:
            row['lines'].append([node['line'], count, state])

    for line_number, line in enumerate(text.splitlines(), 1):
        if line.startswith('Total number in stack'):
            break
        header = re.match(r'\s+(\d+) Thread_(\w+)', line)
        frame = re.match(r'([ +!:|]*)(\d+) (.*)', line)
        if header:
            while stack:
                finish()
            thread = header[2]
        elif frame and thread is not None:
            depth = len(frame[1])
            while stack and stack[-1]['depth'] >= depth:
                finish()
            stack.append({'depth': depth, 'count': int(frame[2]), 'children': 0,
                          'frame': frame[3], 'line': line_number})
    while stack:
        finish()
    return result


def timestamp(value):
    seconds, fraction = value.split('.')
    return int(seconds) * 10**9 + int(fraction.ljust(9, '0'))


def events(lines):
    for number, line in enumerate(lines, 1):
        match = re.match(r'\d+@(\d+\.\d+):(\w+)\s*(.*)', line)
        if match:
            yield {'ns': timestamp(match[1]), 'event': match[2], 'detail': match[3], 'line': number}


def trace_gaps(lines, begin_ns, end_ns):
    previous = gpu = None
    result = []
    # Host threads can emit records slightly out of timestamp order.
    for event in sorted(events(lines), key=lambda e: (e['ns'], e['line'])):
        if previous and event['ns'] - previous['ns'] > 100_000_000:
            if previous['ns'] < end_ns and event['ns'] > begin_ns:
                result.append({'begin_ns': previous['ns'], 'end_ns': event['ns'],
                               'duration_ns': event['ns'] - previous['ns'],
                               'before': previous, 'after': event,
                               'gpu': gpu, 'gpu_line': gpu['line'] if gpu else None})
        if event['event'].startswith('virtio_gpu_cmd_'):
            gpu = event
        previous = event
    return result


def covering_samples(gap, windows):
    return [w['id'] for w in windows
            if w['begin_ns'] <= gap['begin_ns'] and w['end_ns'] >= gap['end_ns']]


def fields(line):
    return dict(re.findall(r'([\w]+)=([^\s]+)', line))


def analyze(capture):
    serial = (capture / 'serial.txt').read_bytes().decode().split('\n')
    trace = (capture / 'qemu-trace.log').read_text().splitlines()
    host = list(events(trace))
    requests, budgets, replacements, contexts = {}, {}, {}, defaultdict(dict)
    creators, subjects, gpu_commands = {}, {}, {}
    identities = defaultdict(dict)
    evidence = []
    for number, line in enumerate(serial, 1):
        if 'VALUE wm-disk-request ' in line:
            f = fields(line)
            requests[int(f['id'])] = (f, number)
        if 'VALUE wm-case-budget ' in line:
            f = fields(line)
            if int(f.get('logic_begin_ns', '0')):
                budgets[f['case']] = (f, number)
        if 'VALUE wm-account-window ' in line:
            f = fields(line)
            if f['id'] in ('65', '67') and f['complete'] == '1':
                replacements[f['id']] = (f, number)
        if 'VALUE wm-gpu-core-command ' in line:
            f = fields(line)
            gpu_commands[f['seq']] = (f, number)
            if f['context'] != '0':
                contexts[int(f['context'])].setdefault((f['pid'], f['tid']), number)
                if f['type'] == '512':
                    creators.setdefault(int(f['context']), {'pid': f['pid'], 'tid': f['tid'], 'line': number})
        if 'VALUE wm-wait-budget ' in line and 'wait=positive click delivery to target client' in line:
            f = fields(line)
            if f.get('end_ns', '0') != '0' and f['tid'] in ('461', '476'):
                subjects.setdefault('click-wait-' + f['tid'], (int(f['begin_ns']), int(f['end_ns']), number))
        if 'VALUE wm-frame-endpoints client=120 ' in line:
            f = fields(line)
            subjects['frame-floor-measurement'] = (int(f['begin_ns']), int(f['end_ns']), number)
        if 'VALUE wm-account-identity ' in line:
            f = fields(line)
            identities[f['pid']].setdefault(f['name'], number)
        if re.search(r'(CASE window-manager (focus/(pair-click|keys-to-focused)|performance/frame-floor)|'
                     r'SUITE window-manager CASE (focus/(pair-click|keys-to-focused)|performance/frame-floor)|'
                     r'VALUE wm-frame-endpoints )', line):
            evidence.append({'line': number, 'text': line.rstrip()})

    # A block request is bracketed by guest submit/used and host handle/complete.
    pending, handles = {}, defaultdict(list)
    for event in host:
        m = re.match(r'virtio_blk_handle_(read|write)$', event['event'])
        if m:
            f = re.search(r'req (\S+) sector (\d+) nsectors (\d+)', event['detail'])
            if f:
                pending[f[1]] = (event, (m[1], int(f[2]), int(f[3]) * 512))
        elif event['event'] == 'virtio_blk_req_complete':
            f = re.search(r'req (\S+) status 0', event['detail'])
            if f and f[1] in pending:
                start, signature = pending.pop(f[1])
                handles[signature].append((start, event))
    first, _ = requests[1]
    origin = handles[('read', int(first['sector']), int(first['bytes']))][0][0]['ns'] - int(first['submit_ns'])
    anchors = []
    for ident, (f, line) in requests.items():
        submit, irq = int(f['submit_ns']), int(f['irq_ns'])
        if not submit or irq <= submit or irq - submit > 1_000_000:
            continue
        candidates = handles.get(('read' if f['op'] == '0' else 'write', int(f['sector']), int(f['bytes'])), [])
        if not candidates:
            continue
        start, done = min(candidates, key=lambda pair: abs(pair[0]['ns'] - submit - origin))
        if abs(start['ns'] - submit - origin) > 50_000_000:
            continue
        low, high = done['ns'] - irq, start['ns'] - submit
        if low <= high:
            anchors.append({'guest_ns': submit, 'low': low, 'high': high,
                            'kind': 'disk', 'request': ident, 'serial_line': line,
                            'trace_lines': [start['line'], done['line']]})
    # Wire fence IDs join directly, including during disk-free performance cases.
    control, response = {}, {}
    for event in host:
        if event['event'] in ('virtio_gpu_fence_ctrl', 'virtio_gpu_fence_resp'):
            fence = int(re.search(r'fence (0x[0-9a-f]+)', event['detail'])[1], 16)
            (control if event['event'].endswith('ctrl') else response)[fence] = event
    for seq, (f, line) in gpu_commands.items():
        submit, end, fence = int(f['submit_ns']), int(f['fence_return_ns']), int(f['fence'])
        if not (submit < end <= submit + 1_000_000 and fence in control and fence in response):
            continue
        start, done = control[fence], response[fence]
        low, high = done['ns'] - end, start['ns'] - submit
        if low <= high:
            anchors.append({'guest_ns': submit, 'low': low, 'high': high,
                            'kind': 'gpu-fence', 'seq': seq, 'fence': fence, 'serial_line': line,
                            'trace_lines': [start['line'], done['line']]})
    command_fences = {}
    last_command = None
    for event in host:
        if event['event'].startswith('virtio_gpu_cmd_'):
            last_command = event
        elif event['event'] == 'virtio_gpu_fence_ctrl' and last_command:
            command_fences[last_command['line']] = event
            last_command = None
    guest_fences = {int(f['fence']): dict(f, serial_line=n)
                    for f, n in gpu_commands.values() if int(f['fence'])}
    anchors.sort(key=lambda a: a['guest_ns'])

    def clock(guest):
        a = min(anchors, key=lambda a: abs(a['guest_ns'] - guest))
        return guest + (a['low'] + a['high']) // 2, a

    sample_dir = capture / 'qemu-samples'
    records = [json.loads(line) for line in (sample_dir / 'samples.jsonl').read_text().splitlines()]
    starts = {row['sample_pid']: row for row in records if row['event'] == 'sample-start'}
    samples = []
    for number, stop in enumerate(records, 1):
        if stop['event'] != 'sample-stop':
            continue
        start = starts[stop['sample_pid']]
        path = sample_dir / Path(start['file']).name
        raw = path.read_text()
        date = re.search(r'^Date/Time:\s+(.*)$', raw, re.M)[1]
        # The report time excludes launch-to-attach delay. End is a process-lifetime
        # upper bound, including symbolication, not a timestamp for the last stack.
        instant = datetime.strptime(date, '%Y-%m-%d %H:%M:%S.%f %z')
        attach = int(instant.timestamp()) * 10**9 + instant.microsecond * 1000
        samples.append({'id': path.name, 'case': start['case'],
                        'begin_ns': attach, 'end_ns': stop['host_end_unix_ns'],
                        'nominal_end_ns': min(attach + 3 * 10**9, stop['host_end_unix_ns']),
                        'record_line': number, 'returncode': stop['returncode'],
                        'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
                        'threads': sample_counts(raw)})
    windows = []
    for ident, (f, line) in sorted(replacements.items()):
        windows.append((f'window-{ident}', int(f['begin_ns']), int(f['end_ns']), line))
    for case in ('focus/pair-click', 'focus/keys-to-focused', 'performance/frame-floor'):
        f, line = budgets[case]
        windows.append((case, int(f['logic_begin_ns']), int(f['logic_end_ns']), line))
    windows.extend((name, *row) for name, row in subjects.items())
    joined = []
    creates = {}
    for event in host:
        if event['event'] == 'virtio_gpu_cmd_ctx_create':
            creates[int(re.search(r'ctx (0x[0-9a-f]+)', event['detail'])[1], 16)] = event
    for name, begin, end, line in windows:
        host_begin, left = clock(begin)
        host_end, right = clock(end)
        gaps = trace_gaps(trace, host_begin, host_end)
        for gap in gaps:
            gap['samples'] = covering_samples(gap, samples)
            gap['nominal_samples'] = covering_samples(gap, [dict(s, end_ns=s['nominal_end_ns']) for s in samples])
            gap['overlap_samples'] = [s['id'] for s in samples
                                      if s['begin_ns'] < gap['end_ns'] and s['end_ns'] > gap['begin_ns']]
            cmd = gap['gpu']
            fence_event = command_fences.get(cmd['line']) if cmd else None
            fence = int(re.search(r'fence (0x[0-9a-f]+)', fence_event['detail'])[1], 16) if fence_event else None
            gap['command_fence'] = fence_event
            gap['command_response'] = response.get(fence)
            gap['guest_command'] = guest_fences.get(fence)
            ctx = re.search(r'ctx (0x[0-9a-f]+)', cmd['detail']) if cmd else None
            gap['context'] = int(ctx[1], 16) if ctx else None
            gap['context_create'] = creates.get(gap['context'])
            gap['creator'] = creators.get(gap['context'])
            gap['origins'] = [{'pid': pid, 'tid': tid, 'line': n,
                               'names': identities[pid]} for (pid, tid), n in contexts[gap['context']].items()]
        joined.append({'name': name, 'guest_begin_ns': begin, 'guest_end_ns': end,
                       'serial_line': line, 'host_begin_ns': host_begin, 'host_end_ns': host_end,
                       'clock_anchors': [left, right], 'gaps': gaps})
    receipts = []
    with (capture / 'serial-host-clock.jsonl').open() as sidecar:
        for number, line in enumerate(sidecar, 1):
            if 'CASE window-manager ' not in line and 'CASE focus/' not in line and 'CASE performance/frame-floor' not in line:
                continue
            row = json.loads(line)
            if any(case in row['line'] for case in ('focus/pair-click', 'focus/keys-to-focused', 'performance/frame-floor')):
                receipts.append(dict(row, sidecar_line=number))
    selected = {name for w in joined for gap in w['gaps'] for name in gap['overlap_samples']}
    for w in joined:
        for gap in w['gaps']:
            for event in (gap['before'], gap['after']):
                if not event['event'].startswith('virtio_gpu_'):
                    event.pop('detail', None)
    return {'boot': capture.name, 'hashes': {n: hashlib.sha256((capture/n).read_bytes()).hexdigest()
             for n in ('serial.txt', 'qemu-trace.log', 'serial-host-clock.jsonl')},
            'samples': [s for s in samples if s['id'] in selected], 'windows': joined,
            'serial_evidence': evidence, 'receipts': receipts}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('capture', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    args.output.write_text(json.dumps(analyze(args.capture), indent=2) + '\n')
