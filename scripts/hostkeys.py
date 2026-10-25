#!/usr/bin/env python3
"""Host key injection for suites that need a key the guest cannot press itself.

A suite case asks for keys on its serial line (docs/console/contract.md, "Host-injected keys"):

    SUITE <suite> HOSTKEY <category>/<case> seq=<n> keys=<chord>[,<chord>...]

`seq` increases across the run and each seq is injected once, so a replayed line is
harmless. A chord is modifiers then one key, joined by '-':

    modifiers  ctrl alt shift super rctrl ralt
    keys       a-z 0-9 f1-f12 left right up down pageup pagedown home end insert
               delete enter tab backspace esc space capslock comma
    examples   alt-f3  ctrl-alt-f2  alt-left  alt-right  alt-f7  shift-pageup

Chords go out in order, each pressed modifiers first and released in reverse.
The runner watching the serial log presses the chord on the target's own virtual
keyboard and the case reads the screen back. The runner never reports success: the
evidence is in the guest. A line that is not at the start of a serial line (the
kernel's announcement lines) is never a request.

Backends for the standalone `send` and `watch` commands:

    qmp:<socket>          QEMU's QMP send-key (ARM64 and x86-64 QEMU)
    prlsdk:<vm>           Parallels Virtualization SDK key and pointer events (Parallels)
    vnc:<host>:<port>     RFB key events (VMware Fusion's built-in VNC server)
    none                  nothing: every request is answered "no host key path"

The x86-metal boot loop reaches the machine only through power control and the serial
console, so it has no virtual keyboard to press. It declares `xhci-replay` and answers each
request by typing `BXKEY <report>[,<report>...]` lines, one per chord, on the serial console
(ReplayKeys); the kernel replays each report through its USB boot keyboard's interrupt-IN
completion path (kernel/src/drivers/xhci/host_keys.rs).
This is a library transport: `metal-boot` supplies its serial writer to `ReplayKeys`
and passes that instance to `Watcher`. `declare --backend xhci-replay` returns the
suite declaration, but `xhci-replay` is not a standalone `send` or `watch` backend.

Parallels (27.0.3): the Parallels Virtualization SDK ships inside Parallels Desktop and
this script calls its published C API through ctypes. Keys are Parallels virtual key
codes (PRL_KEY, PrlKeys.h), which carry the extended keys a PS/2 scancode cannot (`prlctl`
refuses a bare 0xE0 code): probe boots of the clean-room USB driver showed Left (usage
0x50), Right (0x4F), Page Up (0x4B), Home and the right-hand modifiers report by report.
They go one event at a time through PrlDevKeyboard_SendKeyEventEx rather than as a
`prlctl send-key-event -j` batch: in one probe VM the batches lost a chord's releases in
about one chord in eight (the keyboard's own later reports still held the keys), the
per-event calls in none of twenty. Pointer events: PrlDevMouse_SetPos reaches the
virtual tablet (absolute) and PrlDevMouse_Move the relative interface, both shown on
probe boots without Parallels Tools installed.

VMware Fusion: run.sh enables the VM's built-in VNC server for a suite list containing
console, choosing a free loopback port. BREENIX_VMWARE_VNC_PORT overrides that choice
and can enable VNC for other boot modes. RemoteDisplay.vnc.ip binds it to 127.0.0.1;
the server stops with the owned VM. The watcher sends RFB key events to its USB keyboard.
Relative pointer motion remains a host limitation (docs/console/contract.md section 10).

Usage:
    hostkeys.py watch --serial FILE --backend SPEC [--log FILE] [--suite ID] [--parent PID] [--qmp-persistent]
    hostkeys.py send --backend SPEC CHORD
    hostkeys.py declare --backend SPEC      (the contract's hostkeys declaration string)
    hostkeys.py selftest-parse
"""
import argparse
from contextlib import ExitStack
import json
import os
import re
import select
import socket
import struct
import subprocess
import sys
import time

REQUEST = re.compile(r'^SUITE ([a-z0-9]+(?:-[a-z0-9]+)*) HOSTKEY ([a-z0-9]+(?:-[a-z0-9]+)*/[a-z0-9]+(?:-[a-z0-9]+)*)'
                     r' seq=(\d+) keys=([a-z0-9]+(?:-[a-z0-9]+)*(?:,[a-z0-9]+(?:-[a-z0-9]+)*)*)\r?$')

