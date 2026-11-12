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
import shutil
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
B5_SHA256 = '5f5fdaf5e785a0ca59787e425ccbe038387ec0fad2431cf9be8065969c5f620b'
SUITE_LABELS = ('wm-elapsed', 'supervisor-restart', 'first-composite', 'positive-click',
                'click-release', 'reap', 'latency-median', 'latency-max',
                'present-median', 'present-max', 'frame-interval', 'frame-first', 'frame-last',
                'clipboard-max', 'composites-per-second')
KERNEL_LABELS = ('GPU_WINDOW_IDENTITY_FAIL', 'GPU_SCENE_EPOCH_FAIL')


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
    require(qemu.parents[3].name == '10.0.12-utm-4d73aaf0-b5', 'expected b5 bundle')
    require(sha(qemu) == B5_SHA256, 'QEMU identity mismatch')
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
    labels(kernel, KERNEL_LABELS)
    xkernel = Path(config['x86_kernel'])
    labels(xkernel, KERNEL_LABELS)
    xsuite = programs / 'x86_64-breenix/release/suite-window-manager'
    labels(xsuite, SUITE_LABELS)
    labels(suite, SUITE_LABELS)
    client = programs / 'aarch64-breenix/release/wm-client_test'
    labels(client, ())
    return {name: sha(path) for name, path in (
        ('suite', suite), ('kernel', kernel), ('boot_disk', disk),
        ('x86_kernel', xkernel), ('x86_suite', xsuite), ('client', client))}


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


def sample_load(parent, path):
    # Keep sampling across the gate's exec, until that same process exits.
    ready = False
    with Path(path).open('x', buffering=1) as stream:
        while True:
            tick = time.monotonic()
            try:
                os.kill(parent, 0)
            except ProcessLookupError:
                return
            _, raw = read_load()
            stamp = {'host_unix_ns': time.time_ns(), 'host_monotonic_ns': time.monotonic_ns()}
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
                  'load_samples': str(out / 'load.jsonl')}
        (out / 'artifact-gate.json').write_text(json.dumps(record, indent=2) + '\n')
        append(config, 'prediction',
               'A.3 ONE ARM window-manager acceptance at ' + config['head'] +
               ': normal build artifact/load gate passed; ' + json.dumps(record) +
               '. Predict 81/81 with DONE, MEMORY and READY, ANGLE/Metal accepted, '
               'and 25% headroom on each recovery, click, animation and frame-floor deadline. '
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
