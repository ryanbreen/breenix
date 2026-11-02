#!/usr/bin/env python3
"""Prepare and answer the disposable VM persistent installer."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import socket
import sys
import time

spec = importlib.util.spec_from_file_location('vm_slots', Path(__file__).with_name('vm_slots.py'))
vm_slots = importlib.util.module_from_spec(spec)
spec.loader.exec_module(vm_slots)


def digest(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def console_answer(console, data):
    # Match metal-boot.console_send(..., paced=True): Enter terminates the
    # console line, and four-byte bursts leave time for UART input and echo.
    wire = data.removesuffix(b'\n') + b'\r'
    for at in range(0, len(wire), 4):
        console.sendall(wire[at:at + 4])
        time.sleep(.04)


class InstallProtocol:
    def __init__(self, current, previous, next_slot):
        self.answer = f'BREENIX-SLOTS-1 {current} {previous} {next_slot}\n'.encode('ascii')
        self.answered = False
        self.outcome = None

    def feed(self, line, send):
        if self.outcome is not None:
            return
        if line == '[persistent] recovered slot references needed' and not self.answered:
            send(self.answer)
            self.answered = True
        elif line.startswith('[persistent] install failed:'):
            self.outcome = 'failure'
        elif line.startswith('[persistent] install complete:'):
            self.outcome = 'success' if self.answered else 'failure'


def prepare(run):
    run = Path(run)
    candidate = json.loads((run / 'candidate.json').read_text())
    generation = candidate['generation']
    if not isinstance(generation, str) or not re.fullmatch(r'[A-Za-z0-9-]{1,64}', generation) or generation in ('flat', 'empty'):
        raise ValueError('invalid candidate generation')
    for name, key in (('candidate.img', 'imageSha256'), ('candidate-kernel', 'kernelSha256')):
        path = run / name
        if path.is_symlink() or digest(path) != candidate[key]:
            raise ValueError('candidate differs from its seal')
    record = vm_slots.read(run)
    if record is None:
        record = dict(current=None, previous=None, next=None)
    refs = []
    for key in ('current', 'previous', 'next'):
        value = record[key]
        refs.append(vm_slots.entry(value)['userland'] if value is not None else ('flat' if key == 'current' else 'empty'))
    if generation in refs:
        raise ValueError('candidate is already referenced')
    intent = f'BREENIX-BOOT 2\ntarget persistent-install\nuserland {generation}\n'
    if record['current'] is not None:
        intent += f'previous-userland {refs[0]}\n'
    (run / 'boot-intent').write_text(intent, encoding='ascii')
    (run / 'install-references').write_text(' '.join(refs), encoding='ascii')
    shutil.copyfile(run / 'candidate.img', run / 'install-root.img')


def monitor(run):
    run = Path(run)
    protocol = InstallProtocol(*(run / 'install-references').read_text().split())
    deadline = time.monotonic() + 900
    with socket.socket(socket.AF_UNIX) as console:
        console.connect(str(run / 'console.sock'))
        (run / 'install-monitor-ready').touch()
        console.settimeout(1)
        pending = b''
        while protocol.outcome is None and time.monotonic() < deadline:
            try:
                data = console.recv(65536)
            except socket.timeout:
                pid_file = run / 'install-qemu-pid'
                if pid_file.exists():
                    try:
                        os.kill(int(pid_file.read_text()), 0)
                    except ProcessLookupError:
                        break
                continue
            if not data:
                break
            pending += data
            while b'\n' in pending:
                line, pending = pending.split(b'\n', 1)
                protocol.feed(line.decode('utf-8', 'replace').rstrip('\r'),
                              lambda data: console_answer(console, data))
    # QMP quit stops only the QEMU instance owned by this run's locked launch.
    with socket.socket(socket.AF_UNIX) as qmp:
        qmp.settimeout(10)
        qmp.connect(str(run / 'qmp.sock'))
        stream = qmp.makefile('rwb', buffering=0)
        json.loads(stream.readline())
        for command in ('qmp_capabilities', 'quit'):
            stream.write((json.dumps({'execute': command}) + '\n').encode())
            while True:
                message = json.loads(stream.readline())
                if 'error' in message:
                    raise ValueError(str(message['error']))
                if 'return' in message:
                    break
    return protocol.outcome == 'success'


def publish(run):
    run = Path(run)
    candidate = json.loads((run / 'candidate.json').read_text())
    generation = candidate['generation']
    kernel = f'kernel-{generation}'
    shutil.copyfile(run / 'candidate-kernel', run / kernel)
    record = vm_slots.read(run) or dict(current=None, previous=None, next=None)
    record['next'] = dict(kernel=kernel, kernelSha256=candidate['kernelSha256'], userland=generation)
    return record


def publish_record(run):
    run = Path(run)
    candidate = json.loads((run / 'candidate.json').read_text())
    generation = candidate['generation']
    kernel = f'kernel-{generation}'
    path = run / kernel
    if path.is_symlink() or digest(path) != candidate['kernelSha256']:
        raise ValueError('candidate kernel changed before publication')
    record = vm_slots.read(run) or dict(current=None, previous=None, next=None)
    record['next'] = dict(kernel=kernel, kernelSha256=candidate['kernelSha256'], userland=generation)
    return record


if __name__ == '__main__':
    try:
        operation, run = sys.argv[1:]
        if operation == 'prepare':
            prepare(run)
        elif operation == 'monitor':
            sys.exit(0 if monitor(run) else 1)
        elif operation == 'publish':
            record = publish(run)
            print(record['next']['kernel'])
        elif operation == 'commit':
            record = publish_record(run)
            vm_slots.write(run, record)
        else:
            raise ValueError('unknown install operation')
    except (OSError, ValueError, KeyError, json.JSONDecodeError) as error:
        sys.exit(f'VM install: {error}')
