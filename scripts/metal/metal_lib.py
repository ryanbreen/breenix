"""Shared pieces of the x86-metal boot loop (docs/metal/README.md).

Everything private (the management controller's address and account, the boot URL, the site's identifiers) comes
from the site file named by BREENIX_METAL_SITE; nothing here carries a site value.
"""
import codecs
import contextlib
import fcntl
import ipaddress
import json
import os
from pathlib import Path
import re
import socket
import secrets
import subprocess
import time
from urllib.parse import urlsplit

SITE_PATH = Path(os.environ.get('BREENIX_METAL_SITE', '/etc/breenix-metal/site.env'))
STATE = Path(os.environ.get('BREENIX_METAL_STATE', '/var/lib/breenix-metal'))
RUN = Path(os.environ.get('BREENIX_METAL_RUN', '/run/breenix-metal'))
TOOLS = Path(__file__).resolve().parent

# Redfish resources of the server's management controller.
SYSTEM = '/redfish/v1/Systems/System.Embedded.1'
MANAGER = '/redfish/v1/Managers/iDRAC.Embedded.1'
CHASSIS = '/redfish/v1/Chassis/System.Embedded.1'
MANAGER_ATTRIBUTES = MANAGER + '/Oem/Dell/DellAttributes/iDRAC.Embedded.1'
LC_ATTRIBUTES = MANAGER + '/Oem/Dell/DellAttributes/LifecycleController.Embedded.1'

HEARTBEAT_SECONDS = 5
STALE_HEARTBEAT_SECONDS = 30
POWER_OFF_SECONDS = 60
DEFAULT_POST_BUDGET = 600
SECRET_KEYS = re.compile(r'(PASSWORD|SECRET|TOKEN|KEY)', re.I)

# Exit statuses (docs/metal/README.md, "Outcomes").
EXIT_DONE, EXIT_SHORT, EXIT_DELIVERY, EXIT_HUNG = 0, 1, 2, 124
EXIT_BUSY, EXIT_CANCELLED, EXIT_STOPPED, EXIT_INCOMPLETE = 75, 130, 143, 137


def load_site(path=None):
    """KEY=VALUE lines; values may be single- or double-quoted; # starts a comment line."""
    values = {}
    for raw in Path(path or SITE_PATH).read_text().splitlines():
        line = raw.strip()
        if not line or line.startswith('#') or '=' not in line:
            continue
        key, _, value = line.partition('=')
        value = value.strip()
        if len(value) >= 2 and value[0] == value[-1] and value[0] in '"\'':
            value = value[1:-1]
        values[key.strip()] = value
    return values


def power_token():
    """Private derivation seed mirrored into the install's root-only shutdown file."""
    STATE.mkdir(parents=True, exist_ok=True)
    path = STATE / 'power-token'
    try:
        fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    except FileExistsError:
        token = path.read_text()
    else:
        token = secrets.token_hex(32)
        with os.fdopen(fd, 'w') as handle:
            handle.write(token)
            handle.flush()
            os.fsync(handle.fileno())
    if not re.fullmatch(r'[a-f0-9]{64}', token) or path.stat().st_mode & 0o077:
        raise ValueError('invalid private power-control token')
    return token


def now():
    return time.time()


def write_json(path, value):
    """Whole-file replace by rename, so a reader never sees half a file."""
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name('.' + path.name + '.tmp')
    with temporary.open('w') as handle:
        json.dump(value, handle, indent=1, sort_keys=True)
        handle.write('\n')
        handle.flush()
        os.fsync(handle.fileno())
    os.replace(temporary, path)


def write_text(path, text):
    path = Path(path)
    temporary = path.with_name('.' + path.name + '.tmp')
    temporary.write_text(text)
    os.replace(temporary, path)


def read_json(path, default=None):
    try:
        return json.loads(Path(path).read_text())
    except (OSError, ValueError):
        return default


