#!/usr/bin/env python3
"""Install the hosted suite, probes and fixture on a private image."""

import argparse
from array import array
import hashlib
from pathlib import Path
import re
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent / 'metal'))
from metal_root import require_e2fsprogs

PROGRAMS = Path(__file__).resolve().parents[1] / 'userspace/programs'
SUITE_PATH = '/sbin/suite-rust-host'
PROBE_PATH = '/usr/libexec/breenix/rust-host/startup-probe'
PROCESS_PROBE_PATH = '/usr/libexec/breenix/rust-host/process-probe'
MMAP_FIXTURE_PATH = '/usr/libexec/breenix/rust-host/mmap-fixture'
MMAP_FIXTURE_BYTES = 163_067_344  # Largest file mapping in the X0.9 profile.
# Leave room for the fixture and ext2 allocation metadata.
SCRATCH_HEADROOM = 192 * 1024 * 1024


def probe_for_suite(selection, probe):
    return probe if 'rust-host' in selection.split(',') else None


def write_mmap_fixture(path):
    with path.open('wb') as output:
        chunk_bytes = 1024 * 1024
        for start in range(0, MMAP_FIXTURE_BYTES, chunk_bytes):
            end = min(start + chunk_bytes, MMAP_FIXTURE_BYTES)
            words = array('Q', range(start, end, 8))
            if sys.byteorder != 'little':
                words.byteswap()
            words.tofile(output)


def stage(image, selection, suite, probe, process_probe):
    probe = probe_for_suite(selection, probe)
    if probe is None:
        return
    entries = ((SUITE_PATH, suite), (PROBE_PATH, probe), (PROCESS_PROBE_PATH, process_probe))
    for _, elf in entries:
        with elf.open('rb') as source:
            if source.read(4) != b'\x7fELF':
                raise ValueError(f'hosted Rust binary is not an ELF: {elf}')

    debugfs, resize2fs = require_e2fsprogs('debugfs', 'resize2fs')
    with image.open('r+b') as disk:
        disk.truncate(image.stat().st_size + SCRATCH_HEADROOM)
    subprocess.run([resize2fs, image], check=True, capture_output=True)
    with tempfile.TemporaryDirectory(prefix='rust-host-stage-') as temporary:
        directory = Path(temporary)
        fixture = directory / 'mmap-fixture'
        write_mmap_fixture(fixture)
        commands = directory / 'commands'
        commands.write_text(
            'mkdir /sbin\nmkdir /usr\nmkdir /usr/libexec\nmkdir /usr/libexec/breenix\n'
            'mkdir /usr/libexec/breenix/rust-host\n'
            + ''.join(f'rm {path}\nwrite "{elf.resolve()}" {path}\n'
                      f'set_inode_field {path} mode 0100755\n'
                      f'set_inode_field {path} uid 0\nset_inode_field {path} gid 0\n'
                      for path, elf in entries)
            + f'rm {MMAP_FIXTURE_PATH}\nwrite "{fixture.resolve()}" {MMAP_FIXTURE_PATH}\n'
            f'set_inode_field {MMAP_FIXTURE_PATH} mode 0100444\n'
            f'set_inode_field {MMAP_FIXTURE_PATH} uid 0\nset_inode_field {MMAP_FIXTURE_PATH} gid 0\n'
        )
        subprocess.run([debugfs, '-w', '-f', commands, image], check=True, capture_output=True)
        # debugfs may exit successfully after a command failed. Read the file back.
        with fixture.open('rb') as source, subprocess.Popen(
            [debugfs, '-R', f'cat {MMAP_FIXTURE_PATH}', image],
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
        ) as copied:
            expected = hashlib.file_digest(source, 'sha256').digest()
            actual = hashlib.file_digest(copied.stdout, 'sha256').digest()
            if copied.wait() != 0 or actual != expected:
                raise ValueError(f'hosted Rust staging readback differs for {MMAP_FIXTURE_PATH}')
        stat = subprocess.check_output(
            [debugfs, '-R', f'stat {MMAP_FIXTURE_PATH}', image], stderr=subprocess.DEVNULL,
        )
        if b'Mode:  0444' not in stat or not re.search(rb'User:\s+0\s+Group:\s+0', stat):
            raise ValueError(f'hosted Rust staging metadata differs for {MMAP_FIXTURE_PATH}')
    # debugfs may exit successfully after an individual command failed.
    for path, elf in entries:
        data = subprocess.check_output([debugfs, '-R', f'cat {path}', image], stderr=subprocess.DEVNULL)
        stat = subprocess.check_output([debugfs, '-R', f'stat {path}', image], stderr=subprocess.DEVNULL)
        if (data != elf.read_bytes() or b'Mode:  0755' not in stat
                or not re.search(rb'User:\s+0\s+Group:\s+0', stat)):
            raise ValueError(f'hosted Rust staging readback differs for {path}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--arch', choices=('x86_64', 'aarch64'), required=True)
    parser.add_argument('--suite', required=True)
    parser.add_argument('--image', type=Path, required=True)
    args = parser.parse_args()
    release = PROGRAMS / f'target/hosted/{args.arch}-breenix/release'
    stage(args.image, args.suite, release / 'suite-rust-host',
          release / 'rust-host-startup-probe', release / 'rust-host-process-probe')


if __name__ == '__main__':
    main()
