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
import secrets
from urllib.parse import urlsplit, urlunsplit
from metal_probe import PROBE_PATH, PROBE_SHA256


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


def console_identity(site):
    user, password = site.get('CONSOLE_USER'), site.get('CONSOLE_PASSWORD', '')
    if user != 'root' or not 20 <= len(password) <= 256 or not re.fullmatch(r'[A-Za-z0-9_-]+', password):
        raise ValueError('console staging needs a root account and a private strong password')
    return hashlib.sha256((user + '\0' + password).encode()).hexdigest()


def console_shadow(site, hasher):
    console_identity(site)
    alphabet = './0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz'
    salt = ''.join(secrets.choice(alphabet) for _ in range(16))
    try:
        result = subprocess.run([str(hasher)], input=site['CONSOLE_PASSWORD'].encode() + b'\0$6$' + salt.encode(),
                                capture_output=True, check=True, timeout=30)
    except (OSError, subprocess.SubprocessError):
        raise ValueError('console password hashing failed') from None
    value = result.stdout.decode('ascii', errors='replace')
    if not re.fullmatch(r'\$6\$' + re.escape(salt) + r'\$[./A-Za-z0-9]{86}', value):
        raise ValueError('console password hash is malformed')
    return 'root:' + value + ':0:0:99999:7:::\n'


def stage_root(image, mode, suite, site, writer=None, install=None, hasher=None):
    if mode not in ('default', 'shell', 'probe', 'suite', 'console'):
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
    if mode == 'probe':
        if not site.get('BOOT_URL'):
            raise ValueError('probe staging needs BOOT_URL')
        url = urlsplit(site['BOOT_URL'])
        lines += ['METAL_FETCH_URL=' + urlunsplit((url.scheme, url.netloc, PROBE_PATH, '', '')) + '\n',
                  'METAL_FETCH_SHA256=' + PROBE_SHA256 + '\n']
    shadow = console_shadow(site, hasher) if mode == 'console' else None
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
        commands.extend(['rm /etc/resolv.conf', 'symlink /etc/resolv.conf /proc/net/pnp'])
        if shadow is not None:
            source = scratch / 'shadow'
            source.touch(mode=0o600)
            source.write_text(shadow)
            commands.extend(['rm /etc/shadow', f'write {source} /etc/shadow',
                             'set_inode_field /etc/shadow mode 0100600',
                             'set_inode_field /etc/shadow uid 0', 'set_inode_field /etc/shadow gid 0'])
        script = scratch / 'commands'
        script.write_text('\n'.join(commands) + '\n')
        subprocess.run([debugfs, '-w', '-f', str(script), str(image)], check=True, capture_output=True)
        for name, text in files.items():
            result = subprocess.run([debugfs, '-R', f'cat /etc/breenix/{name}', str(image)],
                                    check=True, capture_output=True)
            if result.stdout != text.encode():
                raise ValueError('RAM-disk boot configuration readback differs')
        if shadow is not None:
            readback = subprocess.run([debugfs, '-R', 'cat /etc/shadow', str(image)], check=True, capture_output=True)
            stat = subprocess.run([debugfs, '-R', 'stat /etc/shadow', str(image)], check=True, capture_output=True).stdout
            if (readback.stdout != shadow.encode() or b'Mode:  0600' not in stat
                    or not re.search(rb'User:\s+0\s+Group:\s+0', stat)):
                raise ValueError('console shadow bytes or permissions differ')
            for program in ('getty', 'login'):
                installed = scratch / program
                subprocess.run([debugfs, '-R', f'dump /bin/{program} {installed}', str(image)], check=True, capture_output=True)
                if not installed.is_file() or installed.read_bytes()[:4] != b'\x7fELF':
                    raise ValueError('console login program is missing')
        resolver = subprocess.run([debugfs, '-R', 'stat /etc/resolv.conf', str(image)],
                                  check=True, capture_output=True).stdout
        if b'Type: symlink' not in resolver or b'"/proc/net/pnp"' not in resolver:
            raise ValueError('RAM-disk resolver does not name the live DHCP configuration')
        binary = {'suite': f'/sbin/suite-{suite}', 'probe': '/sbin/probe'}.get(mode, '/sbin/init')
        installed = scratch / 'pid1'
        subprocess.run([debugfs, '-R', f'dump {binary} {installed}', str(image)],
                       check=True, capture_output=True)
        if not installed.is_file() or installed.read_bytes()[:4] != b'\x7fELF':
            raise ValueError('RAM disk has no selected PID 1 ELF')
    facts = {'mode': mode, 'suite': suite or None, 'siteFields': len(lines), 'writerSha256': writer_hash}
    if mode == 'console':
        facts['consoleIdentity'] = console_identity(site)
    if install:
        facts['diskWrite'] = write_install_record(image, install)
    return facts
