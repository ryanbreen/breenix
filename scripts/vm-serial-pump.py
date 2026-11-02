#!/usr/bin/env python3
"""Capture a persistent QEMU run's serial stream and expose its console."""
import datetime
import hashlib
import hmac
import re
import json
import selectors
import signal
import socket
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent / 'metal'))
from metal_lib import SerialCleaner


def main(run, whole_path):
    run = Path(run)
    build_path = run / 'build.json'
    build = json.loads(build_path.read_text()) if build_path.exists() else {}
    verified_path = run / 'verified-build.json'
    verified = json.loads(verified_path.read_text()) if verified_path.exists() else {}
    mode_path = run / 'mode'
    tools_path = run / 'tools-commit'
    record = {'mode': 'persistent', 'bootMode': mode_path.read_text().strip() if mode_path.exists() else '',
              'target': 'qemu', 'bootedCommit': build.get('commit'),
              'toolsCommit': tools_path.read_text().strip() if tools_path.exists() else '',
              'evidenceEligible': bool(verified.get('evidenceEligible') is True and verified.get('commit') == build.get('commit') and verified.get('artifacts') == build.get('artifacts'))}
    if record['evidenceEligible']:
        record['artifacts'] = verified['artifacts']
    # Freeze the scoring definitions at the image's recorded build commit,
    # rather than consulting a later checkout when this run reboots.
    definition = {}
    if build.get('commit'):
        root = Path(__file__).resolve().parents[1]
        for name in ('docs/suites/server.json', 'docs/suites/users.json',
                     'docs/efforts/path.json', 'docs/boot-path.json'):
            definition[name] = subprocess.check_output(
                ['git', '-C', str(root), 'show', build['commit'] + ':' + name])
    serial = raw = timestamps = events = None
    cleaner = None
    directory = None
    last_stamp = 0
    offset = 0
    line_open = False
    token_path = run / 'power-token'
    token = token_path.read_text().strip() if token_path.exists() else ''
    if token and not re.fullmatch(r'[a-f0-9]{64}', token):
        raise ValueError('invalid VM power token')
    receipt_line = ''
    expected_receipt = None
    shutdown_action = None

    def on_line(line):
        nonlocal expected_receipt, shutdown_action
        challenge = re.fullmatch(r'\[init\] shutdown nonce=([a-f0-9]{64})', line)
        if challenge and token:
            expected_receipt = hashlib.sha256((token + challenge[1]).encode()).hexdigest()
        request = re.fullmatch(r'\[shutdown\] requested (reboot|poweroff) token=([a-f0-9]{64})', line)
        if request and expected_receipt and hmac.compare_digest(request[2], expected_receipt):
            shutdown_action = request[1]
            events.write(json.dumps(dict(event='requested-shutdown', action=shutdown_action, t=time.time())) + '\n')
            expected_receipt = None

    def end_boot():
        if directory is not None:
            serial.close()
            raw.close()
            timestamps.close()
            events.close()
            (directory / 'ended').write_text(datetime.datetime.now(datetime.timezone.utc).isoformat() + '\n')

    def start_boot(reserved_stamp=None):
        nonlocal directory, serial, raw, cleaner, timestamps, events, last_stamp, offset, line_open
        nonlocal receipt_line, expected_receipt, shutdown_action
        stamp = reserved_stamp if reserved_stamp is not None else max(int(time.time()), last_stamp + 1)
        while True:
            boot = datetime.datetime.fromtimestamp(stamp, datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ-qemu')
            directory = run / 'boots' / boot
            try:
                directory.mkdir(parents=True)
                break
            except FileExistsError:
                stamp += 1
        last_stamp = stamp
        (directory / 'started').write_text(str(time.time()) + '\n')
        (directory / 'run.json').write_text(json.dumps(dict(record, id=boot)) + '\n')
        for name, content in definition.items():
            destination = directory / 'definition' / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(content)
        serial = (directory / 'serial.txt').open('ab', buffering=0)
        raw = (directory / 'serial.raw').open('ab', buffering=0)
        cleaner = SerialCleaner()
        timestamps = (directory / 'serial.ts').open('a', buffering=1)
        events = (directory / 'events.jsonl').open('a', buffering=1)
        (run / 'current').write_text(boot + '\n')
        offset = 0
        line_open = False
        receipt_line = ''
        expected_receipt = None
        shutdown_action = None

    start_boot()
    def terminate(_signum, _frame):
        raise SystemExit(0)
    signal.signal(signal.SIGTERM, terminate)
    selector = selectors.DefaultSelector()
    listeners = []
    clients = {}
    guest = None
    pending_guest = bytearray()
    qmp = None
    qmp_buffer = bytearray()
    paths = [run / 'serial.sock', run / 'console.sock']
    try:
        for path, kind in zip(paths, ('guest', 'console')):
            path.unlink(missing_ok=True)
            listener = socket.socket(socket.AF_UNIX)
            listener.bind(str(path))
            listener.listen()
            listener.setblocking(False)
            listeners.append(listener)
            selector.register(listener, selectors.EVENT_READ, kind)
        with open(whole_path, 'ab', buffering=0) as whole:
            while True:
                if qmp is None:
                    candidate = socket.socket(socket.AF_UNIX)
                    try:
                        candidate.connect(str(run / 'events.sock'))
                    except (FileNotFoundError, ConnectionRefusedError):
                        candidate.close()
                    else:
                        qmp = candidate
                        qmp.setblocking(False)
                        selector.register(qmp, selectors.EVENT_READ, 'qmp')
                for key, mask in selector.select(timeout=.05 if qmp is None else None):
                    sock, kind = key.fileobj, key.data
                    if sock in listeners:
                        connection, _ = sock.accept()
                        connection.setblocking(False)
                        if kind == 'guest':
                            if guest is not None:
                                connection.close()
                                continue
                            guest = connection
                            selector.register(connection, selectors.EVENT_READ | (selectors.EVENT_WRITE if pending_guest else 0), 'guest-data')
                        else:
                            clients[connection] = bytearray()
                            selector.register(connection, selectors.EVENT_READ, 'console-data')
                        continue
                    if kind == 'qmp':
                        data = sock.recv(65536)
                        if not data:
                            selector.unregister(sock)
                            sock.close()
                            qmp = None
                            continue
                        qmp_buffer.extend(data)
                        while b'\n' in qmp_buffer:
                            line, _, rest = qmp_buffer.partition(b'\n')
                            qmp_buffer[:] = rest
                            message = json.loads(line)
                            if 'QMP' in message:
                                sock.sendall(b'{"execute":"qmp_capabilities"}\n')
                            elif 'event' in message:
                                events.write(json.dumps(message) + '\n')
                                reason = message.get('data', {}).get('reason')
                                if message['event'] == 'SHUTDOWN' and reason == 'guest-shutdown':
                                    (directory / 'exit-status').write_text('0\n')
                                if message['event'] == 'RESET':
                                    previous_events = events
                                    requested = shutdown_action == 'reboot' and reason == 'guest-reset'
                                    if requested:
                                        # Reserve the successor identity before closing the ending boot.
                                        stamp = max(int(time.time()), last_stamp + 1)
                                        while (run / 'boots' / datetime.datetime.fromtimestamp(stamp, datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ-qemu')).exists():
                                            stamp += 1
                                        successor = datetime.datetime.fromtimestamp(stamp, datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ-qemu')
                                        previous_events.write(json.dumps(dict(event='requested-reset-observed', successor=successor, t=time.time())) + '\n')
                                    end_boot()
                                    start_boot(stamp if requested else None)
                        continue
                    # kqueue can return separate read and write notifications
                    # for one client. Its EOF event may already have closed it.
                    if kind == 'console-data' and sock not in clients:
                        continue
                    if mask & selectors.EVENT_READ:
                        try:
                            data = sock.recv(65536)
                        except ConnectionResetError:
                            if kind == 'guest-data':
                                raise
                            data = b''
                        if not data:
                            if kind == 'guest-data':
                                return
                            selector.unregister(sock)
                            del clients[sock]
                            sock.close()
                            continue
                        if kind == 'guest-data':
                            raw.write(data)
                            cleaned = cleaner.feed(data)
                            arrived = time.time()
                            receipt_line += cleaned
                            while '\n' in receipt_line:
                                complete, receipt_line = receipt_line.split('\n', 1)
                                on_line(complete)
                            for line in cleaned.splitlines(keepends=True):
                                if not line_open:
                                    timestamps.write(f'{arrived:.3f}\t{offset}\n')
                                encoded = line.encode()
                                serial.write(encoded)
                                offset += len(encoded)
                                line_open = not line.endswith('\n')
                            whole.write(data)
                            for client, pending in clients.items():
                                pending.extend(data)
                                selector.modify(client, selectors.EVENT_READ | selectors.EVENT_WRITE, 'console-data')
                        else:
                            pending_guest.extend(data)
                            if guest is not None:
                                selector.modify(guest, selectors.EVENT_READ | selectors.EVENT_WRITE, 'guest-data')
                    if mask & selectors.EVENT_WRITE:
                        pending = pending_guest if kind == 'guest-data' else clients[sock]
                        if pending:
                            try:
                                sent = sock.send(pending)
                            except (BrokenPipeError, ConnectionResetError):
                                if kind == 'guest-data':
                                    return
                                selector.unregister(sock)
                                del clients[sock]
                                sock.close()
                                continue
                            del pending[:sent]
                        if not pending:
                            selector.modify(sock, selectors.EVENT_READ, kind)
    finally:
        end_boot()
        if qmp is not None:
            qmp.close()
        selector.close()
        for sock in [*listeners, *clients, *([guest] if guest else [])]:
            sock.close()
        for path in paths:
            path.unlink(missing_ok=True)


if __name__ == '__main__':
    if len(sys.argv) != 3:
        raise SystemExit('usage: vm-serial-pump.py RUN-DIR WHOLE-RUN-SERIAL')
    main(sys.argv[1], sys.argv[2])
