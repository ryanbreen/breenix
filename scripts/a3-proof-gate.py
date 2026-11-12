#!/usr/bin/env python3
"""A.3 pre-exec checks, installed as BREENIX_QEMU_PRE_EXEC_GATE.

BREENIX_A3_PROOF_CONFIG names a local JSON file with head (full Git SHA),
ledger, coordinator_line (last reviewed coordinator line), output_dir,
launch_log and x86_kernel paths. Paths are absolute; site values stay local.
The launcher must use the normal build path and capture its output in launch_log.

--dry-run checks the launch chain, signature, binary identity and QEMU options
only. It neither checks guest artifacts nor samples load, writes a prediction,
or execs QEMU. The full path requires the config and performs those checks.
"""

import hashlib
import json
import os
from pathlib import Path
import re
import runpy
import shutil
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
B4_SHA256 = 'e52ce6461a0bad90d7105991d695fd9288485256a568ee0e15ca9f43f3d0c09b'
SUITE_LABELS = (
    'observer-wait', 'reap-wait', 'frame-endpoints', 'readback-buffer', 'VALUE wm-',
    'WM_STALL', 'arm ', 'end ', 'click-request', 'click-observed',
    '/proc/breenix/window-stall', '/proc/breenix/window-stall-sample',
    'parent-fork', 'case-begin', 'logic-begin', 'logic-end', 'report-begin',
    'report-end', 'parent-outcome', 'watchdog-before-kill', 'diagnostic-begin',
    'diagnostic-end', 'WM_STALL budget ',
    'input-reset', 'input-begin', 'input-end', 'input-close',
)
SHARED_SUITE_LABELS = (
    'VALUE wm-', '/proc/breenix/window-stall', '/proc/breenix/window-stall-sample',
    'parent-fork', 'case-begin', 'logic-begin', 'logic-end', 'report-begin',
    'report-end', 'parent-outcome', 'watchdog-before-kill', 'diagnostic-begin',
    'diagnostic-end', 'WM_STALL budget ',
    'input-reset', 'input-begin', 'input-end', 'input-close',
)
KERNEL_LABELS = (
    'retained-writer-fallback', 'click-delivery', 'stall-buffer', 'stall-snapshot',
    'stall-thread', 'stall-arm-overflow', 'account-buffer', 'account-window',
    'account-thread', 'account-wait', 'account-runqueue', 'account-identity',
    'disk-request', 'file-syscall', 'account-export-truncated', 'disk-irq-entry',
    'account-join', 'account-wake', 'case-budget', 'wait-budget',
    'thread-exit-futex', 'gpu-events', 'gpu-reaper',
    'acquire-phase', 'account-input frozen=', 'account-interval',
)
ARM_KERNEL_LABELS = (
    'gpu-command-ring retained=', 'gpu-command seq=', 'gpu-core-command-ring retained=',
    'gpu-core-command seq=', 'gpu-core-doorbell seq=', 'gpu-core-case-window',
)


def require(condition, message):
    if not condition:
        raise ValueError(message)


