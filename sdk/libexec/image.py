#!/usr/bin/env python3
"""Install static Breenix ELF programs on an unmounted ext2 disk image."""
import argparse
from contextlib import contextmanager
import fcntl
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import sys
import tempfile

BINDIR = '/usr/local/bin'
REGISTRY = '/usr/local/lib/breenix-sdk/installed'
MACHINES = {62: 'x86_64', 183: 'aarch64'}


class ImageError(Exception):
    pass


def debugfs_tool():
    override = os.environ.get('BREENIX_DEBUGFS')
    if override:
        if not os.access(override, os.X_OK):
            raise ImageError('BREENIX_DEBUGFS is not executable')
        return override
    for prefix in ('/opt/homebrew/opt/e2fsprogs', '/usr/local/opt/e2fsprogs',
                   '/home/linuxbrew/.linuxbrew/opt/e2fsprogs'):
        candidate = Path(prefix) / 'sbin/debugfs'
        if candidate.is_file() and os.access(candidate, os.X_OK):
            return str(candidate)
    for directory in ('/sbin', '/usr/sbin'):
        candidate = Path(directory) / 'debugfs'
        if candidate.is_file() and os.access(candidate, os.X_OK):
            return str(candidate)
    found = shutil.which('debugfs')
    if found:
        return found
    raise ImageError('debugfs is required (install e2fsprogs or set BREENIX_DEBUGFS)')


def quote(value):
    value = str(value)
    if any(ord(c) < 32 or ord(c) == 127 for c in value):
        raise ImageError('control characters are not allowed in paths')
    return '"' + value.replace('\\', '\\\\').replace('"', '\\"') + '"'


def name_checked(value):
    # Registry records are one filename per line; no guest path traversal.
    if not re.fullmatch(r'[-A-Za-z0-9_.+]+', value) or value in ('.', '..'):
        raise ImageError('name must be a filename using letters, digits, -, _, . or +')
    return value


def elf_machine(data, program=True):
    if len(data) < 64 or data[:7] != b'\x7fELF\x02\x01\x01':
        raise ImageError('expected a little-endian ELF64 program')
    kind, machine, version = struct.unpack_from('<HHI', data, 16)
    if kind not in (2, 3) or machine not in MACHINES or version != 1:
        raise ImageError('expected an aarch64 or x86_64 executable ELF')
    phoff = struct.unpack_from('<Q', data, 32)[0]
    ehsize, phsize, phnum = struct.unpack_from('<HHH', data, 52)
    if ehsize != 64 or phsize != 56 or not phnum or phoff < 64 or phoff + phnum * phsize > len(data):
        raise ImageError('invalid ELF program header table')
    if program:
        for index in range(phnum):
            if struct.unpack_from('<I', data, phoff + index * phsize)[0] == 3:
                raise ImageError('PT_INTERP programs are unsupported; link a static executable')
    return machine


class Image:
    def __init__(self, path, directory, tool):
        self.path, self.directory, self.tool = path, directory, tool

    def command(self, command, write=False, missing=False):
        result = subprocess.run([self.tool, *(['-w'] if write else []), '-R', command, str(self.path)],
                                capture_output=True, text=True)
        # debugfs often exits zero even when the requested operation failed.
        errors = [line for line in result.stderr.splitlines()
                  if line and not line.startswith('debugfs ')]
        if missing and errors and all('File not found by ext2_lookup' in line for line in errors):
            return None
        if result.returncode or errors:
            raise ImageError('debugfs: ' + ('; '.join(errors) or result.stdout.strip()))
        return result.stdout

    def stat(self, path):
        return self.command('stat ' + quote(path), missing=True)

    def read(self, path):
        with tempfile.TemporaryDirectory(dir=self.directory) as scratch:
            output = Path(scratch) / 'contents'
            self.command('dump ' + quote(path) + ' ' + quote(output))
            if not output.is_file():
                raise ImageError('could not read back ' + path)
            return output.read_bytes()

    def metadata(self, path, mode):
        for key, value in (('uid', '0'), ('gid', '0'), ('mode', '0' + format(mode, 'o'))):
            self.command('sif ' + quote(path) + ' ' + key + ' ' + value, write=True)
        self.verify_metadata(path, mode)

    def verify_metadata(self, path, mode):
        status = self.stat(path) or ''
        expected_type = 'directory' if mode & 0o40000 else 'regular'
        if (not re.search(r'Type:\s+' + expected_type + r'\b', status)
                or not re.search(r'Mode:\s+0*' + format(mode & 0o7777, 'o') + r'\b', status)
                or not re.search(r'User:\s+0\s+Group:\s+0\b', status)):
            raise ImageError('metadata verification failed for ' + path)

    def mkdirs(self, path):
        current = ''
        for part in path.strip('/').split('/'):
            current += '/' + part
            status = self.stat(current)
            if status is None:
                self.command('mkdir ' + quote(current), write=True)
                self.metadata(current, 0o40755)
            elif not re.search(r'Type:\s+directory\b', status):
                raise ImageError(current + ' is not a directory')

    def write(self, path, data, mode):
        status = self.stat(path)
        if status is not None:
            if not re.search(r'Type:\s+regular\b', status):
                raise ImageError('refusing to replace a non-regular file: ' + path)
            self.command('rm ' + quote(path), write=True)
        with tempfile.TemporaryDirectory(dir=self.directory) as scratch:
            source = Path(scratch) / 'contents'
            source.write_bytes(data)
            self.command('write ' + quote(source) + ' ' + quote(path), write=True)
        self.metadata(path, 0o100000 | mode)
        if self.read(path) != data:
            raise ImageError('read-back differs from the input: ' + path)

    def installed(self):
        if self.stat(REGISTRY) is None:
            return set()
        try:
            return {name_checked(name) for name in self.read(REGISTRY).decode('ascii').splitlines()}
        except UnicodeError as error:
            raise ImageError('invalid installed-program registry') from error

    def registry(self, names):
        self.mkdirs('/usr/local/lib/breenix-sdk')
        self.write(REGISTRY, ''.join(name + '\n' for name in sorted(names)).encode('ascii'), 0o644)


