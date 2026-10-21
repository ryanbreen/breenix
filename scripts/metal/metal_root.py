"""Write boot-specific files onto a staged ext2 RAM disk and verify their bytes.

An install boot (docs/metal/README.md, "The disks") also gets the storage record `install-root <boot>` in the
image's first sector, which ext2 leaves unused: it asks the kernel to copy the RAM disk onto the machine's disk.
"""
import base64
from pathlib import Path
import re
import hashlib
import struct
import hmac
import time
import os
import shutil
import subprocess
import tempfile
import secrets
import sys
from urllib.parse import urlsplit, urlunsplit
from metal_probe import PROBE_PATH, PROBE_SHA256


def require_e2fsprogs(*commands):
    """Resolve PATH, Linux sbin and Homebrew's keg before doing any staging."""
    found = []
    for command in commands:
        candidates = (command, '/usr/sbin/' + command, '/sbin/' + command,
                      '/opt/homebrew/opt/e2fsprogs/sbin/' + command,
                      '/usr/local/opt/e2fsprogs/sbin/' + command)
        executable = next((path for candidate in candidates if (path := shutil.which(candidate))), None)
        if executable is None:
            install = 'brew install e2fsprogs' if sys.platform == 'darwin' else 'sudo apt-get install e2fsprogs'
            raise ValueError(f'missing prerequisite: {command} (e2fsprogs); install with: {install}')
        found.append(executable)
    return found


RECORD_MAGIC = b'BREENIX-STORAGE 1\n'
SECTOR = 512
# A suite boot's host-key path (docs/console/contract.md section 10): the boot loop types HID boot reports on the
# serial console and the kernel replays them through its USB boot keyboard (scripts/hostkeys.py, ReplayKeys).
HOSTKEYS = 'xhci-replay'


def install_record(boot_id, partition=None):
    """The first sector asking the kernel to install this RAM disk on the hardware disk: on the persistent layout,
    into the test root partition with unique GUID `partition` (kernel/src/block/install.rs)."""
    if not re.fullmatch(r'[A-Za-z0-9-]{1,64}', boot_id or ''):
        raise ValueError('an install needs a boot id')
    if partition is not None and not re.fullmatch(r'[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}', partition):
        raise ValueError('an install partition must be a GUID')
    line = RECORD_MAGIC + (f'install-root {boot_id} {partition}\n' if partition else f'install-root {boot_id}\n').encode()
    return line + bytes(SECTOR - len(line))


def root_record(boot_id, size, digest):
    """The first sector the kernel writes last when that install succeeds."""
    line = RECORD_MAGIC + f'root {boot_id} {size} {digest}\n'.encode()
    return line + bytes(SECTOR - len(line))


def write_install_record(image, boot_id, partition=None):
    """Put the install request in sector 0 and return what the kernel will write to the disk."""
    record = install_record(boot_id, partition)
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
            **({'partition': partition} if partition else {}),
            'rootRecordSha256': hashlib.sha256(root_record(boot_id, size, digest)).hexdigest()}


def console_identity(site):
    user, password = site.get('CONSOLE_USER'), site.get('CONSOLE_PASSWORD', '')
    if user != 'root' or not 20 <= len(password) <= 256 or not re.fullmatch(r'[A-Za-z0-9_-]+', password):
        raise ValueError('console staging needs a root account and a private strong password')
    key = site.get('CONSOLE_IDENTITY_KEY', '')
    if not re.fullmatch(r'[0-9a-f]{64}', key):
        raise ValueError('console staging needs a private random identity key')
    return hmac.new(bytes.fromhex(key), (user + '\0' + password).encode(), hashlib.sha256).hexdigest()


# The site fields naming the operator's account and SSH public key, staged by install-persistent only
# (docs/metal/README.md, "Install and upgrade"): a public key line; a private key is refused.
OPERATOR_USER_FIELD = 'OPERATOR_USER'
OPERATOR_KEY_FIELD = 'OPERATOR_SSH_PUBLIC_KEY'
OPERATOR_ACCOUNT = '/etc/breenix/operator-account'


def operator_account(site):
    """The installer's operator file: `user <name>` and `key <line>`, from the site's account name and one checked
    ssh-ed25519 or ssh-rsa public key line."""
    user = site.get(OPERATOR_USER_FIELD, '').strip()
    if not re.fullmatch(r'[a-z_][a-z0-9_-]{0,31}', user) or user == 'root':
        raise ValueError(f'install-persistent needs {OPERATOR_USER_FIELD}: a lowercase account name other than root')
    line = site.get(OPERATOR_KEY_FIELD, '').strip()
    match = re.fullmatch(r'(ssh-ed25519|ssh-rsa) ([A-Za-z0-9+/]+={0,2})(?: ([\x21-\x7e][\x20-\x7e]*))?', line)
    if not match:
        raise ValueError(f'install-persistent needs {OPERATOR_KEY_FIELD}: one ssh-ed25519 or ssh-rsa public key line')
    try:
        blob = base64.b64decode(match.group(2), validate=True)
    except ValueError:
        raise ValueError(f'{OPERATOR_KEY_FIELD} is not valid base64') from None
    kind = match.group(1).encode()
    if blob[:4] != struct.pack('>I', len(kind)) or blob[4:4 + len(kind)] != kind:
        raise ValueError(f'{OPERATOR_KEY_FIELD} names {match.group(1)} but holds another key type')
    return f'user {user}\nkey {line}\n'