def sha(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def output(argv):
    return subprocess.check_output(argv, cwd=ROOT, text=True).strip()


def option(argv, name):
    slots = [i for i, value in enumerate(argv) if value == name]
    require(len(slots) == 1 and slots[0] + 1 < len(argv), f'expected one {name} value')
    return argv[slots[0] + 1]


def check_chain(argv):
    require(len(argv) > 3, 'missing wrapper/QEMU command')
    wrapper = ROOT / 'scripts/qemu-ui-launch.sh'
    require(Path(argv[0]).resolve() == wrapper, 'unexpected shell launcher')
    exported = os.environ.get('BREENIX_QEMU_UI_LAUNCHER', str(wrapper))
    require(Path(exported).resolve() == wrapper, 'unexpected exported shell launcher')
    qemu = Path(output([str(ROOT / 'scripts/build-qemu-gl.sh'), '--print-path']))
    require(Path(argv[1]).resolve() == qemu.resolve(), 'unexpected QEMU executable at argv 1')
    require(qemu.parents[3].name == '10.0.12-utm-4d73aaf0-b4', 'expected b4 bundle')
    require(sha(qemu) == B4_SHA256, 'QEMU identity mismatch')
    subprocess.run(['codesign', '--verify', '--deep', '--strict', str(qemu.parents[2])], check=True)

    # The wrapper prints the helper it will exec; its cache key is the hash
    # of these source files (qemu-ui-launch.sh HASH/DIR).
    native = Path(output([str(wrapper)]))
    sources = ('qemu-ui-launch.c', 'qemu-ui-threads.c', 'qemu-unclamp.py')
    key = hashlib.sha256(b''.join((ROOT / 'scripts' / name).read_bytes()
                                  for name in sources)).hexdigest()[:16]
    cache = Path(os.environ.get('BREENIX_QEMU_APP_DIR',
                                str(Path.home() / 'Library/Caches/breenix')))
    expected_native = cache / ('qemu-ui-' + key) / 'qemu-ui-launch'
    require(native.resolve() == expected_native.resolve(), 'unexpected cached native launcher')
    require(os.access(native, os.X_OK), 'native launcher is not executable')
    with native.open('rb') as stream:
        require(stream.read(4) == b'\xcf\xfa\xed\xfe', 'native launcher is not Mach-O')
    library = native.with_name('qemu-ui-threads.dylib')
    unclamp = native.with_name('qemu-unclamp.py')
    require(unclamp.read_bytes() == (ROOT / 'scripts/qemu-unclamp.py').read_bytes(),
            'cached unclamping helper differs')

    require(argv[2:4] == ['-accel', 'hvf'], 'expected HVF accelerator')
    require(option(argv, '-display') == 'none', 'expected headless display')
    require('gl=es' in option(argv, '-spice').split(','), 'expected SPICE GL ES')
    require(not any(arg in ('-s', '-S', '-gdb') or arg.startswith('-gdb=') for arg in argv),
            'debugger options are forbidden')
    require('--no-build' not in argv, 'expected normal build')
    require('timestamp=on' in option(argv, '-msg').split(','), 'QEMU timestamps absent')
    traces = option(argv, '-d').split(',')
    require('trace:virtio_blk_req_complete' in traces and 'trace:virtio_gpu_*' in traces,
            'QEMU trace not enabled')
    option(argv, '-D')
    return {'native_launcher': str(native), 'qemu': str(qemu),
            'hashes': {'shell_launcher': sha(wrapper), 'native_launcher': sha(native),
                       'native_library': sha(library), 'unclamp': sha(unclamp), 'qemu': sha(qemu)}}


def labels(path, expected):
    data = Path(path).read_bytes()
    require(bool(data), f'empty artifact: {path}')
    for label in expected:
        require(label.encode() in data, f'missing artifact label {label}: {path}')
    return data


def check_artifacts(argv, config):
    drives = [arg for arg in argv if arg.startswith('if=none,id=ext2,')]
    require(len(drives) == 1, 'expected one ext2 disk')
    files = [part[5:] for part in drives[0].split(',') if part.startswith('file=')]
    require(len(files) == 1, 'expected one ext2 disk file')
    disk = Path(files[0])
    programs = ROOT / 'userspace/programs/target'
    suite = programs / 'aarch64-breenix/release/suite-window-manager'
    built = labels(suite, SUITE_LABELS)
    candidates = (shutil.which('debugfs'), '/opt/homebrew/opt/e2fsprogs/sbin/debugfs',
                  '/usr/local/opt/e2fsprogs/sbin/debugfs')
    debugfs = next((p for p in candidates if p and Path(p).is_file()), None)
    require(debugfs is not None, 'debugfs is required')
    staged = subprocess.run([debugfs, '-R', 'cat /sbin/suite-window-manager', str(disk)],
                            capture_output=True, check=True).stdout
    require(staged == built, 'boot disk suite differs')
    kernel = Path(option(argv, '-kernel'))
    labels(kernel, KERNEL_LABELS + ARM_KERNEL_LABELS)
    xkernel = Path(config['x86_kernel'])
    # The command journal is ARM-only (kernel/src/window/mod.rs).
    labels(xkernel, KERNEL_LABELS)
    xsuite = programs / 'x86_64-breenix/release/suite-window-manager'
    labels(xsuite, SHARED_SUITE_LABELS)
    labels(suite, SHARED_SUITE_LABELS)
    client = programs / 'aarch64-breenix/release/wm-client_test'
    labels(client, ('WM_STALL click-handled',))
    return {name: sha(path) for name, path in (
        ('suite', suite), ('kernel', kernel), ('boot_disk', disk),
        ('x86_kernel', xkernel), ('x86_suite', xsuite), ('client', client))}


def check_observers(argv):
    # The outer watcher owns sample processes and reads the actual QEMU pidfile.
    serial = os.environ.get('BREENIX_QEMU_CASE_SAMPLE_SERIAL', '')
    require(bool(serial), 'window-manager case sampler is not wired by watch-qemu.py')
    require(Path(option(argv, '-D')).resolve() == Path(serial + '.qemu-trace.log').resolve(),
            'case sampler serial differs from QEMU trace')
    require(Path(option(argv, '-pidfile')).resolve() == Path(serial + '.qemu.pid').resolve(),
            'case sampler pidfile differs from QEMU pidfile')
    sampler = ROOT / 'scripts/qemu-case-sample.py'
    observer = runpy.run_path(str(sampler))['CaseSamples']
    require(observer.continuous('performance/latency-under-animation'),
            'performance samples must cover the whole case')
    return {'case_samples': serial + '.qemu-samples', 'case_sampler_sha256': sha(sampler),
            'watcher_sha256': sha(ROOT / 'scripts/watch-qemu.py'),
            'host_samples': 'all QEMU threads, including renderer worker'}


def check_head(config):
    require(re.fullmatch('[0-9a-f]{40}', config['head']) is not None, 'expected full head SHA')
    require(output(['git', 'rev-parse', 'HEAD']) == config['head'], 'unexpected head')
    require(not output(['git', 'status', '--porcelain']), 'working tree is not clean')


def check_warnings(config):
    log = Path(config['launch_log']).read_text(errors='replace')
    require(bool(log.strip()), 'build log is empty')
    warnings = [line for line in log.splitlines()
                if line.startswith(('warning:', 'error:', 'warning[', 'error['))
                and not line.startswith('warning: the following packages contain code that will be rejected '
                                        'by a future version of Rust: core ')]
    require(not warnings, 'build warnings/errors: ' + repr(warnings))


def check_coordinator(config):
    records = [json.loads(line) for line in Path(config['ledger']).read_text().splitlines()]
    latest = [(i, record) for i, record in enumerate(records, 1) if record.get('role') == 'coordinator'][-1]
    require(latest[0] == config['coordinator_line'],
            f'new coordinator line requires inspection: {latest[0]}')


def append(config, kind, text):
    record = {'role': 'agent', 'type': kind, 'text': text, 'host_unix_ns': time.time_ns()}
    with Path(config['ledger']).open('a') as stream:
        stream.write(json.dumps(record) + '\n')
        stream.flush()
        os.fsync(stream.fileno())


def read_load():
    raw = output(['/usr/sbin/sysctl', '-n', 'vm.loadavg'])
    return float(raw.split()[1]), raw


def cpu_top():
    raw = output(['/bin/ps', '-axo', 'pid=,pcpu=,time=,comm='])
    processes = []
    for line in raw.splitlines():
        pid, cpu, cpu_time, command = line.strip().split(None, 3)
        processes.append({'pid': int(pid), 'cpu_percent': float(cpu),
                          'cpu_time': cpu_time, 'command': command})
    return sorted(processes, key=lambda p: (-p['cpu_percent'], p['pid']))[:10]


def sample_load(parent, path):
    # Keep sampling across the gate's exec, until that same process exits.
    ready = False
    with Path(path).open('x', buffering=1) as stream, \
            Path(path).with_name('cpu-top.jsonl').open('x', buffering=1) as top:
        while True:
            tick = time.monotonic()
            try:
                os.kill(parent, 0)
            except ProcessLookupError:
                return
            _, raw = read_load()
            stamp = {'host_unix_ns': time.time_ns(), 'host_monotonic_ns': time.monotonic_ns()}
            top.write(json.dumps({**stamp, 'processes': cpu_top()}) + '\n')
            stream.write(json.dumps({**stamp, 'vm.loadavg': raw}) + '\n')
            # Readiness is emitted only after the first sample is on disk.
            if not ready:
                print('ready', flush=True)
                ready = True
            time.sleep(max(0, 2 - (time.monotonic() - tick)))


def start_sampler(out):
    sampler = subprocess.Popen([sys.executable, str(Path(__file__).resolve()), '--sample-load',
                                str(os.getpid()), str(out / 'load.jsonl')],
                               stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                               stderr=subprocess.DEVNULL, text=True, start_new_session=True)
    ready = sampler.stdout.readline()
    sampler.stdout.close()
    if ready != 'ready\n':
        sampler.wait()
        raise ValueError('load sampler failed before its first sample')
    return sampler


def wait_for_load(sampler):
    started = time.monotonic()
    while True:
        require(sampler.poll() is None, 'load sampler stopped before exec')
        load, _ = read_load()
        waited = time.monotonic() - started
        if load < 9 or waited >= 1800:
            return {'load1': load, 'waited_s': waited, 'load_timeout': load >= 9}
        time.sleep(2)


def run(argv, config):
    chain = check_chain(argv)
    observers = check_observers(argv)
    hashes = {**chain['hashes'], **check_artifacts(argv, config), 'head': config['head']}
    check_warnings(config)
    check_head(config)
    check_coordinator(config)
    out = Path(config['output_dir'])
    out.mkdir(parents=True, exist_ok=True)
    qemu = Path(chain['qemu'])
    (out / 'PINS').write_bytes((qemu.parents[3] / 'PINS').read_bytes())
    sampler = start_sampler(out)
    try:
        load = wait_for_load(sampler)
        check_head(config)
        check_coordinator(config)
        record = {'hashes': hashes, **load, 'argv': argv, 'time_ns': time.time_ns(),
                  'sampler_pid': sampler.pid, 'native_launcher': chain['native_launcher'],
                  'observers': observers, 'host_cpu_top': str(out / 'cpu-top.jsonl')}
        (out / 'artifact-gate.json').write_text(json.dumps(record, indent=2) + '\n')
        append(config, 'prediction',
               'A.3 ONE instrumented ARM window-manager measurement at ' + config['head'] +
               ': normal build artifact/load gate passed; ' + json.dumps(record) +
               '. Observation question: which named acquire subphase/dependency consumes '
               'the slow input, and is the client blocked or runnable/off-CPU? '
               'Prediction: a host completion dependency accounts for the excess if its '
               'wait overlaps a stalled renderer worker; guest runnable time instead '
               'supports scheduling delay. Neither is established by load alone. '
               'The first input sample over 75 ms freezes phase and CPU/wait intervals. '
               'Read-only macOS samples cover all QEMU threads throughout performance cases; '
               'two-second load and CPU top-10 records identify competing host processes. '
               'This measurement makes no acceptance claim; later proof still requires '
               '25% headroom on each recovery, click, animation and frame-floor deadline. '
               'load_timeout=true means the 30-minute wait expired; proceed as line 182 requires.')
        require(sampler.poll() is None, 'load sampler stopped before exec')
        print('A.3 pre-exec artifact/load gate PASS ' + json.dumps(record), flush=True)
        os.execvp(argv[0], argv)
    except BaseException:
        # This run started this sampler; no other process is stopped here.
        sampler.terminate()
        sampler.wait()
        raise


def main():
    argv = sys.argv[1:]
    if argv[:1] == ['--sample-load']:
        sample_load(int(argv[1]), argv[2])
        return
    if argv[:1] == ['--dry-run']:
        print(json.dumps({**check_chain(argv[1:]), 'argv': argv[1:]}))
        return
    config = json.loads(Path(os.environ['BREENIX_A3_PROOF_CONFIG']).read_text())
    for key in ('ledger', 'output_dir', 'launch_log', 'x86_kernel'):
        require(Path(config[key]).is_absolute(), f'{key} must be absolute')
    try:
        run(argv, config)
    except BaseException as error:
        append(config, 'blocker', 'A.3 pre-exec gate STOP before QEMU: ' + repr(error))
        raise


if __name__ == '__main__':
    main()