def append_jsonl(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open('a') as handle:
        handle.write(json.dumps(value, sort_keys=True) + '\n')


@contextlib.contextmanager
def locked(name):
    """An exclusive flock on STATE/<name>.lock for the duration of the block."""
    STATE.mkdir(parents=True, exist_ok=True)
    with (STATE / (name + '.lock')).open('a') as handle:
        fcntl.flock(handle, fcntl.LOCK_EX)
        try:
            yield
        finally:
            fcntl.flock(handle, fcntl.LOCK_UN)


# The machine lease: one holder at a time. The holder is a boot's driver unit, or a process (the one-time setup).

class Busy(Exception):
    pass


class Quarantined(Exception):
    pass


class NotReady(Exception):
    pass


LEASE = 'lease.json'


def read_lease():
    return read_json(STATE / LEASE)


def machine_ready():
    """The machine service has finished its startup reconciliation and is still the one that did it."""
    ready = read_json(RUN / 'ready.json')
    if not ready:
        return False
    try:
        os.kill(int(ready['pid']), 0)
    except (OSError, KeyError, ValueError):
        return False
    return True


def read_queue():
    return read_json(STATE / 'queue.json', [])


def write_queue(queue):
    """Called under lease.lock; mirror the queue beside the current holder for operators."""
    write_json(STATE / 'queue.json', queue)
    lease = read_lease()
    if lease:
        write_json(STATE / LEASE, dict(lease, queue=queue))


def claim_lease(holder, wait, poll=5.0, on_wait=None, checkpoint=None, wait_seconds=4 * 3600):
    """FIFO admission, bounded separately from the powered lease's ceiling.

    Dead or expired waiters cannot strand the queue. A cancellation removes only
    this request; the machine service alone reconciles a dead lease holder.
    """
    deadline = now() + wait_seconds
    announced = None
    try:
        while True:
            with locked('lease'):
                if checkpoint:
                    checkpoint()
                if not machine_ready():
                    raise NotReady('the machine service has not finished its startup reconciliation')
                lease = read_lease()
                if lease and lease.get('state') == 'quarantined':
                    raise Quarantined(lease.get('reason', 'quarantined'))
                queue = [entry for entry in read_queue()
                         if entry['deadline'] > now() and holder_alive(entry)]
                if now() >= deadline:
                    raise Busy('queue wait expired')
                if not wait and (lease or (queue and queue[0]['id'] != holder['id'])):
                    raise Busy((lease or queue[0]).get('id', 'another holder'))
                if not any(entry['id'] == holder['id'] for entry in queue):
                    queue.append(dict(holder, enqueued=now(), deadline=deadline))
                if not lease and queue[0]['id'] == holder['id']:
                    queue.pop(0)
                    write_queue(queue)
                    record = dict(holder, state='claimed', claimed=now(), heartbeat=now(), queue=queue)
                    write_json(STATE / LEASE, record)
                    return record
                write_queue(queue)
                position = next(i + 1 for i, entry in enumerate(queue) if entry['id'] == holder['id'])
                state = {'holder': (lease or {}).get('id'), 'position': position, 'deadline': deadline}
            if on_wait and state != announced:
                on_wait(state)
                announced = state
            time.sleep(min(poll, max(0, deadline - now())))
    finally:
        with locked('lease'):
            queue = read_queue()
            remaining = [entry for entry in queue if entry['id'] != holder['id']]
            if remaining != queue:
                write_queue(remaining)


def handoff_lease(holder_id, successor):
    """Transfer an owned, non-quarantined lease without exposing a free-machine interval."""
    with locked('lease'):
        lease = read_lease()
        if not lease or lease.get('id') != holder_id or lease.get('state') == 'quarantined':
            return False
        write_json(STATE / LEASE, dict(successor, state='claimed', claimed=now(), heartbeat=now(), queue=read_queue()))
        return True


def update_lease(holder_id, **fields):
    """Refreshes the heartbeat (and any fields) of a lease `holder_id` still holds; False when it no longer does."""
    with locked('lease'):
        lease = read_lease()
        if not lease or lease.get('id') != holder_id:
            return False
        if lease.get('state') == 'quarantined' and fields.get('state') not in (None, 'quarantined'):
            fields.pop('state')
        lease.update(fields, heartbeat=now())
        write_json(STATE / LEASE, lease)
        return True


def release_lease(holder_id):
    with locked('lease'):
        lease = read_lease()
        if lease and lease.get('id') == holder_id and lease.get('state') != 'quarantined':
            (STATE / LEASE).unlink()
            return True
    return False


def quarantined_record(lease, holder_id, reason):
    record = dict(lease or {'id': holder_id})
    record.update(state='quarantined', reason=reason, quarantined=now(), heartbeat=now(), retries=0)
    return record


def quarantine_lease(holder_id, reason):
    with locked('lease'):
        write_json(STATE / LEASE, quarantined_record(read_lease(), holder_id, reason))


# The management controller: Redfish and IPMI requests go through the machine service's queue.

class BmcError(Exception):
    pass


def bmc(request, timeout=180):
    """Sends one request to the machine service's queue and returns its reply (a dict with `ok`)."""
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(timeout)
        try:
            connection.connect(str(RUN / 'bmc.sock'))
            connection.sendall(json.dumps(request).encode() + b'\n')
            data = b''
            while not data.endswith(b'\n'):
                chunk = connection.recv(65536)
                if not chunk:
                    break
                data += chunk
        except OSError as error:
            raise BmcError(f'machine service unreachable: {error.__class__.__name__}') from error
    if not data:
        raise BmcError('machine service closed the connection')
    return json.loads(data)


def rf(method, path, body=None, priority=1, timeout=180):
    reply = bmc({'op': 'redfish', 'method': method, 'path': path, 'body': body, 'priority': priority}, timeout)
    if not reply.get('ok'):
        raise BmcError(reply.get('error') or f"HTTP {reply.get('status')}")
    return reply


def rf_get(path, **options):
    return rf('GET', path, **options).get('body') or {}


def ipmi(arguments, priority=1):
    return bmc({'op': 'ipmi', 'args': list(arguments), 'priority': priority})


def power_state(priority=1):
    return rf_get(SYSTEM + '?$select=PowerState', priority=priority).get('PowerState')


def force_off_and_wait(seconds=None):
    """ForceOff, then poll until PowerState reads Off. True when it did within `seconds` (default 60)."""
    deadline = now() + (POWER_OFF_SECONDS if seconds is None else seconds)
    try:
        if power_state(priority=0) == 'Off':
            return True
        rf('POST', SYSTEM + '/Actions/ComputerSystem.Reset', {'ResetType': 'ForceOff'}, priority=0)
    except BmcError:
        pass
    while now() < deadline:
        try:
            if power_state(priority=0) == 'Off':
                return True
        except BmcError:
            pass
        time.sleep(2)
    return False


def clear_override():
    try:
        boot = rf_get(SYSTEM + '?$select=Boot').get('Boot', {})
        if boot.get('BootSourceOverrideEnabled') not in (None, 'Disabled'):
            rf('PATCH', SYSTEM, {'Boot': {'BootSourceOverrideEnabled': 'Disabled'}})
        return True
    except BmcError:
        return False


def sol_deactivate():
    # "not active" is the normal answer when no session is open.
    reply = ipmi(['sol', 'deactivate'])
    return reply.get('ok') or 'not active' in (reply.get('output') or '').lower()


# Staging: the HTTP server serves STATE/stage/current.json's file, once.

def stage_dir(boot_id):
    return STATE / 'stage' / boot_id


def stage(boot_id, path, sha256, size, content_type, redirect=None):
    """Stages one file for the route. With `redirect` (a path beside the route, such as /breenix/boot.iso), the route
    answers 302 to that path and the file is served there: the URI's file name can then match the image form."""
    with locked('stage'):
        write_json(STATE / 'stage' / 'current.json', {'id': boot_id, 'path': str(path), 'sha256': sha256,
                                                      'size': size, 'contentType': content_type,
                                                      'redirect': redirect, 'served': False, 'staged': now()})


def unstage(boot_id=None):
    """Removes the staged entry (only `boot_id`'s, when given). The route then answers 404."""
    with locked('stage'):
        current = read_json(STATE / 'stage' / 'current.json')
        if current and (boot_id is None or current.get('id') == boot_id):
            (STATE / 'stage' / 'current.json').unlink()


# Boot records: STATE/boots/<id>/, in Vigil's folder layout.

def boot_dir(boot_id):
    return STATE / 'boots' / boot_id


def event(directory, kind, *, observed_at=None, **fields):
    stamp = now() if observed_at is None else observed_at
    append_jsonl(Path(directory) / 'events.jsonl', dict(fields, t=round(stamp, 3), event=kind))


def phase(directory, text):
    with (Path(directory) / 'console.log').open('a') as handle:
        handle.write('==> ' + text + '\n')


def close_record(directory, status, ended_by):
    """Writes exit-status, then ended, then the finished.jsonl entry. A record is closed once; False if it was."""
    directory = Path(directory)
    with locked('finished'):
        if (directory / 'ended').exists():
            return False
        event(directory, 'closed', status=status, endedBy=ended_by)
        write_text(directory / 'ended-by', ended_by + '\n')
        write_text(directory / 'exit-status', f'{status}\n')
        write_text(directory / 'ended', f'{int(now())}\n')
        finished = STATE / 'finished.jsonl'
        seq = 0
        if finished.exists():
            for line in finished.read_text().splitlines():
                with contextlib.suppress(ValueError, KeyError):
                    seq = max(seq, int(json.loads(line)['seq']))
        append_jsonl(finished, {'seq': seq + 1, 'id': directory.name})
    return True


def open_records():
    boots = STATE / 'boots'
    if not boots.is_dir():
        return []
    return sorted(path for path in boots.iterdir() if path.is_dir() and not (path / 'ended').exists())


def unit_active(unit):
    result = subprocess.run(['systemctl', 'is-active', '--quiet', unit])
    return result.returncode == 0


def holder_alive(lease):
    if lease.get('unit'):
        return unit_active(lease['unit'])
    if lease.get('pid'):
        try:
            os.kill(int(lease['pid']), 0)
            return True
        except (OSError, ValueError):
            return False
    return False


def post_budget():
    """Ten minutes until three power-on-to-request intervals are measured; then twice the longest."""
    times = read_json(STATE / 'post-times.json', [])
    if len(times) < 3:
        return DEFAULT_POST_BUDGET
    return 2 * max(entry['seconds'] for entry in times)


def record_post_time(boot_id, seconds):
    with locked('post-times'):
        times = read_json(STATE / 'post-times.json', [])
        times.append({'id': boot_id, 'seconds': round(seconds, 1)})
        write_json(STATE / 'post-times.json', times)


# Serial: bytes as received become serial.raw; text with terminal control sequences removed becomes serial.txt.

class SerialCleaner:
    """VT100/ANSI sequences and NULs removed, CRLF and lone CR made LF, cursor positioning made a line break."""

    def __init__(self):
        self.decoder = codecs.getincrementaldecoder('utf-8')('replace')
        self.state = 'text'
        self.sequence = ''
        self.line = ''
        self.pending_cr = False
        self.broke = False

    def feed(self, data):
        out = []
        for char in self.decoder.decode(data):
            out.append(self._char(char))
        return ''.join(out)

    def _newline(self):
        self.line = ''
        return '\n'

    def _break(self):
        # A break made by cursor movement absorbs the newline that may follow it.
        if not self.line:
            return ''
        self.broke = True
        return self._newline()

    def _char(self, char):
        if self.state == 'esc':
            if char == '[':
                self.state, self.sequence = 'csi', ''
            elif char == ']':
                self.state = 'osc'
            elif char in '()*+':
                self.state = 'charset'
            else:
                self.state = 'text'
            return ''
        if self.state == 'charset':
            self.state = 'text'
            return ''
        if self.state == 'osc':
            if char == '\x07' or char == '\x1b':
                self.state = 'text'
            return ''
        if self.state == 'csi':
            if '\x40' <= char <= '\x7e':
                self.state = 'text'
                if char in 'Hf' or (char == 'J' and self.sequence in ('', '2')):
                    return self._break()
                return ''
            self.sequence += char
            return ''
        out = ''
        if self.pending_cr:
            self.pending_cr = False
            if char != '\n':
                out = self._break()
        if char == '\x1b':
            self.state = 'esc'
            return out
        if char == '\r':
            self.pending_cr = True
            return out
        if char == '\n':
            if self.broke:
                self.broke = False
                return out
            return out + self._newline()
        self.broke = False
        if char == '\b':
            self.line = self.line[:-1]
            return out
        if char == '\t' or char >= ' ':
            if char == '\x7f':
                return out
            self.line += char
            return out + char
        return out


# Redaction: applied wherever text leaves the private record (phone payloads, notifications, PR excerpts).

IPV4 = re.compile(r'(?<![\w.])(?:\d{1,3}\.){3}\d{1,3}(?![\w.]*\d)')
MAC = re.compile(r'(?<![\w:-])[0-9A-Fa-f]{2}(?:([:-])[0-9A-Fa-f]{2})(?:\1[0-9A-Fa-f]{2}){4}(?![\w:-])')
IPV6 = re.compile(r'(?<![\w:])(?:[0-9A-Fa-f]{0,4}:){2,7}[0-9A-Fa-f]{0,4}(?:%\w+)?(?![\w:])')
PRIVATE_PATH = re.compile(r'(?:/Users/[^/\s]+|/home/[^/\s]+|/root|/var/lib/breenix-metal|/etc/breenix-metal'
                          r'|/run/breenix-metal|/private/tmp|/tmp/breenix-metal[^/\s]*)(?:/[^\s"\'<>]*)?')


def _ipv6(match):
    text = match.group(0)
    core = text.split('%')[0]
    # Clock times (12:34:56) and ratios have at most two colons and no '::'; require the forms addresses take.
    if '::' not in core and core.count(':') < 7:
        return text
    try:
        ipaddress.IPv6Address(core)
    except ValueError:
        return text
    return '<ip6>'


def _ipv4(match):
    try:
        ipaddress.IPv4Address(match.group(0))
    except ValueError:
        return match.group(0)
    return '<ip>'


# Site fields kept out of the literals: the console account (root) and the operator's uid (the first ordinary
# account's number), which would otherwise mask ordinary text such as "1000 ms".
NAMES_NOTHING = ('CONSOLE_USER', 'OPERATOR_UID')


class Redactor:
    def __init__(self, site):
        self.identifiers = sorted(identifier_list(site), key=len, reverse=True)
        # Short configuration values (slots, switches, counters) do not name
        # a private entity. Explicit identifiers and credentials still count.
        literals = {value for key, value in site.items()
                    if key not in NAMES_NOTHING and value and (len(value) >= 4 or SECRET_KEYS.search(key) or value in self.identifiers)}
        for value in list(literals):
            # The complete URL, host and path are site literals independently.
            if '://' in value:
                parts = urlsplit(value)
                literals.update(part for part in (parts.netloc, parts.hostname, parts.path, *parts.path.split("/"))
                                if part and part != "/")
        self.literals = sorted(literals | set(self.identifiers), key=len, reverse=True)
        # Match site values as complete tokens, preserving clock times and hashes.
        # Keep punctuation inside a token (host names, paths and credential strings included).
        secrets = {value for key, value in site.items() if SECRET_KEYS.search(key) and value}
        patterns = []
        for value in self.literals:
            if value in secrets and len(value) >= 4:
                patterns.append(rf'(?<!\w){re.escape(value)}(?!\w)')
            else:
                # Colons delimit named identifiers (host:port, user:name),
                # but join digits in clock times and ratios.
                joiner = r'[./:@%+~-]' if len(value) < 4 else r'[./@%+~-]'
                patterns.append(rf'(?<!\w)(?<!\w{joiner}){re.escape(value)}(?!\w|{joiner}\w)')
        self.pattern = re.compile('|'.join(patterns), re.I) if patterns else None
        # One pass protects addresses from short site literals and replacement labels from later substitutions.
        mac = MAC.pattern.replace('([:-])', '(?P<mac_sep>[:-])').replace(r'\1', '(?P=mac_sep)')
        patterns = [('path', PRIVATE_PATH.pattern), ('mac', mac), ('ip6', IPV6.pattern), ('ip', IPV4.pattern)]
        if self.pattern:
            patterns.append(('site', self.pattern.pattern))
        self.tokens = re.compile('|'.join(f'(?P<{kind}>{pattern})' for kind, pattern in patterns), re.I)

    def __call__(self, text):
        def replace(match):
            kind, value = match.lastgroup, match.group()
            if kind == 'site' or value.lower() in {literal.lower() for literal in self.literals}:
                return '<site>'
            if kind in ('path', 'mac'):
                return '<' + kind + '>'
            address = _ipv6(match) if kind == 'ip6' else _ipv4(match)
            if address != value:
                return address
            return self.pattern.sub('<site>', value) if self.pattern else value
        return self.tokens.sub(replace, text)


def identifier_list(site):
    """Names to mask that are not secrets: the listed identifiers plus non-secret site values that name things."""
    names = set(re.split(r'[\s,]+', site.get('REDACT_IDENTIFIERS', '').strip())) - {''}
    for key, value in site.items():
        if key not in ('REDACT_IDENTIFIERS', 'CONSOLE_USER') and value and not SECRET_KEYS.search(key) and re.search(r'[A-Za-z]', value) \
                and key.endswith(('_USER', '_HOST', '_NAME')):
            names.add(value)
    return names


# The firmware settings the boot loop depends on (docs/metal/README.md, "One-time setup"). Values that differ by
# site (the boot URL, which port the HTTP device uses, how it gets its address) come from the site file.

def expected_bios(site):
    expected = {
        'HttpDev1EnDis': 'Enabled', 'HttpDev1Interface': site['HTTP_DEV_INTERFACE'], 'HttpDev1Protocol': 'IPv4',
        'HttpDev1VlanEnDis': 'Disabled', 'HttpDev1TlsMode': 'None', 'HttpDev1Uri': site['BOOT_URL'],
        'UsbPorts': 'AllOn', 'GenericUsbBoot': 'Disabled', 'EmbVideo': 'Enabled',
        'InternalUsb': site['INTERNAL_USB'], 'UsbManagedPort': site['USB_MANAGED_PORT'],
        'SerialComm': 'OnConRedirCom1', 'ErrPrompt': 'Disabled', 'AcPwrRcvry': 'Off', 'EmbSata': site.get('SATA_MODE', 'AhciMode'),
    }
    if site.get('HTTP_DEV_ADDRESSING', 'dhcp') == 'static':
        expected.update(HttpDev1DhcpEnDis='Disabled', HttpDev1DnsDhcpEnDis='Disabled', HttpDev1Ip=site['HTTP_DEV_IP'],
                        HttpDev1Mask=site['HTTP_DEV_MASK'], HttpDev1Gateway=site['HTTP_DEV_GATEWAY'])
    else:
        expected.update(HttpDev1DhcpEnDis='Enabled', HttpDev1DnsDhcpEnDis='Enabled')
    return expected


# Kept as they are and checked before each boot.
KEPT_BIOS = {'BootMode': 'Uefi', 'SecureBoot': 'Disabled', 'SerialPortAddress': 'Serial1Com2Serial2Com1',
             'RedirAfterBoot': 'Enabled', 'BootSeqRetry': 'Enabled', 'ProcX2Apic': 'Enabled'}
EXPECTED_MANAGER = {'IPMILan.1.Enable': 'Enabled', 'IPMISOL.1.Enable': 'Enabled', 'IPMISOL.1.BaudRate': '115200',
                    'CurrentNIC.1.Selection': 'Dedicated'}
EXPECTED_LC = {'LCAttributes.1.CollectSystemInventoryOnRestart': 'Disabled'}


def machine_checks(site, include_lc=True):
    """Read-only: every setting and state a boot depends on, as (name, expected, actual) rows, plus the facts
    a boot records as its baseline (power state, boot progress, POST code)."""
    system = rf_get(SYSTEM)
    bios = rf_get(SYSTEM + '/Bios').get('Attributes', {})
    manager = rf_get(MANAGER_ATTRIBUTES).get('Attributes', {})
    port = rf_get(SYSTEM + '/EthernetInterfaces/' + site['HTTP_DEV_INTERFACE'])
    rows = []
    for name, value in dict(expected_bios(site), **KEPT_BIOS).items():
        rows.append(('bios.' + name, value, bios.get(name)))
    for name, value in EXPECTED_MANAGER.items():
        rows.append(('manager.' + name, value, None if manager.get(name) is None else str(manager.get(name))))
    # The port has no standby link while the machine is off; its link is checked only with the machine on,
    # and shown during delivery by the firmware's image request.
    if system.get('PowerState') == 'On':
        rows.append(('port.LinkStatus', 'LinkUp', port.get('LinkStatus')))
    rows.append(('boot.BootSourceOverrideEnabled', 'Disabled', (system.get('Boot') or {}).get('BootSourceOverrideEnabled')))
    if include_lc:
        lc = rf_get(LC_ATTRIBUTES).get('Attributes', {})
        for name, value in EXPECTED_LC.items():
            rows.append(('lc.' + name, value, lc.get(name)))
    baseline = {'power': system.get('PowerState'), 'bootProgress': system.get('BootProgress'),
                'postCode': manager.get('SysInfo.1.POSTCode'), 'portLink': port.get('LinkStatus')}
    return rows, baseline


def failed_checks(rows):
    return [row for row in rows if row[1] != row[2]]
