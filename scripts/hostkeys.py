#!/usr/bin/env python3
"""Host key injection for suites that need a key the guest cannot press itself.

A suite case asks for keys on its serial line (docs/console/contract.md, "Host-injected keys"):

    SUITE <suite> HOSTKEY <category>/<case> seq=<n> keys=<chord>[,<chord>...]

`seq` increases across the run and each seq is injected once, so a replayed line is
harmless. A chord is modifiers then one key, joined by '-':

    modifiers  ctrl alt shift super rctrl ralt
    keys       a-z 0-9 f1-f12 left right up down pageup pagedown home end insert
               delete enter tab backspace esc space capslock
    examples   alt-f3  ctrl-alt-f2  alt-left  alt-right  alt-f7  shift-pageup

Chords go out in order, each pressed modifiers first and released in reverse.
The runner watching the serial log presses the chord on the target's own virtual
keyboard and the case reads the screen back. The runner never reports success: the
evidence is in the guest. A line that is not at the start of a serial line (the
kernel's announcement lines) is never a request.

Backends, one per way the target takes keys:

    qmp:<socket>          QEMU's QMP send-key (ARM64 and x86-64 QEMU)
    prlctl:<vm>           prlctl send-key-event with PS/2 set-1 scancodes (Parallels)
    vnc:<host>:<port>     RFB key events (VMware Fusion's built-in VNC server)
    none                  nothing: every request is answered "no host key path"

Earlier Parallels (27.0.3) probe: basic scancodes worked. Alt+Fn, Ctrl+Alt+Fn and letters reached the
guest and the shell showed the typed letters. Arrows, Home/End and Page Up/Down do not:
`prlctl` refuses a bare extended code ("Invalid scan code sequence"), and the encodings
it does accept (a 224 prefix event, the 16-bit code 0xE04B, `--key` with several
values) left the shell's line editor, which moves its cursor on Left, where it was,
while QMP's `left` moved it. So Alt+Left, Alt+Right and Shift+PageUp cases have no host
key path on Parallels; they run on QEMU, and the Alt+Fn cases run on both.

Earlier VMware Fusion probe (see contract section 10 for limits): add `RemoteDisplay.vnc.enabled = "TRUE"` and `RemoteDisplay.vnc.port`
to a boot VM's vmx (run.sh does so when BREENIX_VMWARE_VNC_PORT names a port). Fusion
on Apple Silicon starts the VNC server (no password with `useKey 0`), the RFB handshake
and framebuffer updates work and the key events are accepted. That a key reached the
guest could not be shown: the guest's xHCI controller fails its first command on VMware
("XHCI command IRQ unavailable"), so the guest has no working keyboard there until the
ARM64 xHCI port (V7). Until then the VMware `keys` cases answer "no host key path" (the
watcher on a VMware boot without a VNC port) and V7 re-runs the self-test
(scripts/hostkeys-selftest.sh vnc:127.0.0.1:<port> <serial log>). The server listens on
all interfaces for the length of the boot, so use the variable only on a trusted network.

Usage:
    hostkeys.py watch --serial FILE --backend SPEC [--log FILE] [--suite ID] [--parent PID]
    hostkeys.py send --backend SPEC CHORD
    hostkeys.py declare --backend SPEC      (the contract's hostkeys declaration string)
    hostkeys.py selftest-parse
"""
import argparse
import json
import os
import re
import socket
import struct
import subprocess
import sys
import time

REQUEST = re.compile(r'^SUITE ([a-z0-9]+(?:-[a-z0-9]+)*) HOSTKEY ([a-z0-9]+(?:-[a-z0-9]+)*/[a-z0-9]+(?:-[a-z0-9]+)*)'
                     r' seq=(\d+) keys=([a-z0-9]+(?:-[a-z0-9]+)*(?:,[a-z0-9]+(?:-[a-z0-9]+)*)*)\r?$')

POINTER_REQUEST = re.compile(r'^USB POINTER HOST seq=(\d+) x=(\d+) y=(\d+) buttons=([0-7]) wheel=(-?1|0)\r?$')

