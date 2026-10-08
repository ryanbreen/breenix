#!/usr/bin/env python3
"""Run an ordered suite boot with bounded, private evidence at every boundary."""
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import signal
import socket
import subprocess
import sys
import time

spec = importlib.util.spec_from_file_location('suite_verdict', Path(__file__).with_name('suite-verdict.py'))
scorer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(scorer)


class QMP:
    def __init__(self, path):
        self.socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.socket.settimeout(10)
        self.socket.connect(path)
        self.stream = self.socket.makefile('rw')
        json.loads(self.stream.readline())
        self.command('qmp_capabilities')

    def command(self, name, arguments=None):
        self.stream.write(json.dumps(dict(execute=name, arguments=arguments or {})) + '\n')
        self.stream.flush()
        while True:
            reply = json.loads(self.stream.readline())
            if 'error' in reply:
                raise RuntimeError(f'QMP {name}: {reply["error"]}')
            if 'return' in reply:
                return reply['return']

    def close(self):
        self.stream.close()
        self.socket.close()


def record(repo, out, suite, user, kernel, disk, started, ended):
    directory = out / ('suite-' + suite)
    directory.mkdir(exist_ok=True)
    (directory / 'serial_user.log').write_bytes(user)
    (directory / 'serial_kernel.log').write_bytes(kernel)
    manifest = json.loads((repo / 'docs/suites' / (suite + '.json')).read_text())
    has_start = re.search(rf'^SUITE {suite} START cases=\d+\r?$', user.decode(errors='replace'), re.M)
    if not has_start:
        verdict, reason = 'NOT-RUN', 'suite never printed START'
    else:
        ok, reason = scorer.verdict(manifest, directory / 'serial_user.log', [directory / 'serial_user.log', directory / 'serial_kernel.log'])
        if ok:
            ok, disk_reason = scorer.disk_verdict(manifest.get('diskChecks', []), disk)
            if disk_reason:
                reason += '; ' + disk_reason
        verdict = 'PASS' if ok else 'FAIL'
    result = dict(suite=suite, verdict=verdict, reason=reason, started=started, ended=ended)
    (directory / 'result.json').write_text(json.dumps(result) + '\n')
    print(f'  Suite {suite}: {verdict}: {reason}', flush=True)
    return result


def main():
    repo, out, suites, timeout, command = Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3].split(','), float(sys.argv[4]), sys.argv[5:]
    user_path, kernel_path = out / 'serial_user.log', out / 'serial_kernel.log'
    offsets = [0, 0]
    started = time.time()
    deadline = time.monotonic() + timeout
    current = 0
    results = []
    qmp = None
    child = subprocess.Popen(command)
    def interrupt(number, frame):
        raise InterruptedError(f'suite boot interrupted by signal {number}')
    for number in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        signal.signal(number, interrupt)
    def read():
        return [p.read_bytes() if p.exists() else b'' for p in (user_path, kernel_path)]
    try:
        while child.poll() is None and current < len(suites):
            data = read()
            suite = suites[current]
            user = data[0][offsets[0]:].decode(errors='replace').replace('\r', '')
            done = re.search(rf'^SUITE {suite} DONE passed=\d+ failed=\d+ skipped=\d+ total=\d+$', user, re.M)
            ready = f'SUITE_SEQUENCE READY {suite}' in user.splitlines()
            # Every suite draws its final panel before READY and then waits for
            # keyboard acknowledgement. No subsequent disk writes can race us.
            if done and ready:
                qmp = qmp or QMP(os.environ['BREENIX_QMP_SOCKET'])
                qmp.command('stop')
                data = read()
                directory = out / ('suite-' + suite)
                directory.mkdir(exist_ok=True)
                qmp.command('screendump', {'filename': str(directory / 'screen.png'), 'format': 'png'})
                manifest = json.loads((repo / 'docs/suites' / (suite + '.json')).read_text())
                disk = None
                if manifest.get('diskChecks'):
                    disk = directory / 'disk.img'
                    shutil.copyfile(repo / 'target/ext2.img', disk)
                result = record(repo, out, suite, data[0][offsets[0]:], data[1][offsets[1]:], disk, started, time.time())
                results.append(result)
                if disk:
                    disk.unlink()  # raw checks already recorded; avoid multi-GB evidence
                offsets = list(map(len, data))
                current += 1
                if current == len(suites):
                    break
                qmp.command('cont')
                qmp.command('send-key', {'keys': [{'type': 'qcode', 'data': 'ret'}]})
                started = time.time()
                deadline = time.monotonic() + timeout
            elif 'SUITE_SEQUENCE FAIL' in user or time.monotonic() >= deadline:
                print(f'[gate] suite {suite} stopped: sequence failure or {timeout:g}s deadline', flush=True)
                break
            time.sleep(.05)
    finally:
        if child.poll() is None:
            child.terminate()
            try:
                child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()
        if qmp:
            qmp.close()
        data = read()
        for index in range(current, len(suites)):
            # Only the interrupted current suite owns the remaining output.
            user, kernel = (data[0][offsets[0]:], data[1][offsets[1]:]) if index == current else (b'', b'')
            results.append(record(repo, out, suites[index], user, kernel, None, started, time.time()))
        (out / 'suite-results.json').write_text(json.dumps(results) + '\n')
    return 0 if all(row['verdict'] == 'PASS' for row in results) else 1


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, ValueError, RuntimeError, InterruptedError) as error:
        print(f'GATE: FAIL ({error})', flush=True)
        sys.exit(1)
