#!/usr/bin/env python3
"""Score one BreenFS power-cut or data bit-flip boot from its cold readback."""

import argparse
import hashlib
import json
import math
from pathlib import Path
import re
import struct
import subprocess
import sys
import tempfile


CASES = (
    ('aimed-cuts', 'Aimed power cuts preserve acknowledged writes'),
    ('random-cuts', 'Random power cuts preserve acknowledged writes'),
    ('bit-flips', 'Reads reject the injected block and the checker identifies it'),
)
WRITER_START = re.compile(rb'SUITE filesystem START cases=1 only=power-cut/writer nonce=([0-9a-f]{16})')
ACK = re.compile(rb'FSYNC ([0-9a-f]{16}) ([1-9][0-9]{0,19}) ([0-9a-f]{64})')
INJECTION = re.compile(rb'\[breenfs\] corrupt-inject block=([1-9][0-9]{0,19}) offset=([0-9]{1,4}) bit=[0-7]')
READ_RESULT = re.compile(rb'\[breenfs\] corrupt-read block=([1-9][0-9]{0,19}) result=(\S+)')
DATA_CRC = re.compile(rb'error C1 block ([1-9][0-9]{0,19}) \(data REF [0-9]+\+[0-9]+\): '
                      rb'CRC 0x[0-9a-f]{8}, REF records 0x[0-9a-f]{8}')
RECORD_BYTES = 4096
BODY = struct.Struct('<508Q')


def receipts(serial):
    lines = serial.replace(b'\r\n', b'\n').split(b'\n')
    starts = [(index, match[1]) for index, line in enumerate(lines[:-1])
              if (match := WRITER_START.fullmatch(line))]
    if len(starts) != 1:
        raise ValueError('expected one power-cut writer START in serial')
    start, nonce = starts[0]
    acknowledged = []
    for index, line in enumerate(lines):
        if line.startswith(b'SUITE filesystem ERROR '):
            raise ValueError(line.decode(errors='replace'))
        if b'FSYNC ' not in line:
            continue
        match = ACK.fullmatch(line)
        if match is None and index == len(lines) - 1:
            prefix = f'FSYNC {nonce.decode()} {len(acknowledged) + 1} '.encode()
            if prefix.startswith(line) or (line.startswith(prefix)
                    and re.fullmatch(rb'[0-9a-f]{0,63}', line[len(prefix):])):
                continue  # The cut may interrupt the last serial line.
        if match is None or index < start or match[1] != nonce:
            raise ValueError(f'invalid FSYNC receipt at serial line {index + 1}')
        seq = int(match[2])
        expected = len(acknowledged) + 1
        if seq != expected:
            raise ValueError(f'expected FSYNC sequence {expected}, found {seq}')
        acknowledged.append(bytes.fromhex(match[3].decode('ascii')))
    if not acknowledged:
        raise ValueError("no acknowledged writes before the cut")
    return int(nonce, 16), acknowledged


def cut_evidence(directory, nonce):
    run = json.loads((directory / 'run.json').read_text())
    events = [json.loads(line) for line in (directory / 'events.jsonl').read_text().splitlines()]
    cut = run.get('powerCut') or {}
    cut_at = cut.get('cutAt')
    off = [event for event in events if event['event'] == 'power-off']
    if (cut.get('powerOffConfirmed') is not True or type(cut_at) not in (int, float)
            or not math.isfinite(cut_at) or len(off) != 1 or off[0].get('off') is not True
            or not cut_at <= off[0]['t']):
        raise ValueError('cut evidence requires confirmed power-off')
    start = [event for event in events if event['event'] == 'power-cut-writer-start']
    if (len(start) != 1 or start[0].get('nonce') != f'{nonce:016x}'
            or not start[0]['t'] <= cut_at):
        raise ValueError('writer START differs from the cut evidence')
    if ((run.get('mode'), run.get('disk'), run.get('rootFs'), run.get('onlyCase'))
            != ('suite', 'root', 'breenfs', 'power-cut/writer')
            or not any(event['event'] == 'disk-root-record' and event.get('matchesLastInstall') is True
                       and event.get('installedBy') == run['id'] and event['t'] <= start[0]['t']
                       for event in events)):
        raise ValueError('cut evidence requires verified test-root identity')
    for event in events:
        if event['event'] == 'console-gap-start' and start[0]['t'] <= event['t'] <= off[0]['t']:
            raise ValueError('console gap could hide a write acknowledgement')
    return cut


def verify_aimed_window(serial, cut, window):
    marker = f'[breenfs] commit-window {window}'
    offset = cut.get('triggerSerialOffset')
    arrived = cut.get('triggerObservedAt')
    off_at = cut.get('powerOffAt')
    if (cut.get('triggerLine') != marker or type(offset) is not int or not 0 <= offset < len(serial)
            or (offset and serial[offset - 1:offset] != b'\n')
            or type(arrived) not in (int, float) or type(off_at) not in (int, float)
            or not arrived <= cut['cutAt'] <= off_at):
        raise ValueError('aimed cut is missing its timed trigger')
    first, _, after = serial[offset:].partition(b'\n')
    if first.rstrip(b'\r') != marker.encode():
        raise ValueError('aimed cut trigger differs from captured serial')
    if b'[breenfs] commit-window ' in after or b'FSYNC ' in after:
        raise ValueError('publication advanced past the cut trigger')