MODIFIERS = ('ctrl', 'alt', 'shift', 'super', 'rctrl', 'ralt')
NAMED = ('left', 'right', 'up', 'down', 'pageup', 'pagedown', 'home', 'end', 'insert', 'delete', 'enter', 'tab',
         'backspace', 'esc', 'space', 'capslock')
# Keys the PS/2 set-1 table sends behind an 0xE0 prefix, which Parallels will not take.
EXTENDED = ('left', 'right', 'up', 'down', 'pageup', 'pagedown', 'home', 'end', 'insert', 'delete',
            'super', 'rctrl', 'ralt')


def parse_chord(spec):
    """Return (modifiers, key) in a fixed modifier order, or raise ValueError."""
    parts = spec.split('-')
    key = parts[-1]
    mods = parts[:-1]
    if len(set(mods)) != len(mods) or any(m not in MODIFIERS for m in mods):
        raise ValueError('bad modifiers in ' + repr(spec))
    valid = (re.fullmatch(r'f([1-9]|1[0-2])', key) or re.fullmatch(r'[a-z0-9]', key) or key in NAMED)
    if not valid:
        raise ValueError('unknown key in ' + repr(spec))
    return tuple(m for m in MODIFIERS if m in mods), key


class NoPath(Exception):
    """The backend cannot press this chord."""


class QMPKeys:
    QCODES = {'ctrl': 'ctrl', 'alt': 'alt', 'shift': 'shift', 'super': 'meta_l', 'rctrl': 'ctrl_r', 'ralt': 'alt_r',
              'enter': 'ret', 'esc': 'esc', 'tab': 'tab', 'backspace': 'backspace', 'space': 'spc',
              'capslock': 'caps_lock', 'left': 'left', 'right': 'right', 'up': 'up', 'down': 'down',
              'pageup': 'pgup', 'pagedown': 'pgdn', 'home': 'home', 'end': 'end', 'insert': 'insert',
              'delete': 'delete'}

    def __init__(self, path, hold_ms=100):
        self.path, self.hold_ms = path, hold_ms

    @classmethod
    def keys(cls, mods, key):
        return [{'type': 'qcode', 'data': cls.QCODES.get(k, k)} for k in (*mods, key)]

    def send(self, mods, key):
        qcodes = [k['data'] for k in self.keys(mods, key)]
        # One connection per chord: QEMU serves one QMP client at a time, and Vigil or a screen
        # grab may want the socket between chords.
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.settimeout(0.8)
        try:
            sock.connect(self.path)
            stream = sock.makefile('rw')
            json.loads(stream.readline())
            for command, arguments in (('qmp_capabilities', {}),
                                       ('send-key', {'keys': [{'type': 'qcode', 'data': q} for q in qcodes],
                                                     'hold-time': self.hold_ms})):
                stream.write(json.dumps({'execute': command, 'arguments': arguments}) + '\n')
                stream.flush()
                while True:
                    reply = json.loads(stream.readline())
                    if 'error' in reply:
                        raise RuntimeError('QMP ' + command + ': ' + str(reply['error']))
                    if 'return' in reply:
                        break
        finally:
            sock.close()


class PrlctlKeys:
    # PS/2 set 1 make codes; Parallels turns them into USB HID usages.
    CODES = {'ctrl': 29, 'alt': 56, 'shift': 42, 'esc': 1, 'tab': 15, 'enter': 28, 'space': 57, 'capslock': 58,
             'backspace': 14}

    def __init__(self, vm, hold_ms=60):
        self.vm, self.hold_ms = vm, hold_ms

    @classmethod
    def scancode(cls, key):
        if key in cls.CODES:
            return cls.CODES[key]
        if key in EXTENDED:
            raise NoPath(key + ' is an extended scancode, which Parallels rejects')
        match = re.fullmatch(r'f(\d+)', key)
        if match:
            n = int(match.group(1))
            return 58 + n if n <= 10 else 76 + n
        if key.isdigit():
            return 11 if key == '0' else 1 + int(key)
        rows = {'q': 16, 'w': 17, 'e': 18, 'r': 19, 't': 20, 'y': 21, 'u': 22, 'i': 23, 'o': 24, 'p': 25,
                'a': 30, 's': 31, 'd': 32, 'f': 33, 'g': 34, 'h': 35, 'j': 36, 'k': 37, 'l': 38,
                'z': 44, 'x': 45, 'c': 46, 'v': 47, 'b': 48, 'n': 49, 'm': 50}
        return rows[key]

    def send(self, mods, key):
        for name in (*mods, key):
            if name in EXTENDED:
                raise NoPath(name + ' is an extended scancode, which Parallels rejects')
        codes = [self.CODES[m] for m in mods] + [self.scancode(key)]
        events = [{'scancode': c, 'event': 'press'} for c in codes]
        events.append({'delay': self.hold_ms})
        events += [{'scancode': c, 'event': 'release'} for c in reversed(codes)]
        result = subprocess.run(['prlctl', 'send-key-event', self.vm, '-j'], input=json.dumps(events),
                                text=True, capture_output=True, timeout=30)
        if result.returncode != 0 or result.stderr.strip():
            raise RuntimeError('prlctl send-key-event rc=%d: %s' % (result.returncode, result.stderr.strip()))