POINTER_REQUEST = re.compile(r'^USB POINTER HOST seq=(\d+) axes=(absolute|relative) x=(\d+) y=(\d+) buttons=([0-7])'
                             r' wheel=(-?1|0)\r?$')

SUITE_POINTER = re.compile(r'^SUITE ([a-z0-9-]+) HOSTPOINTER ([a-z0-9-]+/[a-z0-9-]+) seq=(\d+)'
                           r' x=(\d+) y=(\d+) width=(\d+) height=(\d+) buttons=([0-7]) wheel=(-?1|0)\r?$')

MODIFIERS = ('ctrl', 'alt', 'shift', 'super', 'rctrl', 'ralt')
NAMED = ('left', 'right', 'up', 'down', 'pageup', 'pagedown', 'home', 'end', 'insert', 'delete', 'enter', 'tab',
         'backspace', 'esc', 'space', 'capslock', 'comma')



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
        self.persistent = False
        self.session = self.stream = None

    def connect(self):
        if self.stream is not None:
            return
        with ExitStack() as opened:
            sock = opened.enter_context(socket.socket(socket.AF_UNIX, socket.SOCK_STREAM))
            sock.settimeout(0.8)
            sock.connect(self.path)
            stream = opened.enter_context(sock.makefile('rw'))
            json.loads(stream.readline())
            self.request(stream, 'qmp_capabilities', {})
            self.session, self.stream = opened.pop_all(), stream

    @staticmethod
    def request(stream, command, arguments):
        stream.write(json.dumps({'execute': command, 'arguments': arguments}) + '\n')
        stream.flush()
        while True:
            reply = json.loads(stream.readline())
            if 'error' in reply:
                raise RuntimeError('QMP ' + command + ': ' + str(reply['error']))
            if 'return' in reply:
                return

    def close(self):
        session = self.session
        self.session = self.stream = None
        if session is not None:
            session.close()

    def execute(self, command, arguments):
        try:
            self.connect()
            self.request(self.stream, command, arguments)
        except Exception:
            # A lost reply must not replay an input event that may have arrived.
            self.close()
            raise
        finally:
            if not self.persistent:
                self.close()

    @classmethod
    def keys(cls, mods, key):
        return [{'type': 'qcode', 'data': cls.QCODES.get(k, k)} for k in (*mods, key)]

    def send(self, mods, key):
        qcodes = [k['data'] for k in self.keys(mods, key)]
        # W30: each chord must finish its releases before the next chord.
        # send-key returns before its release timer, so adjacent repeated keys
        # in a text request could overlap and lose a press.
        def edge(code, down):
            return {'type': 'key', 'data': {'down': down,
                    'key': {'type': 'qcode', 'data': code}}}
        self.execute('input-send-event', {'events': [edge(q, True) for q in qcodes]})
        time.sleep(self.hold_ms / 1000)
        self.execute('input-send-event', {'events': [edge(q, False) for q in reversed(qcodes)]})

    def pointer(self, x, y, buttons, wheel, axes, width=32768, height=32768):
        if axes != 'absolute' or width < 2 or height < 2 or not (0 <= x < width and 0 <= y < height):
            raise NoPath('QMP tablet requires absolute coordinates within its declared mode')
        events = [{'type': 'abs', 'data': {'axis': axis, 'value': value}}
                  for axis, value in (('x', (x * 32767 + width - 2) // (width - 1)),
                                      ('y', (y * 32767 + height - 2) // (height - 1)))]
        events += [{'type': 'btn', 'data': {'button': name, 'down': bool(buttons & bit)}}
                   for name, bit in (('left', 1), ('right', 2), ('middle', 4))]
        if wheel:
            events += [{'type': 'btn', 'data': {'button': 'wheel-up' if wheel > 0 else 'wheel-down', 'down': down}}
                       for down in (True, False)]
        self.execute('input-send-event', {'events': events})


class ParallelsKeys:
    """Keys and pointer events through the Parallels Virtualization SDK.

    Keys are Parallels virtual key codes (PRL_KEY, PrlKeys.h), sent one event at a time
    with PrlDevKeyboard_SendKeyEventEx (PKE_PRESS 0, PKE_RELEASE 0x80) and the hold timed
    here. Pointer requests are positions, each naming the interface kind it measures: for
    `absolute`, PrlDevMouse_SetPos places the absolute (tablet) interface there; for
    `relative`, PrlDevMouse_Move moves the relative interface by the distance from the
    previous relative request (the first has no known origin and moves nothing). Buttons are PRL_MOUSE_BUTTON (left 1, right 2, middle 4), the request's
    own HID order; the wheel is the z argument.
    """
    SDK = ('/Applications/Parallels Desktop.app/Contents/Frameworks/ParallelsVirtualizationSDK.framework/'
           'ParallelsVirtualizationSDK')
    CODES = {'ctrl': 37, 'alt': 64, 'shift': 50, 'super': 115, 'rctrl': 109, 'ralt': 113, 'esc': 9, 'tab': 23,
             'enter': 36, 'space': 65, 'capslock': 66, 'backspace': 22, 'home': 97, 'up': 98, 'pageup': 99,
             'left': 100, 'right': 102, 'end': 103, 'down': 104, 'pagedown': 105, 'insert': 106, 'delete': 107,
             'f11': 95, 'f12': 96, 'comma': 59}
    ROWS = 'qwertyuiop', 'asdfghjkl', 'zxcvbnm'
    PRESS, RELEASE = 0, 0x80

    def __init__(self, vm, hold_ms=60, library=None):
        self.name, self.hold_ms = vm, hold_ms
        self.sdk, self.vm, self.previous = library, (vm if library is not None else None), None

    @classmethod
    def code(cls, key):
        if key in cls.CODES:
            return cls.CODES[key]
        match = re.fullmatch(r'f(\d+)', key)
        if match:
            return 66 + int(match.group(1))
        if key.isdigit():
            return 19 if key == '0' else 9 + int(key)
        for row, first in zip(cls.ROWS, (24, 38, 52)):
            if key in row:
                return first + row.index(key)
        raise NoPath(key + ' has no Parallels virtual key code here')

    @staticmethod
    def check(code, what):
        if code & 0xffffffff:
            raise RuntimeError('%s returned %#x' % (what, code & 0xffffffff))

    def connect(self):
        if self.vm is not None:
            return
        import ctypes
        sdk = ctypes.CDLL(self.SDK)
        handle = ctypes.c_void_p
        for name, args, result in (
                ('PrlApi_InitEx', [ctypes.c_uint32] * 4, ctypes.c_int32),
                ('PrlSrv_Create', [ctypes.POINTER(handle)], ctypes.c_int32),
                ('PrlSrv_LoginLocal', [handle, ctypes.c_char_p, ctypes.c_uint32, ctypes.c_uint32], handle),
                ('PrlSrv_GetVmList', [handle], handle),
                ('PrlJob_Wait', [handle, ctypes.c_uint32], ctypes.c_int32),
                ('PrlJob_GetRetCode', [handle, ctypes.POINTER(ctypes.c_int32)], ctypes.c_int32),
                ('PrlJob_GetResult', [handle, ctypes.POINTER(handle)], ctypes.c_int32),
                ('PrlResult_GetParamsCount', [handle, ctypes.POINTER(ctypes.c_uint32)], ctypes.c_int32),
                ('PrlResult_GetParamByIndex', [handle, ctypes.c_uint32, ctypes.POINTER(handle)], ctypes.c_int32),
                ('PrlVmCfg_GetName', [handle, ctypes.c_char_p, ctypes.POINTER(ctypes.c_uint32)], ctypes.c_int32),
                ('PrlDevDisplay_ConnectToVm', [handle, ctypes.c_uint32], handle),
                ('PrlDevKeyboard_SendKeyEventEx', [handle, ctypes.c_uint32, ctypes.c_uint32], ctypes.c_int32),
                ('PrlDevMouse_Move', [handle, ctypes.c_int32, ctypes.c_int32, ctypes.c_int32, ctypes.c_uint32],
                 ctypes.c_int32),
                ('PrlDevMouse_SetPos', [handle, ctypes.c_int32, ctypes.c_int32, ctypes.c_int32, ctypes.c_uint32],
                 ctypes.c_int32)):
            function = getattr(sdk, name)
            function.argtypes, function.restype = args, result

        def job(started, what):
            code = ctypes.c_int32()
            self.check(sdk.PrlJob_Wait(started, 10000), what + ' wait')
            self.check(sdk.PrlJob_GetRetCode(started, ctypes.byref(code)), what + ' result')
            self.check(code.value, what)
            return started
        # PrlApi_InitEx takes 0 as this library's own version; PAM_DESKTOP_MAC is 1.
        sdk.PrlApi_InitEx(0, 1, 0, 0)
        server = handle()
        self.check(sdk.PrlSrv_Create(ctypes.byref(server)), 'PrlSrv_Create')
        job(sdk.PrlSrv_LoginLocal(server, None, 0, 1), 'PrlSrv_LoginLocal')
        listing = job(sdk.PrlSrv_GetVmList(server), 'PrlSrv_GetVmList')
        result, count = handle(), ctypes.c_uint32()
        self.check(sdk.PrlJob_GetResult(listing, ctypes.byref(result)), 'PrlJob_GetResult')
        self.check(sdk.PrlResult_GetParamsCount(result, ctypes.byref(count)), 'PrlResult_GetParamsCount')
        for i in range(count.value):
            vm, found, length = handle(), ctypes.create_string_buffer(256), ctypes.c_uint32(256)
            self.check(sdk.PrlResult_GetParamByIndex(result, i, ctypes.byref(vm)), 'PrlResult_GetParamByIndex')
            self.check(sdk.PrlVmCfg_GetName(vm, found, ctypes.byref(length)), 'PrlVmCfg_GetName')
            if found.value.decode() == self.name:
                # Keyboard and mouse events need a remote desktop session (C API reference).
                job(sdk.PrlDevDisplay_ConnectToVm(vm, 0), 'PrlDevDisplay_ConnectToVm')
                self.sdk, self.vm = sdk, vm
                return
        raise RuntimeError('Parallels VM %r not found through the SDK' % self.name)

    def send(self, mods, key):
        codes = [self.code(m) for m in mods] + [self.code(key)]
        self.connect()
        for code in codes:
            self.check(self.sdk.PrlDevKeyboard_SendKeyEventEx(self.vm, code, self.PRESS), 'key press')
        time.sleep(self.hold_ms / 1000)
        for code in reversed(codes):
            self.check(self.sdk.PrlDevKeyboard_SendKeyEventEx(self.vm, code, self.RELEASE), 'key release')

    def pointer(self, x, y, buttons, wheel, axes):
        self.connect()
        if axes == 'relative':
            dx, dy = (0, 0) if self.previous is None else (x - self.previous[0], y - self.previous[1])
            self.previous = (x, y)
            self.check(self.sdk.PrlDevMouse_Move(self.vm, dx, dy, wheel, buttons & 7), 'PrlDevMouse_Move')
        else:
            self.check(self.sdk.PrlDevMouse_SetPos(self.vm, x, y, wheel, buttons & 7), 'PrlDevMouse_SetPos')


class VNCKeys:
    SYMS = {'ctrl': 0xFFE3, 'alt': 0xFFE9, 'shift': 0xFFE1, 'super': 0xFFEB, 'rctrl': 0xFFE4, 'ralt': 0xFFEA,
            'capslock': 0xFFE5, 'enter': 0xFF0D, 'esc': 0xFF1B, 'tab': 0xFF09, 'backspace': 0xFF08, 'space': 0x20,
            'left': 0xFF51, 'up': 0xFF52, 'right': 0xFF53, 'down': 0xFF54, 'pageup': 0xFF55, 'pagedown': 0xFF56,
            'home': 0xFF50, 'end': 0xFF57, 'insert': 0xFF63, 'delete': 0xFFFF, 'comma': 0x2C}

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

    def pointer(self, x, y, buttons, wheel, axes):
        # Fusion's configuration, not the RFB event, decides which interface reports it
        # (run.sh's BREENIX_VMWARE_POINTER); the requested kind changes no RFB field.
        if not (0 <= x <= 65535 and 0 <= y <= 65535):
            raise ValueError("RFB pointer coordinate exceeds 16 bits")
        fresh = not hasattr(self, '_pointer_socket')
        if fresh:
            self._pointer_socket = self.connect()
        # HID and RFB order middle/right differently. RFB wheel-up/down are
        # momentary buttons 4/5 (RFB 3.8 section 7.5.5).
        mask = (buttons & 1) | ((buttons & 4) >> 1) | ((buttons & 2) << 1)
        def send(value):
            self._pointer_socket.sendall(struct.pack('>BBHH', 5, value, x, y))
        if fresh:
            # Fusion's VNC server did not deliver the first pointer event of a new
            # connection to the guest (one VMware run, seq=0); later events arrived.
            # Send the position and buttons once more before the requested event.
            send(mask)
            time.sleep(0.05)
        send(mask | (8 if wheel > 0 else 16 if wheel < 0 else 0))
        if wheel:
            time.sleep(0.05)
            send(mask)


class NoKeys:
    def __init__(self, why='no backend'):
        self.why = why

    def send(self, mods, key):
        raise NoPath(self.why)


class ReplayKeys:
    """HID boot keyboard reports (HID 1.11 B.1) for the kernel's xHCI replay, one line per chord.

    A chord is pressed as a keyboard reports it: each modifier in order, then the key, released
    in reverse, one report per change. `write` takes one line; the kernel replays its reports
    HOST_KEY_GAP_MS apart, so each chord waits that long per report before the next is written.
    """
    # Modifier bits of the report's first byte; usages of the Keyboard/Keypad page (HID Usage Tables 1.12, 10).
    BITS = {'ctrl': 0x01, 'shift': 0x02, 'alt': 0x04, 'super': 0x08, 'rctrl': 0x10, 'ralt': 0x40}
    USAGES = {**{chr(ord('a') + i): 0x04 + i for i in range(26)}, **{str(i): 0x1D + i for i in range(1, 10)},
              '0': 0x27, 'enter': 0x28, 'esc': 0x29, 'backspace': 0x2A, 'tab': 0x2B, 'space': 0x2C,
              'capslock': 0x39, 'comma': 0x36, **{'f%d' % i: 0x39 + i for i in range(1, 13)}, 'insert': 0x49, 'home': 0x4A,
              'pageup': 0x4B, 'delete': 0x4C, 'end': 0x4D, 'pagedown': 0x4E, 'right': 0x4F, 'left': 0x50,
              'down': 0x51, 'up': 0x52}
    HOST_KEY_GAP_MS = 10

    def __init__(self, write, pace=time.sleep):
        self.write, self.pace = write, pace

    @classmethod
    def reports(cls, mods, key):
        held, steps = 0, []
        for mod in mods:
            held |= cls.BITS[mod]
            steps.append(bytes([held, 0, 0, 0, 0, 0, 0, 0]))
        steps.append(bytes([held, 0, cls.USAGES[key], 0, 0, 0, 0, 0]))
        return steps + steps[-2::-1] + [bytes(8)]

    @classmethod
    def line(cls, mods, key):
        return 'BXKEY ' + ','.join(report.hex() for report in cls.reports(mods, key))

    def send(self, mods, key):
        self.write(self.line(mods, key))
        self.pace(len(self.reports(mods, key)) * self.HOST_KEY_GAP_MS / 1000)


def backend(spec):
    kind, _, rest = spec.partition(':')
    if kind == 'qmp' and rest:
        return QMPKeys(rest)
    if kind == 'prlsdk' and rest:
        return ParallelsKeys(rest)
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
        suite_pointer = SUITE_POINTER.fullmatch(line)
        if suite_pointer:
            suite, case, seq, *numbers = suite_pointer.groups()
            if self.suite and suite != self.suite:
                return
            identity = (suite, case, int(seq))
            if identity in self.seen:
                return
            self.seen.add(identity)
            x, y, width, height, buttons, wheel = map(int, numbers)
            try:
                if not (width >= 2 and height >= 2 and 0 <= x < width and 0 <= y < height):
                    raise ValueError('pointer outside declared mode')
                if isinstance(self.keys, QMPKeys):
                    self.keys.pointer(x, y, buttons, wheel, 'absolute', width, height)
                else:
                    send = getattr(self.keys, 'pointer', None)
                    if send is None:
                        raise NoPath('this backend has no pointer transport')
                    send(x, y, buttons, wheel, 'absolute')
                self.log('[hostpointer] case=%s seq=%s sent via %s' % (case, seq, self.name))
            except (NoPath, OSError, RuntimeError, ValueError) as error:
                self.log('[hostpointer] case=%s seq=%s failed: %s' % (case, seq, error))
            return
        pointer = POINTER_REQUEST.fullmatch(line)
        if pointer:
            seq, axes, x, y, buttons, wheel = pointer.groups()
            seq, x, y, buttons, wheel = map(int, (seq, x, y, buttons, wheel))
            identity = ('usb-pointer', seq)
            if identity in self.seen:
                return
            self.seen.add(identity)
            try:
                send = getattr(self.keys, 'pointer', None)
                if send is None:
                    raise NoPath('this backend has no pointer transport')
                send(x, y, buttons, wheel, axes)
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
    return kind if kind in ('qmp', 'prlsdk', 'vnc', 'xhci-replay') else ''


def watch(args):
    keys = backend(args.backend)
    if args.qmp_persistent:
        if not isinstance(keys, QMPKeys):
            raise ValueError('--qmp-persistent requires a private QMP backend')
        keys.persistent = True
    out = open(args.log, 'a', buffering=1) if args.log else None

    def log(message):
        print(message, flush=True)
        if out:
            out.write(message + '\n')

    watcher = Watcher(keys, args.backend.partition(':')[0], log, args.suite)
    offset = 0
    events = select.kqueue() if hasattr(select, 'kqueue') else None
    try:
        while True:
            if args.parent:
                try:
                    os.kill(args.parent, 0)
                except ProcessLookupError:
                    return
                except PermissionError:
                    pass
            if args.qmp_persistent:
                try:
                    # The launcher starts the watcher before QEMU creates its socket.
                    # Negotiate while idle, rather than on a timed input request.
                    keys.connect()
                except (OSError, json.JSONDecodeError, RuntimeError):
                    # Teardown can close the greeting or reject capabilities.
                    # connect() closes incomplete sessions; retry while the parent lives.
                    pass
            try:
                with open(args.serial, 'rb') as f:
                    if events:
                        # Register before reading: an append between EOF and wait must wake us.
                        events.control([select.kevent(f.fileno(), filter=select.KQ_FILTER_VNODE,
                            flags=select.KQ_EV_ADD | select.KQ_EV_CLEAR,
                            fflags=select.KQ_NOTE_WRITE)], 0, 0)
                    f.seek(offset)
                    data = f.read()
                    offset += len(data)
                    if data:
                        watcher.feed(data.decode(errors='replace'))
                    if events:
                        events.control(None, 1, 0.1)
                    else:
                        time.sleep(0.1)
            except FileNotFoundError:
                time.sleep(0.1)
    finally:
        if events:
            events.close()
        if isinstance(keys, QMPKeys):
            keys.close()
        if out:
            out.close()


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
    # The chord grammar's keys (26 letters, 10 digits, 12 F-keys, NAMED) are the usages' keys.
    keys = [chr(c) for c in range(ord('a'), ord('z') + 1)] + [str(d) for d in range(10)] + \
        ['f%d' % n for n in range(1, 13)] + list(NAMED)
    assert sorted(ReplayKeys.USAGES) == sorted(keys)
    assert ReplayKeys.line(*parse_chord('ctrl-alt-f2')) == \
        'BXKEY 0100000000000000,0500000000000000,05003b0000000000,0500000000000000,0100000000000000,0000000000000000'
    assert ReplayKeys.line(*parse_chord('a')) == 'BXKEY 0000040000000000,0000000000000000'
    assert ReplayKeys.reports((), '0')[0][2] == 0x27 and ReplayKeys.reports((), 'f12')[0][2] == 0x45
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
    # Parallels: virtual key codes (PRL_KEY), one SDK event at a time, and pointer requests.
    class SDK:
        def __init__(self):
            self.calls = []
        def PrlDevKeyboard_SendKeyEventEx(self, vm, code, event):
            self.calls.append(('key', code, event))
            return 0
        def PrlDevMouse_Move(self, vm, dx, dy, dz, buttons):
            self.calls.append(('move', dx, dy, dz, buttons))
            return 0
        def PrlDevMouse_SetPos(self, vm, x, y, z, buttons):
            self.calls.append(('pos', x, y, z, buttons))
            return 0
    sdk = SDK()
    prl = ParallelsKeys('vm', hold_ms=0, library=sdk)
    def press(chord):
        sdk.calls.clear()
        prl.send(*parse_chord(chord))
        return [c[1] for c in sdk.calls if c[2] == ParallelsKeys.PRESS]
    assert press('a') == [38] and press('z') == [52] and press('p') == [33] and press('m') == [58]
    assert press('1') == [10] and press('0') == [19] and press('f1') == [67] and press('f10') == [76]
    assert press('f11') == [95] and press('f12') == [96] and press('capslock') == [66] and press('enter') == [36]
    assert press('alt-left') == [64, 100] and press('alt-right') == [64, 102]
    assert press('shift-pageup') == [50, 99] and press('shift-pagedown') == [50, 105]
    assert press('ctrl-alt-f2') == [37, 64, 68]
    sdk.calls.clear()
    prl.send(('ctrl',), 'c')
    assert sdk.calls == [('key', 37, 0), ('key', 54, 0), ('key', 54, 0x80), ('key', 37, 0x80)], sdk.calls
    assert declaration('prlsdk:x') == 'prlsdk' and declaration('qmp:x') == 'qmp'
    requests = ((100, 100, 0, 0), (140, 130, 0, 0), (140, 130, 1, 0), (140, 130, 0, 1), (100, 100, 4, -1))
    sdk.calls.clear()
    for request in requests:
        prl.pointer(*request, 'absolute')
    assert sdk.calls == [('pos', 100, 100, 0, 0), ('pos', 140, 130, 0, 0), ('pos', 140, 130, 0, 1),
                         ('pos', 140, 130, 1, 0), ('pos', 100, 100, -1, 4)], sdk.calls
    sdk.calls.clear()
    for request in requests:
        prl.pointer(*request, 'relative')
    assert sdk.calls == [('move', 0, 0, 0, 0), ('move', 40, 30, 0, 0), ('move', 0, 0, 0, 1), ('move', 0, 0, 1, 0),
                         ('move', -40, -30, -1, 4)], sdk.calls
    # The watcher hands each pointer request, with its kind, to the backend once.
    class Pointers:
        def __init__(self):
            self.requests = []
        def pointer(self, *request):
            self.requests.append(request)
    pointers = Pointers()
    w = Watcher(pointers, 'test', lambda m: None)
    w.feed('USB POINTER HOST seq=0 axes=relative x=10 y=20 buttons=1 wheel=-1\r\n'
           'USB POINTER HOST seq=0 axes=relative x=10 y=20 buttons=1 wheel=-1\n'
           'USB POINTER HOST seq=1 axes=absolute x=30 y=40 buttons=0 wheel=0\n')
    assert pointers.requests == [(10, 20, 1, -1, 'relative'), (30, 40, 0, 0, 'absolute')], pointers.requests
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
        vnc.pointer(123, 456, buttons, 0, 'absolute')
        assert wire.packets.pop() == (5, rfb, 123, 456)
    for wheel, rfb in ((1, 8), (-1, 16)):
        vnc.pointer(123, 456, 2, wheel, 'absolute')
        assert wire.packets == [(5, rfb | 4, 123, 456), (5, 4, 123, 456)]
        wire.packets.clear()
    # A new connection sends the state once before the requested event.
    fresh = VNCKeys('', 0)
    fresh.connect = lambda: wire
    fresh.pointer(10, 20, 1, 1, 'absolute')
    assert wire.packets == [(5, 1, 10, 20), (5, 9, 10, 20), (5, 1, 10, 20)], wire.packets
    wire.packets.clear()
    print('hostkeys parse, Parallels SDK keys and pointer, and RFB pointer wire ok')


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest='command', required=True)
    w = sub.add_parser('watch')
    w.add_argument('--serial', required=True)
    w.add_argument('--backend', required=True)
    w.add_argument('--log')
    w.add_argument('--suite')
    w.add_argument('--parent', type=int, help='exit when this process is gone')
    w.add_argument('--qmp-persistent', action='store_true',
                   help='keep a private QMP connection ready between input requests')
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
