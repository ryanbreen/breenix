"""Write boot-specific files onto a staged ext2 RAM disk and verify their bytes.

An install boot (docs/metal/README.md, "The disks") also gets the storage record `install-root <boot>` in the
image's first sector, which ext2 leaves unused: it asks the kernel to copy the RAM disk onto the machine's disk.
"""
from pathlib import Path
import re
import hashlib
import os
import shutil
import subprocess
import tempfile


RECORD_MAGIC = b'BREENIX-STORAGE 1\n'
SECTOR = 512


def install_record(boot_id):
    """The first sector asking the kernel to install this RAM disk on the hardware disk."""
    if not re.fullmatch(r'[A-Za-z0-9-]{1,64}', boot_id or ''):
        raise ValueError('an install needs a boot id')
    line = RECORD_MAGIC + f'install-root {boot_id}\n'.encode()
    return line + bytes(SECTOR - len(line))


def root_record(boot_id, size, digest):
    """The first sector the kernel writes last when that install succeeds."""
    line = RECORD_MAGIC + f'root {boot_id} {size} {digest}\n'.encode()
    return line + bytes(SECTOR - len(line))


def write_install_record(image, boot_id):
    """Put the install request in sector 0 and return what the kernel will write to the disk."""
    record = install_record(boot_id)
    with open(image, 'r+b') as handle:
        first = handle.read(SECTOR)
        handle.seek(1024 + 56)
        magic = handle.read(2)
        if len(first) != SECTOR or any(first) or magic != b'\x53\xef':
            raise ValueError('the root image has no unused first sector before its ext2 superblock')
        handle.seek(0)
        handle.write(record)
    with open(image, 'rb') as handle:
        if handle.read(SECTOR) != record:
            raise ValueError('the install record readback differs')
        digest = hashlib.sha256()
        while chunk := handle.read(1 << 20):
            digest.update(chunk)
    size = Path(image).stat().st_size
    if size % SECTOR or size <= SECTOR:
        raise ValueError('the root image is not a whole number of sectors')
    digest = digest.hexdigest()
    return {'installedBy': boot_id, 'lba': 0, 'bytes': size, 'sha256AfterRecord': digest,
            'rootRecordSha256': hashlib.sha256(root_record(boot_id, size, digest)).hexdigest()}


def stage_root(image, mode, suite, site, writer=None, install=None):
    if mode not in ('default', 'shell', 'probe', 'suite'):
        raise ValueError('unsupported RAM-disk boot mode')
    if mode == 'suite' and not re.fullmatch(r'[a-z0-9]+(?:-[a-z0-9]+)*', suite or ''):
        raise ValueError('suite mode needs a suite id')
    if mode != 'suite' and suite:
        raise ValueError('a suite needs suite mode')
    # Only explicitly guest-designated values cross from the host's site file.
    lines = []
    for key, value in sorted(site.items()):
        if not key.startswith('GUEST_'):
            continue
        if (not re.fullmatch(r'GUEST_[A-Z][A-Z0-9_]*', key)
                or re.search(r'PASS|SECRET|TOKEN|CREDENTIAL|KEY', key)
                or any(char in value for char in '\r\n\x00')):
            raise ValueError('invalid guest site field')
        lines.append(key.removeprefix('GUEST_') + '=' + value + '\n')
    debugfs = shutil.which('debugfs')
    if not debugfs:
        raise ValueError('staging needs debugfs')
    image = Path(image).resolve()
    packaged = Path(__file__).resolve().with_name('write-boot-target.sh')
    writer = Path(writer) if writer else (packaged if packaged.is_file() else
                                         Path(__file__).resolve().parents[1] / 'write-boot-target.sh')
    writer_hash = hashlib.sha256(writer.read_bytes()).hexdigest()
    target = suite if mode == 'suite' else '--' + mode
    try:
        subprocess.run(['bash', str(writer), str(image), target], check=True, capture_output=True,
                       env=dict(os.environ, BREENIX_REQUIRE_DEBUGFS='1'))
    except subprocess.CalledProcessError as error:
        raise ValueError('RAM-disk boot-target staging failed') from error
    with tempfile.TemporaryDirectory(prefix='metal-root-') as scratch:
        scratch = Path(scratch)
        files = {'site.env': ''.join(lines)}
        commands = []
        for name, text in files.items():
            source = scratch / name
            source.write_text(text)
            commands.extend([f'rm /etc/breenix/{name}', f'write {source} /etc/breenix/{name}'])
        script = scratch / 'commands'
        script.write_text('\n'.join(commands) + '\n')
        subprocess.run([debugfs, '-w', '-f', str(script), str(image)], check=True, capture_output=True)
        for name, text in files.items():
            result = subprocess.run([debugfs, '-R', f'cat /etc/breenix/{name}', str(image)],
                                    check=True, capture_output=True)
            if result.stdout != text.encode():
                raise ValueError('RAM-disk boot configuration readback differs')
        binary = {'suite': f'/sbin/suite-{suite}', 'probe': '/sbin/probe'}.get(mode, '/sbin/init')
        installed = scratch / 'pid1'
        subprocess.run([debugfs, '-R', f'dump {binary} {installed}', str(image)],
                       check=True, capture_output=True)
        if not installed.is_file() or installed.read_bytes()[:4] != b'\x7fELF':
            raise ValueError('RAM disk has no selected PID 1 ELF')
    facts = {'mode': mode, 'suite': suite or None, 'siteFields': len(lines), 'writerSha256': writer_hash}
    if install:
        facts['diskWrite'] = write_install_record(image, install)
    return facts