class VNCKeys:
    SYMS = {'ctrl': 0xFFE3, 'alt': 0xFFE9, 'shift': 0xFFE1, 'super': 0xFFEB, 'rctrl': 0xFFE4, 'ralt': 0xFFEA,
            'capslock': 0xFFE5, 'enter': 0xFF0D, 'esc': 0xFF1B, 'tab': 0xFF09, 'backspace': 0xFF08, 'space': 0x20,
            'left': 0xFF51, 'up': 0xFF52, 'right': 0xFF53, 'down': 0xFF54, 'pageup': 0xFF55, 'pagedown': 0xFF56,
            'home': 0xFF50, 'end': 0xFF57, 'insert': 0xFF63, 'delete': 0xFFFF}

    def __init__(self, host, port, hold_ms=100):
        self.host, self.port, self.hold_ms = host, int(port), hold_ms

    @classmethod
    def keysym(cls, key):
        if key in cls.SYMS:
            return cls.SYMS[key]
        match = re.fullmatch(r'f(\d+)', key)
        if match:
            return 0xFFBD + int(match.group(1))
        return ord(key)

    def connect(self):
        sock = socket.create_connection((self.host, self.port), timeout=10)
        banner = self.read(sock, 12)
        if not banner.startswith(b'RFB 003.'):
            raise RuntimeError('not an RFB server: ' + repr(banner))
        minor = int(banner[8:11])
        sock.sendall(b'RFB 003.008\n' if minor >= 8 else b'RFB 003.003\n')
        if minor >= 7:
            count = self.read(sock, 1)[0]
            if count == 0:
                length = struct.unpack('>I', self.read(sock, 4))[0]
                raise RuntimeError('RFB refused: ' + self.read(sock, length).decode(errors='replace'))
            types = list(self.read(sock, count))
            if 1 not in types:
                raise RuntimeError('RFB wants authentication (security types %s); the spike runs without a password' % types)
            sock.sendall(b'\x01')
            if minor >= 8:
                status = struct.unpack('>I', self.read(sock, 4))[0]
                if status != 0:
                    raise RuntimeError('RFB security result %d' % status)
        else:
            kind = struct.unpack('>I', self.read(sock, 4))[0]
            if kind != 1:
                raise RuntimeError('RFB security type %d' % kind)
        sock.sendall(b'\x01')
        header = self.read(sock, 24)
        self.read(sock, struct.unpack('>I', header[20:24])[0])
        return sock

    @staticmethod
    def read(sock, n):
        data = b''
        while len(data) < n:
            chunk = sock.recv(n - len(data))
            if not chunk:
                raise RuntimeError('RFB connection closed')
            data += chunk
        return data

    def send(self, mods, key):
        syms = [self.SYMS[m] for m in mods] + [self.keysym(key)]
        sock = self.connect()
        try:
            for sym in syms:
                sock.sendall(struct.pack('>BBHI', 4, 1, 0, sym))
            time.sleep(self.hold_ms / 1000)
            for sym in reversed(syms):
                sock.sendall(struct.pack('>BBHI', 4, 0, 0, sym))
            time.sleep(0.05)
        finally:
            sock.close()

    def pointer(self, x, y, buttons, wheel):
        if not (0 <= x <= 65535 and 0 <= y <= 65535):
            raise ValueError("RFB pointer coordinate exceeds 16 bits")
        if not hasattr(self, '_pointer_socket'):
            self._pointer_socket = self.connect()
        # HID and RFB order middle/right differently. RFB wheel-up/down are
        # momentary buttons 4/5 (RFB 3.8 section 7.5.5).
        mask = (buttons & 1) | ((buttons & 4) >> 1) | ((buttons & 2) << 1)
        def send(value):
            self._pointer_socket.sendall(struct.pack('>BBHH', 5, value, x, y))
        send(mask | (8 if wheel > 0 else 16 if wheel < 0 else 0))
        if wheel:
            time.sleep(0.05)
            send(mask)


