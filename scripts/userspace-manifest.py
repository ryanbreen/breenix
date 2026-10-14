#!/usr/bin/env python3
"""Record and verify which sources produced the installed userspace ELFs.

userspace/programs/build.sh writes the manifest after it installs every
binary. A consumer that accepts prebuilt userspace (BREENIX_USERSPACE_PREBUILT=1)
checks it, so ELFs from older sources, a missing binary or an extra one are
refused instead of packed.

  userspace-manifest.py write --arch x86_64|aarch64
  userspace-manifest.py check --arch x86_64|aarch64 [--disk IMAGE]

--disk also refuses a disk image packed before the installed userspace last
changed. The manifest is rewritten only when its content changes, so its
modification time marks the last userspace change.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = '.userspace-manifest.json'
# Installed by scripts/install-busybox.py from a pinned, checksummed build.
FOREIGN = {'busybox.elf'}
# Ignored by git, but still inputs of the build.
LOCKFILES = ('userspace/programs/Cargo.lock', 'libs/libbreenix-libc/Cargo.lock')
# The build of C programs against Breenix's own C library (libc suite, BusyBox).
C_BUILD = ('scripts/breenix-cc', 'scripts/libc-symbol-audit.py', 'scripts/libc-clean-archive.py', 'scripts/build-libc-busybox.sh',
           'vendor/busybox/breenix-libc.config', 'vendor/busybox/busybox-1.37.0.tar.bz2',
           'x86_64-breenix-c.json', 'vendor/busybox/patches',
           'vendor/busybox/required-applets', 'scripts/install-libc-busybox.sh')


def elf_dir(arch):
    directory = ROOT / 'userspace/programs'
    return directory / 'aarch64' if arch == 'aarch64' else directory


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def fingerprint(arch):
    listed = subprocess.check_output(
        ['git', '-C', str(ROOT), 'ls-files', '-z', '--cached', '--others', '--exclude-standard',
         '--', 'userspace', 'libs', 'rust-toolchain.toml', f'{arch}-breenix.json', *C_BUILD])
    names = {os.fsdecode(name) for name in listed.split(b'\0') if name}
    names.update(name for name in LOCKFILES if (ROOT / name).is_file())
    result = hashlib.sha256(f'breenix-userspace-v1\0{arch}\0'.encode())
    for name in sorted(names):
        path = ROOT / name
        if name.endswith('.elf') or Path(name).name == MANIFEST or not path.is_file():
            continue
        result.update(name.encode() + b'\0' + path.read_bytes() + b'\0')
    return result.hexdigest()


def inventory(arch):
    return {path.name: digest(path) for path in sorted(elf_dir(arch).glob('*.elf'))
            if path.name not in FOREIGN}


def write(arch):
    path = elf_dir(arch) / MANIFEST
    text = json.dumps(dict(arch=arch, sources=fingerprint(arch), elfs=inventory(arch)),
                      indent=1, sort_keys=True) + '\n'
    if not path.is_file() or path.read_text() != text:
        path.write_text(text)
    return 0


def check(arch, disk):
    path = elf_dir(arch) / MANIFEST
    rebuild = f'rebuild with userspace/programs/build.sh --arch {arch}'
    if not path.is_file():
        return f'no userspace manifest at {path}; {rebuild}'
    manifest = json.loads(path.read_text())
    if manifest.get('arch') != arch:
        return f'{path} records {manifest.get("arch")}, not {arch}; {rebuild}'
    if manifest.get('sources') != fingerprint(arch):
        return f'userspace sources changed since the installed {arch} ELFs were built; {rebuild}'
    recorded, actual = manifest.get('elfs', {}), inventory(arch)
    if recorded != actual:
        names = sorted(name for name in recorded.keys() | actual.keys()
                       if recorded.get(name) != actual.get(name))
        return f'installed {arch} ELFs differ from the last build ({", ".join(names)}); {rebuild}'
    if disk is not None:
        if not disk.is_file():
            return f'no disk image at {disk}'
        if disk.stat().st_mtime < path.stat().st_mtime:
            return f'{disk} was packed before the installed {arch} userspace last changed'
    return None


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('operation', choices=('write', 'check'))
    parser.add_argument('--arch', choices=('x86_64', 'aarch64'), required=True)
    parser.add_argument('--disk', type=Path)
    args = parser.parse_args()
    if args.operation == 'write':
        return write(args.arch)
    error = check(args.arch, args.disk)
    if error:
        print(f'ERROR: {error}', file=sys.stderr)
        return 1
    print(f'Prebuilt {args.arch} userspace matches its sources')
    return 0


if __name__ == '__main__':
    sys.exit(main())
