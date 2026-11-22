"""The transfer test boot's staged inputs and nonce-bound evidence."""
import ipaddress
import re
import subprocess
import tempfile
from pathlib import Path
from urllib.parse import urlsplit

SUITE = 'transfer-fixture'
ARTIFACT = 'transfer-artifact'
RECEIPT = 'server-transfer.json'
SETTINGS = '/etc/breenix/test-ssh/transfer'
ELF = '/sbin/suite-transfer-fixture'

READY = re.compile(r'TRANSFER READY nonce=([0-9a-f]{32}) fp=(SHA256:[A-Za-z0-9+/]{43})'
                   r' peer=([0-9.]+) user=([a-z_][a-z0-9_-]{0,31}) uid=([1-9][0-9]*) gid=([1-9][0-9]*)')
ENDED = re.compile(r'TRANSFER LEASE-END nonce=([0-9a-f]{32}) reason=(done|timeout) refused=(\d+)')


def selected(suites):
    return SUITE in (suites or '').split(',')


def metal_peer(site):
    # The HTTP boot endpoint is on the boot host. Sites whose boot service
    # has a separate address can name the OpenSSH client's source explicitly.
    address = site.get('TRANSFER_SSH_PEER') or urlsplit(site['BOOT_URL']).hostname
    peer = ipaddress.IPv4Address(address)
    if peer.is_unspecified or peer.is_loopback or peer.is_multicast or peer.is_reserved:
        raise ValueError('transfer peer needs the boot host LAN IPv4 address')
    return str(peer)


def stage(image, debugfs, *, binary=None, settings=None):
    """Write the selected ELF or private site settings and verify the ext2 readback."""
    entries = []
    with tempfile.TemporaryDirectory(prefix='transfer-stage-') as temporary:
        scratch = Path(temporary)
        if binary is not None:
            binary = Path(binary).resolve(strict=True)
            with binary.open('rb') as source:
                if source.read(4) != b'\x7fELF':
                    raise ValueError('transfer fixture binary must be an ELF')
            entries.append((binary, ELF, '0100755'))
        if settings is not None:
            config = scratch / 'settings'
            config.write_text(settings)
            entries.append((config, SETTINGS, '0100600'))
        commands = scratch / 'commands'
        commands.write_text(''.join(f'rm {dest}\nwrite "{source}" {dest}\nset_inode_field {dest} mode {mode}\n'
                                    f'set_inode_field {dest} uid 0\nset_inode_field {dest} gid 0\n'
                                    for source, dest, mode in entries))
        subprocess.run([debugfs, '-w', '-f', str(commands), str(image)], check=True, capture_output=True)
        for source, dest, mode in entries:
            readback = subprocess.run([debugfs, '-R', f'cat {dest}', str(image)], check=True, capture_output=True).stdout
            stat = subprocess.run([debugfs, '-R', f'stat {dest}', str(image)], check=True, capture_output=True).stdout
            if (readback != source.read_bytes() or ('Mode:  ' + mode[-4:]).encode() not in stat
                    or not re.search(rb'User:\s+0\s+Group:\s+0', stat)):
                raise ValueError(f'transfer fixture staging readback differs for {dest}')


def identity(boot, sha):
    """S22: trust the build identity validated by the driver and its tools commit."""
    suites = boot.get('suites') or (boot.get('suite') or '').split(',')
    if (boot.get('mode') != 'suite' or SUITE not in suites or boot.get('evidenceEligible') is not True
            or boot.get('session') or boot.get('watch') or boot.get('onlyCase') or boot.get('inject')
            or boot.get('kind') in ('persistent', 'fault-test', 'console-session')):
        raise ValueError('transfer needs an eligible, bounded transfer-fixture suite boot')
    if not re.fullmatch(r'[a-f0-9]{40}', sha or '') or boot.get('toolsCommit') != sha:
        raise ValueError('transfer tools commit differs from the requested commit')
    build = boot.get('build', {})
    if build.get('commit') != sha or build.get('profile') != 'production':
        raise ValueError('transfer image commit or production profile differs from the retained build')
    facts = boot.get('transferInput')
    if (not isinstance(facts, dict) or not re.fullmatch(r'[0-9a-f]{64}', facts.get('sha256', ''))
            or type(facts.get('size')) is not int or facts['size'] <= 0):
        raise ValueError('transfer boot did not retain a hashed input')
    return facts


def score(boot, sha, records, serial, fatal_patterns, case_table):
    facts = identity(boot, sha)
    if records.get('boot') != boot['id'] or records.get('commit') != sha:
        raise ValueError('transfer evidence belongs to another boot or commit')
    if records.get('artifactSha256') != facts['sha256']:
        raise ValueError('transfer input digest differs from the launch record')
    rows = serial.split('\n')[:-1]  # A fragment at the end of the capture is not a receipt.
    ready = [(index, READY.fullmatch(row.rstrip('\r'))) for index, row in enumerate(rows)
             if row.startswith('TRANSFER READY ')]
    ended = [(index, ENDED.fullmatch(row.rstrip('\r'))) for index, row in enumerate(rows)
             if row.startswith('TRANSFER LEASE-END ')]
    if (len(ready) != 1 or not ready[0][1] or records.get('ready') is not True
            or ready[0][1].group(1, 2, 3) != tuple(records.get(key) for key in ('nonce', 'fingerprint', 'peer'))):
        raise ValueError('transfer readiness does not match the authenticated host record')
    if (len(ended) != 1 or not ended[0][1] or ended[0][0] <= ready[0][0]
            or ended[0][1][1] != records['nonce']):
        raise ValueError('matching transfer lease-end record is missing')
    if int(ended[0][1][3]):
        raise ValueError('a connection from a non-allowed peer was refused during the transfer lease')
    if ended[0][1][2] != 'done':
        raise ValueError('transfer lease ended without host completion')
    if records.get('consoleGaps') or any(re.search(pattern, serial) for pattern in fatal_patterns):
        raise ValueError('transfer boot has a console gap or fatal kernel output')
    cases = records.get('cases')
    if (not isinstance(cases, list) or not all(isinstance(row, dict) for row in cases)
            or [row.get('id') for row in cases] != ['transfer/' + case for case, _ in case_table]):
        raise ValueError('transfer results are missing, duplicated or out of order')
    for row in cases:
        if row.get('result') != 'PASS':
            raise ValueError(f"{row['id']} failed: {row.get('reason', 'no passing result')}")
    first, _, interrupted, _, _ = cases
    user, uid, gid = ready[0][1].group(4, 5, 6)
    if (first.get('executedSha256'), first.get('uid'), first.get('gid'), first.get('home'),
            first.get('status')) != (facts['sha256'], int(uid), int(gid), f'/home/{user}', 42):
        raise ValueError('executed artifact or SSH identity differs from the boot inputs')
    progress = interrupted.get('interruptedAfter')
    if (type(progress) is not int or not 0 < progress <= 3
            or interrupted.get('publishedSha256') != facts['sha256']):
        raise ValueError('interrupted upload lacks progress or verified reconnect evidence')
    return 'PASS: server/transfer (5/5)'