class NoKeys:
    def __init__(self, why='no backend'):
        self.why = why

    def send(self, mods, key):
        raise NoPath(self.why)


def backend(spec):
    kind, _, rest = spec.partition(':')
    if kind == 'qmp' and rest:
        return QMPKeys(rest)
    if kind == 'prlctl' and rest:
        return PrlctlKeys(rest)
    if kind == 'vnc' and rest:
        host, _, port = rest.rpartition(':')
        return VNCKeys(host or '127.0.0.1', port)
    if kind == 'none':
        return NoKeys()
    raise ValueError('bad backend ' + repr(spec))


class Watcher:
    """Feeds on serial text, injects each requested seq once."""

    def __init__(self, keys, name, log=print, suite=None):
        self.keys, self.name, self.log, self.suite = keys, name, log, suite
        self.seen, self.pending = set(), ''

    def feed(self, text):
        self.pending += text
        *lines, self.pending = self.pending.split('\n')
        for line in lines:
            self.line(line)

    def line(self, line):
        pointer = POINTER_REQUEST.fullmatch(line)
        if pointer:
            seq, x, y, buttons, wheel = map(int, pointer.groups())
            identity = ('usb-pointer', seq)
            if identity in self.seen:
                return
            self.seen.add(identity)
            try:
                send = getattr(self.keys, 'pointer', None)
                if send is None:
                    raise NoPath('this backend has no pointer transport')
                send(x, y, buttons, wheel)
                self.log('[hostpointer] seq=%d sent via %s' % (seq, self.name))
            except (NoPath, OSError, RuntimeError, ValueError) as error:
                self.log('[hostpointer] seq=%d no host pointer input on %s: %s' % (seq, self.name, error))
            return
        start = re.fullmatch(r'SUITE ([a-z0-9-]+) START .*\r?', line)
        if start:
            self.seen = {key for key in self.seen if key[0] != start.group(1)}
        match = REQUEST.match(line)
        if not match or (self.suite and match.group(1) != self.suite):
            return
        suite, case, seq, chords = match.groups()
        identity = (suite, case, seq)
        if identity in self.seen:
            return
        self.seen.add(identity)
        done = []
        try:
            for chord in chords.split(','):
                mods, key = parse_chord(chord)
                self.keys.send(mods, key)
                done.append(chord)
            self.log('[hostkeys] case=%s seq=%s keys=%s sent via %s' % (case, seq, chords, self.name))
        except NoPath as why:
            self.log('[hostkeys] case=%s seq=%s keys=%s no host key path for %s on %s: %s'
                     % (case, seq, chords, chord, self.name, why))
        except (ValueError, RuntimeError, OSError, subprocess.SubprocessError) as error:
            self.log('[hostkeys] case=%s seq=%s keys=%s failed on %s after %d chord(s): %s'
                     % (case, seq, chords, self.name, len(done), error))


def declaration(spec):
    """The contract's `<method>[;missing=<key>,...]` string for a backend; empty when there is no path."""
    kind = spec.partition(':')[0]
    if kind == 'prlctl':
        return 'prlctl;missing=' + ','.join(EXTENDED)
    return kind if kind in ('qmp', 'vnc') else ''