@contextmanager
def locked(path):
    # A preceding writer may have replaced the inode while we waited. Reopen
    # until our lease is on the current image before reading or copying it.
    while True:
        handle = path.open('rb')
        fcntl.flock(handle, fcntl.LOCK_EX)
        if os.fstat(handle.fileno()).st_ino == path.stat().st_ino:
            break
        handle.close()
    try:
        yield
    finally:
        handle.close()


def operate(args):
    path = args.image.resolve(strict=True)
    if not path.is_file():
        raise ImageError('image must be an unmounted ext2 image file')
    data = args.file.read_bytes() if args.action == 'install' else None
    machine = elf_machine(data) if data is not None else None
    name = name_checked(args.name or args.file.name) if data is not None else None
    if args.action == 'remove':
        name = name_checked(args.name)
    tool = debugfs_tool()
    with locked(path), tempfile.TemporaryDirectory(prefix='.breenix-image-', dir=path.parent) as directory:
        disk = Image(path, directory, tool)
        if args.action == 'list':
            for installed in sorted(disk.installed()):
                print(installed)
            return
        if data is not None:
            disk_machine = elf_machine(disk.read('/bin/busybox'), program=False)
            if machine != disk_machine:
                raise ImageError('wrong architecture: program is ' + MACHINES[machine]
                                 + ', image is ' + MACHINES[disk_machine])
        names = disk.installed()
        if args.action == 'remove' and name not in names:
            raise ImageError('program is not installed: ' + name)
        # Publish only a fully verified edit. Failed operations leave the original
        # byte-identical, including allocation metadata and the registry.
        staged = Path(directory) / 'image'
        shutil.copyfile(path, staged)
        shutil.copymode(path, staged)
        disk.path = staged
        destination = BINDIR + '/' + name
        if data is not None:
            disk.mkdirs(BINDIR)
            disk.metadata(BINDIR, 0o40755)
            disk.write(destination, data, args.mode)
            names.add(name)
        else:
            if disk.stat(destination) is None:
                raise ImageError('registered program is missing: ' + name)
            disk.command('rm ' + quote(destination), write=True)
            if disk.stat(destination) is not None:
                raise ImageError('program still exists after removal: ' + name)
            names.remove(name)
        disk.registry(names)
        os.replace(staged, path)


def octal(value):
    if not re.fullmatch(r'[0-7]{3,4}', value):
        raise argparse.ArgumentTypeError('mode must be three or four octal digits')
    return int(value, 8)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--version', action='store_true')
    actions = parser.add_subparsers(dest='action')
    install = actions.add_parser('install')
    install.add_argument('--image', required=True, type=Path)
    install.add_argument('--mode', type=octal, default=0o755)
    install.add_argument('--name')
    install.add_argument('file', type=Path)
    remove = actions.add_parser('remove')
    remove.add_argument('--image', required=True, type=Path)
    remove.add_argument('name')
    listing = actions.add_parser('list')
    listing.add_argument('--image', required=True, type=Path)
    args = parser.parse_args()
    if args.version:
        root = Path(__file__).resolve().parents[3]
        stamp = (root / 'VERSION').read_text().strip().split()
        if len(stamp) != 3 or stamp[0] != 'breenix-sdk':
            raise ImageError('invalid SDK VERSION')
        print('breenix-sdk ' + stamp[1] + ' image ' + stamp[2])
    elif args.action:
        operate(args)
    else:
        parser.error('choose install, remove or list')


if __name__ == '__main__':
    try:
        main()
    except (ImageError, OSError) as error:
        print('breenix-image: ' + str(error), file=sys.stderr)
        sys.exit(1)
