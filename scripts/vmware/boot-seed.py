#!/usr/bin/env python3
"""Put fresh host entropy on a per-run VMware ESP; never seed the cached image.

Design reference only: systemd-boot(7)'s ESP random-seed handoff:
https://www.freedesktop.org/software/systemd/man/260/systemd-boot.html
Breenix consumes the file instead of deriving a replacement: only the host can
refresh it, and a boot without this script must not credit the old bytes again.
"""

from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import uuid


def write_seed(path):
    """Read the Mac's random device anew for every invocation, including --no-build."""
    with open('/dev/urandom', 'rb', buffering=0) as source:
        seed = source.read(64)
    if len(seed) != 64:
        raise RuntimeError('short read from host random source')
    Path(path).write_bytes(seed)


def esp_offset(image):
    """Locate the EFI System Partition in the raw image's GPT (512-byte sectors)."""
    with open(image, 'rb') as disk:
        disk.seek(512)
        header = disk.read(92)
        if len(header) != 92 or header[:8] != b'EFI PART':
            raise ValueError('boot disk has no GPT')
        table, count, size = struct.unpack_from('<QII', header, 72)
        if size < 128 or count > 4096 or size > 4096:
            raise ValueError('invalid GPT entry bounds')
        esp = uuid.UUID('C12A7328-F81F-11D2-BA4B-00A0C93EC93B').bytes_le
        for index in range(count):
            disk.seek(table * 512 + index * size)
            entry = disk.read(size)
            if len(entry) != size:
                raise ValueError('short GPT partition table')
            if entry[:16] == esp:
                return struct.unpack_from('<Q', entry, 32)[0] * 512
    raise ValueError('boot disk has no EFI System Partition')


def stage(source, destination):
    if Path(source).resolve() == Path(destination).resolve():
        raise ValueError('the reusable boot image must remain seed-free')
    with tempfile.TemporaryDirectory(prefix='breenix-boot-seed-') as work:
        raw = Path(work) / 'boot.img'
        seed = Path(work) / 'random-seed'
        subprocess.run(['qemu-img', 'convert', '-f', 'vmdk', '-O', 'raw', source, str(raw)], check=True)
        offset = esp_offset(raw)
        write_seed(seed)
        subprocess.run(['mcopy', '-o', '-i', f'{raw}@@{offset}', str(seed),
                        '::/EFI/BREENIX/random-seed'], check=True)
        subprocess.run(['qemu-img', 'convert', '-f', 'raw', '-O', 'vmdk', str(raw), destination], check=True)
    print('  Boot seed: 64 fresh host-random bytes on the per-run ESP')


if __name__ == '__main__':
    if len(sys.argv) != 3:
        sys.exit('usage: boot-seed.py SOURCE.vmdk PER-RUN.vmdk')
    stage(sys.argv[1], sys.argv[2])