def watch(args):
    keys = backend(args.backend)
    out = open(args.log, 'a', buffering=1) if args.log else None

    def log(message):
        print(message, flush=True)
        if out:
            out.write(message + '\n')

    watcher = Watcher(keys, args.backend.partition(':')[0], log, args.suite)
    offset = 0
    while True:
        if args.parent:
            try:
                os.kill(args.parent, 0)
            except ProcessLookupError:
                return
            except PermissionError:
                pass
        try:
            with open(args.serial, 'rb') as f:
                f.seek(offset)
                data = f.read()
            offset += len(data)
            if data:
                watcher.feed(data.decode(errors='replace'))
        except FileNotFoundError:
            pass
        time.sleep(0.1)


def selftest_parse():
    assert parse_chord('alt-f3') == (('alt',), 'f3')
    assert parse_chord('ctrl-alt-f12') == (('ctrl', 'alt'), 'f12')
    assert parse_chord('alt-left') == (('alt',), 'left')
    for bad in ('f13', 'alt-alt-f1', 'meta-f1', 'alt-', 'pgup'):
        try:
            parse_chord(bad)
        except ValueError:
            continue
        raise AssertionError(bad)
    sent = []

    class Recorder:
        def send(self, mods, key):
            sent.append((mods, key))
    w = Watcher(Recorder(), 'test', lambda m: None)
    w.feed('kernel::test_exec:   SUITE console HOSTKEY vt/a seq=0 keys=alt-f1\r\n'
           'SUITE console HOSTKEY vt/a seq=1 keys=alt-f2,alt-left\r\nSUITE console HOSTKEY vt/a seq=1 keys=alt-f2\n'
           'SUITE console HOSTKEY vt/b seq')
    w.feed('=2 keys=ctrl-alt-f3\n')
    assert sent == [(('alt',), 'f2'), (('alt',), 'left'), (('ctrl', 'alt'), 'f3')], sent
    sent.clear()
    w.feed('SUITE other HOSTKEY vt/a seq=1 keys=z\n'
           'SUITE console START cases=1\n'
           'SUITE console HOSTKEY vt/a seq=1 keys=x\n')
    assert sent == [((), 'z'), ((), 'x')], sent
    sent.clear()
    log = []
    p = Watcher(PrlctlKeys('none'), 'prlctl', log.append)
    p.feed('SUITE console HOSTKEY vt/c seq=7 keys=alt-left\n')
    assert 'no host key path for alt-left on prlctl' in log[0], log
    assert declaration('prlctl:x').startswith('prlctl;missing=left,right') and declaration('qmp:x') == 'qmp'
    # Exercise the production RFB encoder, including HID middle/right order
    # and the wheel release packet, without opening a VM connection.
    class Wire:
        def __init__(self):
            self.packets = []
        def sendall(self, data):
            self.packets.append(struct.unpack('>BBHH', data))
    wire = Wire()
    vnc = VNCKeys('', 0)
    vnc._pointer_socket = wire
    for buttons, rfb in ((0, 0), (1, 1), (2, 4), (4, 2), (7, 7)):
        vnc.pointer(123, 456, buttons, 0)
        assert wire.packets.pop() == (5, rfb, 123, 456)
    for wheel, rfb in ((1, 8), (-1, 16)):
        vnc.pointer(123, 456, 2, wheel)
        assert wire.packets == [(5, rfb | 4, 123, 456), (5, 4, 123, 456)]
        wire.packets.clear()
    print('hostkeys parse and RFB pointer wire ok')


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest='command', required=True)
    w = sub.add_parser('watch')
    w.add_argument('--serial', required=True)
    w.add_argument('--backend', required=True)
    w.add_argument('--log')
    w.add_argument('--suite')
    w.add_argument('--parent', type=int, help='exit when this process is gone')
    s = sub.add_parser('send')
    s.add_argument('--backend', required=True)
    s.add_argument('chord')
    d = sub.add_parser('declare')
    d.add_argument('--backend', required=True)
    sub.add_parser('selftest-parse')
    args = parser.parse_args()
    if args.command == 'watch':
        try:
            watch(args)
        except KeyboardInterrupt:
            pass
    elif args.command == 'send':
        mods, key = parse_chord(args.chord)
        try:
            backend(args.backend).send(mods, key)
        except NoPath as why:
            print('no host key path: %s' % why, file=sys.stderr)
            return 3
    elif args.command == 'declare':
        print(declaration(args.backend))
    else:
        selftest_parse()
    return 0


if __name__ == '__main__':
    sys.exit(main())
