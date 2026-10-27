"""Bounded, checksum-checked serial receipt for the RAM-only nouveau instrument."""
import base64
import gzip
import hashlib
import io
from pathlib import Path
import re
import tarfile

LIMIT = 1200000  # base64 bytes, below the operator's 2 MB wire budget
FILES = {'trace.txt', 'first.txt', 'last.txt', 'dmesg.txt', 'uname.txt', 'cmdline.txt',
         'safety.txt', 'display.txt', 'filter.txt', 'status.txt'}


def validate_config(text):
    values = dict(re.findall(r'^CONFIG_(\w+)=(.*)$', text, re.M))
    for name in ('MMIOTRACE', 'EFI_STUB', 'BLK_DEV_INITRD', 'SERIAL_8250_CONSOLE', 'MODULES'):
        if values.get(name) != 'y':
            raise ValueError('trace kernel missing ' + name)
    if values.get('DRM_NOUVEAU') != 'm':
        raise ValueError('nouveau must load after tracer activation')
    # Distribution drivers may exist as modules, but cannot be built in.
    for name in ('TG3', 'E1000', 'E1000E', 'IGB', 'IGC', 'BNX2', 'BNX2X', 'R8169',
                 'SATA_AHCI', 'ATA_PIIX', 'BLK_DEV_NVME', 'BLK_DEV_SD', 'USB_STORAGE', 'IPMI_HANDLER'):
        if values.get(name) == 'y':
            raise ValueError('built-in device driver: ' + name)


class Trace:
    def __init__(self):
        self.started = self.done = False
        self.header = None
        self.parts = []
        self.files = {}
        self.total = 0
        self.receiving = False

    def feed(self, line):
        if line == 'metal-trace-start':
            if self.started:
                raise ValueError('duplicate trace start')
            self.started = True
            return False
        if not self.started:
            return False
        if self.receiving:
            if line == 'END metal-nouveau-trace-v1':
                data = base64.b64decode(''.join(self.parts), validate=True)
                size, digest = self.header
                if len(data) != size or hashlib.sha256(data).hexdigest() != digest:
                    raise ValueError('trace checksum mismatch')
                with gzip.GzipFile(fileobj=io.BytesIO(data)) as stream:
                    raw = stream.read(128 * 1024 * 1024 + 1)
                if len(raw) > 128 * 1024 * 1024:
                    raise ValueError('expanded trace too large')
                with tarfile.open(fileobj=io.BytesIO(raw)) as archive:
                    for member in archive:
                        if member.name not in FILES or not member.isfile() or member.name in self.files:
                            raise ValueError('unexpected archive member')
                        self.files[member.name] = archive.extractfile(member).read()
                if self.files.keys() != FILES:
                    raise ValueError('missing evidence file')
                self.receiving = False
            else:
                if not re.fullmatch(r'[A-Za-z0-9+/=]{1,76}', line):
                    raise ValueError('damaged trace frame')
                self.total += len(line)
                if self.total > LIMIT:
                    raise ValueError('trace exceeds wire budget')
                self.parts.append(line)
            return False
        if self.header is None and re.fullmatch(
                r'metal-trace-progress phase=(module|module-return|collect|filter|compress) bytes=[0-9]+', line):
            return False
        match = re.fullmatch(r'TRACE ([0-9]+) ([0-9a-f]{64})', line)
        if match and self.header is None:
            self.header = (int(match[1]), match[2])
            if self.header[0] > LIMIT * 3 // 4:
                raise ValueError('trace exceeds wire budget')
        elif line == 'BEGIN metal-nouveau-trace-v1' and self.header and not self.files:
            self.receiving = True
        elif line == 'metal-trace-ok' and self.files:
            self.done = True
        else:
            raise ValueError('unexpected trace protocol line')
        return self.done

    def write(self, destination):
        if not self.done:
            raise ValueError('incomplete trace receipt')
        destination = Path(destination)
        destination.mkdir(exist_ok=False)
        for name, data in self.files.items():
            (destination / name).write_bytes(data)
