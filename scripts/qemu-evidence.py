#!/usr/bin/env python3
"""Serve and stage a standalone QEMU ui-testing evidence route."""
import argparse
from contextlib import nullcontext
from http.server import BaseHTTPRequestHandler, HTTPServer
import json
import os
from pathlib import Path
import re
import secrets
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent / 'metal'))
import gt710_upload
import metal_lib
from metal_root import require_e2fsprogs


class Backend:
    def __init__(self, directory, parent, boot_id):
        self.STATE = directory
        self.parent = parent
        self.boot_id = boot_id

    def alive(self):
        try:
            os.kill(self.parent, 0)
            return True
        except ProcessLookupError:
            return False
        except PermissionError:
            return True

    def read_lease(self):
        return {'id': self.boot_id, 'state': 'powered'} if self.alive() else None

    def boot_dir(self, boot_id):
        return self.STATE

    def locked(self, name):
        return nullcontext()

    read_json = staticmethod(metal_lib.read_json)
    write_json = staticmethod(metal_lib.write_json)


def serve(directory, parent):
    directory.mkdir(parents=True, exist_ok=True)
    boot_id = secrets.token_hex(16)
    token = secrets.token_hex(32)
    backend = Backend(directory, parent, boot_id)
    if not backend.alive():
        raise ValueError('parent is not alive')
    class Handler(BaseHTTPRequestHandler):
        def do_HEAD(self):
            gt710_upload.ready(self, backend)

        def do_PUT(self):
            gt710_upload.receive(self, backend)

        def log_message(self, format, *args):
            pass

    with HTTPServer(('127.0.0.1', 0), Handler) as server:
        server.timeout = 0.2
        metal_lib.write_json(directory / 'gt710-upload.json',
                             {'id': boot_id, 'route': '/gt710-upload/' + token, 'suite': 'ui-testing'})
        metal_lib.write_json(directory / 'route.json', {'port': server.server_port, 'token': token})
        while backend.alive():
            server.handle_request()


def stage(image, directory):
    debugfs, = require_e2fsprogs('debugfs')
    image = Path(image).resolve()
    route = json.loads((directory / 'route.json').read_text())
    port, token = route['port'], route['token']
    if type(port) is not int or not 1 <= port <= 65535 or not isinstance(token, str) or not re.fullmatch(r'[0-9a-f]{64}', token):
        raise ValueError('invalid evidence route')
    def read(command):
        return subprocess.run([debugfs, '-R', command, str(image)], check=True, capture_output=True)
    if read('cat /etc/breenix/boot-target').stdout != b'suite ui-testing\n':
        raise ValueError('evidence route requires standalone ui-testing')
    if b'Inode:' in read('stat /etc/breenix/suite-sequence').stdout:
        raise ValueError('evidence route refuses suite sequences')
    content = f'http://10.0.2.2:{port}/gt710-upload/{token}\n'
    with tempfile.TemporaryDirectory(prefix='qemu-evidence-') as tmp:
        source = Path(tmp) / 'route'
        source.write_text(content)
        commands = Path(tmp) / 'commands'
        commands.write_text(f'rm /etc/breenix/evidence-route\nwrite {source} /etc/breenix/evidence-route\n'
                            'set_inode_field /etc/breenix/evidence-route mode 0100400\n'
                            'set_inode_field /etc/breenix/evidence-route uid 0\n'
                            'set_inode_field /etc/breenix/evidence-route gid 0\n')
        subprocess.run([debugfs, '-w', '-f', str(commands), str(image)], check=True, capture_output=True)
    if read('cat /etc/breenix/evidence-route').stdout != content.encode():
        raise ValueError('evidence route staging failed')
    stat = read('stat /etc/breenix/evidence-route').stdout.decode()
    if 'Mode:  0400' not in stat or not re.search(r'User:\s+0\s+Group:\s+0', stat):
        raise ValueError('evidence route permissions differ')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    receiver = sub.add_parser('serve')
    receiver.add_argument('--dir', type=Path, required=True)
    receiver.add_argument('--parent', type=int, required=True)
    staging = sub.add_parser('stage')
    staging.add_argument('image', type=Path)
    staging.add_argument('directory', type=Path)
    args = parser.parse_args()
    try:
        if args.command == 'serve':
            if args.parent <= 0:
                raise ValueError('invalid parent PID')
            serve(args.dir, args.parent)
        else:
            stage(args.image, args.directory)
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        parser.exit(1, f'qemu-evidence: {error}\n')


if __name__ == '__main__':
    main()
