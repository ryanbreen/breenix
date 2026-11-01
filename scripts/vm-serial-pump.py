#!/usr/bin/env python3
"""Capture a persistent QEMU run's serial stream and expose its console."""
import datetime
import selectors
import socket
import sys
import time
from pathlib import Path


def main(run, whole_path):
    run = Path(run)
    boot = datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ-qemu')
    directory = run / 'boots' / boot
    directory.mkdir(parents=True, exist_ok=False)
    (run / 'current').write_text(boot + '\n')
    selector = selectors.DefaultSelector()
    listeners = []
    clients = {}
    guest = None
    pending_guest = bytearray()
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
        with (directory / 'serial.txt').open('ab', buffering=0) as serial, \
             (directory / 'serial.ts').open('a', buffering=1) as timestamps, \
             open(whole_path, 'ab', buffering=0) as whole:
            offset = 0
            while True:
                for key, mask in selector.select():
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
                    if mask & selectors.EVENT_READ:
                        data = sock.recv(65536)
                        if not data:
                            if kind == 'guest-data':
                                return
                            selector.unregister(sock)
                            del clients[sock]
                            sock.close()
                            continue
                        if kind == 'guest-data':
                            serial.write(data)
                            whole.write(data)
                            offset += len(data)
                            timestamps.write(f'{time.time()}\t{offset}\n')
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
        selector.close()
        for sock in [*listeners, *clients, *([guest] if guest else [])]:
            sock.close()
        for path in paths:
            path.unlink(missing_ok=True)


if __name__ == '__main__':
    if len(sys.argv) != 3:
        raise SystemExit('usage: vm-serial-pump.py RUN-DIR WHOLE-RUN-SERIAL')
    main(sys.argv[1], sys.argv[2])