def replay(serial_path, extract, image):
    serial = serial_path.read_bytes()
    nonce, acknowledged = receipts(serial)
    cut = cut_evidence(serial_path.parent, nonce)
    if window := cut.get('window'):
        verify_aimed_window(serial, cut, window)
    # The extractor can emit a prefix before an error. Wait for its status
    # before judging any bytes and keep the binary log off the Python heap.
    with tempfile.TemporaryFile() as log:
        result = subprocess.run([extract, image, 'cat', '/crash/log'], stdout=log,
                                stderr=subprocess.PIPE)
        if result.returncode != 0:
            detail = result.stderr.decode(errors='replace').strip()
            raise ValueError(f'breenfs-extract /crash/log exited {result.returncode}: {detail}')
        log.seek(0)
        count = 0
        for seq, record in enumerate(iter(lambda: log.read(RECORD_BYTES), b''), 1):
            if len(record) != RECORD_BYTES:
                raise ValueError(f'torn record at sequence {seq}: {len(record)} bytes')
            actual, record_nonce, *payload = BODY.unpack_from(record)
            if actual != seq:
                raise ValueError(f'expected sequence {seq}, found {actual}')
            if record_nonce != nonce:
                raise ValueError(f'writer nonce differs at sequence {seq}')
            body = record[:BODY.size]
            digest = hashlib.sha256(body).digest()
            if digest != record[BODY.size:]:
                raise ValueError(f'stored digest differs at sequence {seq}')
            if any(word != seq ^ index for index, word in enumerate(payload)):
                raise ValueError(f'payload differs at sequence {seq}')
            if seq <= len(acknowledged) and digest != acknowledged[seq - 1]:
                raise ValueError(f'FSYNC digest differs at sequence {seq}')
            count = seq
    if count < len(acknowledged):
        raise ValueError(f'acknowledged sequence {count + 1} is missing')
    return f'{count} records, {len(acknowledged)} acknowledged'


def bit_flip(serial, check_output):
    lines = serial.replace(b'\r\n', b'\n').split(b'\n')
    injections = [i for i, line in enumerate(lines) if line.startswith(b'[breenfs] corrupt-inject')]
    if len(injections) != 1 or injections[0] == len(lines) - 1:
        raise ValueError('expected one complete corruption injection in serial')
    injection = INJECTION.fullmatch(lines[injections[0]])
    if injection is None or int(injection[2]) >= RECORD_BYTES:
        raise ValueError('invalid corruption injection in serial')
    block = int(injection[1])

    reads = [i for i, line in enumerate(lines) if line.startswith(b'[breenfs] corrupt-read')]
    if len(reads) != 1 or not injections[0] < reads[0] < len(lines) - 1:
        raise ValueError(f'expected one complete typed read result after injection for block {block}')
    read = READ_RESULT.fullmatch(lines[reads[0]])
    if read is None or int(read[1]) != block or read[2] != b'ReadError::Data':
        detail = lines[reads[0]].decode(errors='replace')
        raise ValueError(f'expected ReadError::Data for block {block}, got {detail}')

    if not any(match and int(match[1]) == block
               for match in map(DATA_CRC.fullmatch, check_output.splitlines())):
        raise ValueError(f'breenfs-check --full did not name injected block {block} in a C1 CRC error')
    return f'block {block} returned ReadError::Data and failed C1'


def check_manifest(path):
    manifest = json.loads(path.read_text())
    categories = [c for c in manifest['categories'] if c.get('hostRunner') is True]
    if (manifest['id'] != 'filesystem' or len(categories) != 1 or categories[0]['id'] != 'power-cut'
            or tuple((c['id'], c['title']) for c in categories[0]['cases']) != CASES):
        raise ValueError('filesystem power-cut host records differ from the replay checker')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='mode')
    for mode in ('replay', 'bit-flip'):
        command = commands.add_parser(mode)
        command.add_argument('serial', type=Path, help='boot driver serial.txt')
        command.add_argument('image', type=Path, help='cold test-root partition image')
        command.add_argument('--tools-dir', type=Path, required=True, help='matching build of the BreenFS tools')
    parser.add_argument('--check-manifest', type=Path)
    args = parser.parse_args()
    if bool(args.check_manifest) == bool(args.mode):
        parser.error('select replay, bit-flip or --check-manifest')

    try:
        if args.check_manifest:
            check_manifest(args.check_manifest)
            return 0
        serial = args.serial.read_bytes()
        tools = args.tools_dir.resolve()
        result = subprocess.run([tools / 'breenfs-check', '--full', args.image], capture_output=True)
        expected_exit = 0 if args.mode == 'replay' else 4
        if result.returncode != expected_exit:
            detail = (result.stdout + result.stderr).decode(errors='replace').strip()
            raise ValueError(f'breenfs-check --full exited {result.returncode}, expected {expected_exit}: {detail}')
        if args.mode == 'replay':
            detail = replay(args.serial, tools / 'breenfs-extract', args.image)
        else:
            detail = bit_flip(serial, result.stdout)
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f'FAIL: {error}', file=sys.stderr)
        return 1
    print(f'PASS: {detail}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