# The users suite's wheel fixture account (scripts/install-layout.sh), which a console boot logs in as once the image
# carries it: login refuses root (ROOT_LOGIN no), and the account reaches root through sudo or su.
CONSOLE_FIXTURE = 'uw'
CONSOLE_FIXTURE_HOME = '/etc/breenix/test-users/uw'


def console_account(passwd):
    """The account a console boot of an image with this passwd text logs in as: the wheel fixture when the image
    carries it, root for images from before the users effort's login milestone."""
    fields = [line.split(':') for line in passwd.splitlines()]
    present = any(len(f) == 7 and f[0] == CONSOLE_FIXTURE and f[5] == CONSOLE_FIXTURE_HOME for f in fields)
    return CONSOLE_FIXTURE if present else 'root'


def console_hash(site, hasher):
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
    return value


def console_shadow(site, hasher, image_shadow='', account='root'):
    """The image's shadow with the console password set for root (su's target) and for `account`, each with its
    own salt; the image's other lines are kept as they are."""
    console_identity(site)
    day = str(int(time.time() // 86400))
    staged = {name: name + ':' + console_hash(site, hasher) + ':' + day + ':0:99999:7:::' for name in ('root', account)}
    lines = [staged.pop(line.split(':')[0], line) for line in image_shadow.splitlines() if line]
    return '\n'.join(lines + list(staged.values())) + '\n'


def stage_root(image, mode, suite, site, writer=None, install=None, hasher=None, idle_minutes=None, replug=False, xhci_fault=None,
               install_partition=None, operator=None):
    """`operator`, the operator file (operator_account), is written for install-persistent's userland only."""
    if xhci_fault is not None and (xhci_fault not in ('reset-device', 'short-read')
                                   or mode != 'console' or install or idle_minutes is not None or replug):
        raise ValueError('invalid xHCI fault-test target')
    if mode not in ('default', 'shell', 'probe', 'suite', 'console', 'tests'):
        raise ValueError('unsupported RAM-disk boot mode')
    # The testing kernel has production init compiled out and does not read the boot target: its root is a default boot's.
    staged = 'default' if mode == 'tests' else mode
    # A list runs each suite in turn (docs/suites/README.md, "Several suites in one boot").
    if mode == 'suite' and not re.fullmatch(r'[a-z0-9]+(?:-[a-z0-9]+)*(?:,[a-z0-9]+(?:-[a-z0-9]+)*)*', suite or ''):
        raise ValueError('suite mode needs a suite id')
    first = suite.split(',')[0] if suite else None
    if mode != 'suite' and suite:
        raise ValueError('a suite needs suite mode')
    if idle_minutes is not None and (mode != 'console' or not 1 <= idle_minutes <= 240):
        raise ValueError('console idle must be 1 through 240 minutes')
    if replug and (mode != 'console' or idle_minutes is not None):
        raise ValueError('replug needs a measured console boot, without session idle options')
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
    debugfs, = require_e2fsprogs('debugfs')
    image = Path(image).resolve()
    # Default boots use the same getty login as explicit console boots.
    account = shadow = None
    if mode in ('default', 'console'):
        read = lambda path: subprocess.run([debugfs, '-R', f'cat {path}', str(image)], check=True,
                                           capture_output=True).stdout.decode('utf-8', errors='replace')
        account = console_account(read('/etc/passwd'))
        shadow = console_shadow(site, hasher, read('/etc/shadow'), account)
    packaged = Path(__file__).resolve().with_name('write-boot-target.sh')
    writer = Path(writer) if writer else (packaged if packaged.is_file() else
                                         Path(__file__).resolve().parents[1] / 'write-boot-target.sh')
    writer_hash = hashlib.sha256(writer.read_bytes()).hexdigest()
    target = first if mode == 'suite' else '--' + staged
    try:
        subprocess.run(['bash', str(writer), str(image), target], check=True, capture_output=True,
                       env=dict(os.environ, BREENIX_REQUIRE_DEBUGFS='1',
                                BREENIX_CONSOLE_IDLE_MINUTES='' if idle_minutes is None else str(idle_minutes),
                                BREENIX_CONSOLE_REPLUG='1' if replug else '0',
                                BREENIX_HOSTKEYS=HOSTKEYS if mode == 'suite' else ''))
    except subprocess.CalledProcessError as error:
        raise ValueError('RAM-disk boot-target staging failed') from error
    with tempfile.TemporaryDirectory(prefix='metal-root-') as scratch:
        scratch = Path(scratch)
        files = {'site.env': ''.join(lines)}
        # Replace this file so ordinary staging disarms a prior fault.
        files['xhci-fault'] = '' if xhci_fault is None else xhci_fault + '\n'
        # Several suites: the list beside the boot target (the writer removed any earlier one). Written here, by the
        # driver's own staging code, so tools deployed while a boot builds cannot change how it is staged.
        if mode == 'suite' and ',' in suite:
            files['suite-sequence'] = suite + '\n'
        commands = []
        for name, text in files.items():
            source = scratch / name
            source.write_text(text)
            commands.extend([f'rm /etc/breenix/{name}', f'write {source} /etc/breenix/{name}'])
        commands.extend(['rm /etc/resolv.conf', 'symlink /etc/resolv.conf /proc/net/pnp'])
        commands.append('rm ' + OPERATOR_ACCOUNT)
        if operator is not None:
            source = scratch / 'operator-account'
            source.touch(mode=0o600)
            source.write_text(operator)
            commands.extend([f'write {source} {OPERATOR_ACCOUNT}', f'set_inode_field {OPERATOR_ACCOUNT} mode 0100600',
                             f'set_inode_field {OPERATOR_ACCOUNT} uid 0', f'set_inode_field {OPERATOR_ACCOUNT} gid 0'])
        if shadow is not None:
            source = scratch / 'shadow'
            source.touch(mode=0o600)
            source.write_text(shadow)
            commands.extend(['mkdir /root', 'set_inode_field /root mode 040700',
                             'set_inode_field /root uid 0', 'set_inode_field /root gid 0',
                             'rm /etc/shadow', f'write {source} /etc/shadow',
                             'set_inode_field /etc/shadow mode 0100600',
                             'set_inode_field /etc/shadow uid 0', 'set_inode_field /etc/shadow gid 0'])
        script = scratch / 'commands'
        script.write_text('\n'.join(commands) + '\n')
        subprocess.run([debugfs, '-w', '-f', str(script), str(image)], check=True, capture_output=True)
        expected_target = ('suite ' + first + ' hostkeys ' + HOSTKEYS if mode == 'suite' else staged)
        if replug:
            expected_target += ' replug'
        if idle_minutes is not None:
            expected_target += f' idle={idle_minutes}'
        actual_target = subprocess.run([debugfs, '-R', 'cat /etc/breenix/boot-target', str(image)],
                                       check=True, capture_output=True).stdout
        if actual_target != (expected_target + '\n').encode():
            raise ValueError('RAM-disk boot target readback differs')
        for name, text in files.items():
            result = subprocess.run([debugfs, '-R', f'cat /etc/breenix/{name}', str(image)],
                                    check=True, capture_output=True)
            if result.stdout != text.encode():
                raise ValueError('RAM-disk boot configuration readback differs')
        if shadow is not None:
            home = subprocess.run([debugfs, '-R', 'stat /root', str(image)], check=True, capture_output=True).stdout
            if (b'Type: directory' not in home or b'Mode:  0700' not in home
                    or not re.search(rb'User:\s+0\s+Group:\s+0', home)):
                raise ValueError('console home directory or permissions differ')
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
        staged = subprocess.run([debugfs, '-R', 'cat ' + OPERATOR_ACCOUNT, str(image)], check=True, capture_output=True).stdout
        if staged != (operator or '').encode():
            raise ValueError('staged operator account readback differs')
        if operator is not None:
            stat = subprocess.run([debugfs, '-R', 'stat ' + OPERATOR_ACCOUNT, str(image)], check=True, capture_output=True).stdout
            if b'Mode:  0600' not in stat or not re.search(rb'User:\s+0\s+Group:\s+0', stat):
                raise ValueError('staged operator account permissions differ')
        resolver = subprocess.run([debugfs, '-R', 'stat /etc/resolv.conf', str(image)],
                                  check=True, capture_output=True).stdout
        if b'Type: symlink' not in resolver or b'"/proc/net/pnp"' not in resolver:
            raise ValueError('RAM-disk resolver does not name the live DHCP configuration')
        binaries = [f'/sbin/suite-{each}' for each in suite.split(',')] if mode == 'suite' else \
            [{'probe': '/sbin/probe'}.get(mode, '/sbin/init')]
        for binary in binaries:
            installed = scratch / 'pid1'
            installed.unlink(missing_ok=True)
            subprocess.run([debugfs, '-R', f'dump {binary} {installed}', str(image)],
                           check=True, capture_output=True)
            if not installed.is_file() or installed.read_bytes()[:4] != b'\x7fELF':
                raise ValueError('RAM disk has no selected PID 1 ELF')
    facts = {'mode': mode, 'suite': suite or None, 'siteFields': len(lines), 'writerSha256': writer_hash}
    if operator is not None:
        facts['operatorAccountSha256'] = hashlib.sha256(operator.encode()).hexdigest()
    if mode == 'console':
        facts['consoleIdentity'] = console_identity(site)
    if account is not None:
        facts['consoleAccount'] = account
    if install:
        facts['diskWrite'] = write_install_record(image, install, install_partition)
    return facts
