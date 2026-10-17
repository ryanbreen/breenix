"""Checksum-checked, bounded platform-fact frames from a private SOL capture."""
import base64
import hashlib
from pathlib import Path, PurePosixPath
import re
import shutil
import tempfile

LIMIT = 8 * 1024 * 1024
REQUIRED = {'cpu/cpuid', 'acpi/APIC', 'acpi/FACP', 'proc/cpuinfo', 'proc/iomem', 'proc/interrupts',
            'proc/cmdline', 'kernel/dmesg', 'kernel/platform-log', 'census/manifest'}


FORBIDDEN_CONFIG = ('MODULES', 'NET', 'BLOCK', 'ATA', 'SCSI', 'NVME_CORE', 'USB_SUPPORT',
            'I2C', 'SPI', 'DRM', 'FB', 'SOUND', 'INPUT', 'SERIO', 'PARPORT',
            'ACPI_HOTPLUG_CPU', 'ACPI_PROCESSOR', 'ACPI_FAN', 'ACPI_CPU_FREQ_PSS', 'ACPI_THERMAL',
            'X86_INTEL_PSTATE', 'X86_ACPI_CPUFREQ', 'INTEL_IDLE', 'HW_RANDOM', 'WATCHDOG', 'HWMON', 'THERMAL', 'CPU_FREQ', 'CPU_IDLE')
# Linux 6.12 selects CPU hotplug with SMP; it is unrelated to device drivers.
REQUIRED_CONFIG = ('EFI_STUB', 'ACPI', 'PCI', 'SERIAL_8250_CONSOLE', 'SYSFS', 'SMP',
                   'CMDLINE_OVERRIDE', 'FIRMWARE_MEMMAP', 'BLK_DEV_INITRD', 'BINFMT_SCRIPT')


def validate_config(text):
    """Enforce the instrument's driver prohibitions at build and staging."""
    values = dict(re.findall(r'^CONFIG_(\w+)=(.*)$', text, re.M))
    for symbol in FORBIDDEN_CONFIG:
        if values.get(symbol) in ('y', 'm'):
            raise ValueError('forbidden configuration: ' + symbol)
    if 'm' in values.values():
        raise ValueError('loadable driver in instrument configuration')
    for symbol in REQUIRED_CONFIG:
        if values.get(symbol) != 'y':
            raise ValueError('missing configuration: ' + symbol)
    if not values.get('INITRAMFS_SOURCE', '').strip('"'):
        raise ValueError('built-in initramfs missing')


class Census:
    def __init__(self):
        self.started = False
        self.files_done = False
        self.done = False
        self.metadata = None
        self.current = None
        self.parts = []
        self.files = {}
        self.total = 0

    def feed(self, line):
        if line == 'metal-linux-start':
            if self.started:
                raise ValueError('duplicate census start')
            self.started = True
            return False
        if not self.started:
            return False
        if self.done:
            raise ValueError('data after census success')
        if self.current:
            if line == 'END ' + self.current:
                data = base64.b64decode(''.join(self.parts), validate=True)
                name, size, digest = self.metadata
                if len(data) != size or hashlib.sha256(data).hexdigest() != digest:
                    raise ValueError('platform fact checksum or size mismatch')
                self.files[name] = data
                self.current = self.metadata = None
                self.parts = []
            else:
                if not re.fullmatch(r'[A-Za-z0-9+/=]{1,76}', line):
                    raise ValueError('damaged platform fact frame')
                self.parts.append(line)
                self.total += len(line)
                if self.total > LIMIT:
                    raise ValueError('platform facts exceed capture limit')
            return False
        if self.metadata:
            if line != 'BEGIN ' + self.metadata[0]:
                raise ValueError('missing platform fact begin')
            self.current = self.metadata[0]
            return False
        if line.startswith('FILE ') and not self.files_done:
            match = re.fullmatch(r'FILE ([A-Za-z0-9_./:-]+) ([0-9]+) ([0-9a-f]{64})', line)
            if not match:
                raise ValueError('invalid platform fact header')
            name, size, digest = match.groups()
            path = PurePosixPath(name)
            if path.is_absolute() or '..' in path.parts or str(path) != name or name in self.files:
                raise ValueError('unsafe or duplicate platform fact path')
            if int(size) > LIMIT:
                raise ValueError('platform fact too large')
            self.metadata = (name, int(size), digest)
        elif line == 'metal-linux-files-done' and not self.files_done:
            self.files_done = True
        elif line == 'metal-linux-ok' and self.files_done:
            if not REQUIRED <= self.files.keys():
                raise ValueError('required platform facts missing')
            if not any(k.startswith('pci/') and k.endswith('/config') for k in self.files):
                raise ValueError('PCI config census missing')
            if not any(k.startswith('memmap/') and k.endswith('/type') for k in self.files):
                raise ValueError('firmware memory map missing')
            expected = {}
            for row in self.files['census/manifest'].decode('ascii').splitlines():
                name, size, digest = row.split()
                if name in expected:
                    raise ValueError('duplicate manifest path')
                expected[name] = (int(size), digest)
            actual = {name: (len(data), hashlib.sha256(data).hexdigest())
                      for name, data in self.files.items() if name != 'census/manifest'}
            if expected != actual:
                raise ValueError('census manifest does not match received files')
            self.done = True
        else:
            raise ValueError('unexpected census line')
        return self.done

    def write(self, destination):
        if not self.done:
            raise ValueError('incomplete platform facts')
        destination = Path(destination)
        if destination.exists():
            raise FileExistsError(destination)
        temporary = Path(tempfile.mkdtemp(prefix='.census-', dir=destination.parent))
        try:
            for name, data in self.files.items():
                target = temporary / name
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(data)
            temporary.rename(destination)
        finally:
            if temporary.exists():
                shutil.rmtree(temporary)


def extract_record(record, destination):
    """Refuse interrupted or gapped records, including captures with an apparent success line."""
    import json
    record = Path(record)
    if not (record / 'ended').exists() or (record / 'exit-status').read_text().strip() != '0':
        raise ValueError('record did not finish successfully')
    for line in (record / 'events.jsonl').read_text().splitlines():
        event = json.loads(line)
        if 'console-gap' in event.get('event', ''):
            raise ValueError('record has a console gap')
    census = Census()
    for line in (record / 'serial.txt').read_text().splitlines():
        if census.feed(line):
            break
    census.write(destination)
    return len(census.files)
