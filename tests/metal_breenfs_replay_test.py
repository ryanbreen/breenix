"""Score cold readback evidence through the replay CLI."""

import hashlib
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
CHECKER = ROOT / 'scripts/metal/breenfs_replay.py'
TOOLS = Path(os.environ.get('BREENFS_TOOLS_DIR', ROOT / 'tools/breenfs/target/release')).resolve()
NONCE = '0123456789abcdef'
START = f'SUITE filesystem START cases=1 only=power-cut/writer nonce={NONCE}\n'.encode()
INJECTION = b'[breenfs] corrupt-inject block=120 offset=14 bit=2\n'
CRC_ERROR = b'error C1 block 120 (data REF 120+1): CRC 0x12345678, REF records 0x87654321\n'
BLOCK_BYTES = 4096
IMAGE_BYTES = 64 * 1024 * 1024
RING_FIRST = (16, IMAGE_BYTES // BLOCK_BYTES - 4)


def superblock(template, ring, generation, *, compat=0, torn=False):
    data = bytearray(template)
    slot = generation % 4
    struct.pack_into('<QBB', data, 32, generation, ring, slot)
    struct.pack_into('<Q', data, 64, compat)
    crc = 0xffffffff
    for byte in data[12:]:
        crc ^= byte
        for _ in range(8):
            crc = (crc >> 1) ^ (0x82f63b78 if crc & 1 else 0)
    struct.pack_into('<I', data, 8, crc ^ 0xffffffff ^ int(torn))
    return RING_FIRST[ring] + slot, data


def record(seq, *, wrong_payload=False, nonce=NONCE):
    words = [seq ^ index for index in range(506)]
    if wrong_payload:
        words[0] ^= 1
    body = struct.pack('<508Q', seq, int(nonce, 16), *words)
    return body + hashlib.sha256(body).digest()


def ack(seq):
    return f'FSYNC {seq} {record(seq)[-32:].hex()} nonce={NONCE}\n'.encode()


class ColdReadbackTest(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.work = Path(tmp.name)
        self.image = self.work / 'cold.img'
        self.image.touch()
        for name in ('breenfs-extract', 'breenfs-check'):
            tool = self.work / name
            tool.write_text(f'#!{sys.executable}\n' + '''
from pathlib import Path
import sys

if Path(sys.argv[0]).name == 'breenfs-extract':
    assert sys.argv[1:] == ['cold.img', 'cat', '/crash/log']
    sys.stdout.buffer.write(Path('crash-log').read_bytes())
else:
    assert sys.argv[1:] == ['--full', 'cold.img']
    sys.stdout.buffer.write(Path('check-stdout').read_bytes())
    sys.exit(int(Path('check-status').read_text()))
''')
            tool.chmod(0o755)

    def score(self, mode, serial, log=b'', *, check_status=0, check_stdout=b'', gap=False, off=True,
              root_match=True, aimed=False, tools_dir=None):
        (self.work / 'crash-log').write_bytes(log)
        (self.work / 'serial.txt').write_bytes(serial)
        (self.work / 'check-stdout').write_bytes(check_stdout)
        events = [
            dict(event='disk-root-record', t=1, matchesLastInstall=root_match, installedBy='writer'),
            dict(event='power-cut-writer-start', t=2, nonce='0123456789abcdef'),
            dict(event='power-off', t=5, off=off),
        ]
        if gap:
            events.insert(2, dict(event='console-gap-start', t=3))
        (self.work / 'events.jsonl').write_text(''.join(json.dumps(event) + '\n' for event in events))
        cut = dict(cutAt=4, powerOffConfirmed=off)
        if aimed:
            marker = b'[breenfs] commit-window first-flush'
            cut.update(window='first-flush', triggerLine=marker.decode(),
                       triggerSerialOffset=serial.index(marker), triggerObservedAt=3, powerOffAt=5)
        (self.work / 'run.json').write_text(json.dumps(dict(
            id='writer', rootFs='breenfs', disk='root', mode='suite',
            onlyCase='power-cut/writer', powerCut=cut,
        )))
        (self.work / 'check-status').write_text(str(check_status))
        return subprocess.run(
            [sys.executable, str(CHECKER), mode, 'serial.txt', self.image.name,
             '--tools-dir', str(self.work if tools_dir is None else tools_dir)], cwd=self.work,
            capture_output=True, text=True, timeout=5,
        )

    def test_clean_prefix_includes_unacknowledged_complete_tail(self):
        result = self.score('replay', START + ack(1) + b'FSYNC 2 123', record(1) + record(2))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('2 records, 1 acknowledged', result.stdout)

    def test_replay_refuses_lost_altered_torn_duplicated_or_gapped_records(self):
        first = record(1)
        cases = (
            ('lost', START + ack(1) + ack(2), first, 'acknowledged sequence 2 is missing'),
            ('altered', START + ack(1).replace(record(1)[-32:].hex().encode(), b'0' * 64),
             first, 'FSYNC digest differs at sequence 1'),
            ('torn', START + ack(1), first + record(2)[:512], 'torn record at sequence 2'),
            ('duplicate', START + ack(1), first + first, 'expected sequence 2, found 1'),
            ('hole', START + ack(1), first + record(3), 'expected sequence 2, found 3'),
            ('wrong payload', START + ack(1), first + record(2, wrong_payload=True),
             'payload differs at sequence 2'),
            ('torn full block', START + ack(1), first + record(2)[:-32] + bytes(32),
             'stored digest differs at sequence 2'),
        )
        for name, serial, log, message in cases:
            with self.subTest(name=name):
                result = self.score('replay', serial, log)
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertIn(message, result.stderr)

    def test_replay_refuses_unbound_or_incomplete_capture(self):
        result = self.score('replay', START + ack(1), record(1), gap=True)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn('console gap', result.stderr)

        result = self.score('replay', START + b'kernel: ' + ack(1), record(1))
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn('invalid FSYNC receipt', result.stderr)

        result = self.score('replay', START + ack(1), record(1), off=False)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn('confirmed power-off', result.stderr)

        result = self.score('replay', START + ack(1), record(1), root_match=False)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn('root identity', result.stderr)

        result = self.score('replay', START + ack(1), record(1) + record(2, nonce='fedcba9876543210'))
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn('writer nonce differs at sequence 2', result.stderr)

    def test_aimed_replay_refuses_publication_progress_after_the_trigger(self):
        trigger = START + ack(1) + b'[breenfs] commit-window first-flush'
        valid = self.score('replay', trigger + b'\n', record(1), aimed=True)
        self.assertEqual(valid.returncode, 0, valid.stderr)
        for tail in (b'[breenfs] commit-window first-flush released\n',
                     b'[breenfs] commit-window between-rings\n', ack(2)):
            result = self.score('replay', trigger + b'\n' + tail, record(1) + record(2), aimed=True)
            self.assertEqual(result.returncode, 1, result.stdout)
            self.assertIn('advanced past the cut trigger', result.stderr)

    def test_replay_requires_an_acknowledged_write(self):
        result = self.score('replay', START, record(1))
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn('no acknowledged writes', result.stderr)

    def test_replay_requires_clean_full_check(self):
        result = self.score('replay', START + ack(1), record(1), check_status=1,
                            check_stdout=b'warning abandoned slot\n')
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn('breenfs-check --full exited 1', result.stderr)

    def test_replay_accepts_only_the_interrupted_ring_b_publication(self):
        tree = self.work / 'tree'
        (tree / 'crash').mkdir(parents=True)
        (tree / 'crash/log').write_bytes(record(1))
        subprocess.run([TOOLS / 'breenfs-mkfs', '--populate', tree, '--size', str(IMAGE_BYTES), self.image],
                       check=True, capture_output=True, timeout=5)
        with self.image.open('r+b') as disk:
            disk.seek((RING_FIRST[0] + 1) * BLOCK_BYTES)
            template = disk.read(BLOCK_BYTES)
        # Keep mkfs's valid roots and vary only the ring slots covering publication.
        a1 = superblock(template, 0, 1)
        a5 = superblock(template, 0, 5)
        b1 = superblock(template, 1, 1)
        serial = START + ack(1)
        for label, slots, capture, status, detail in (
            ('empty paired B', (a1,), serial, 0, 'ring-B-missing'),
            ('previous paired B', (a5, b1), serial, 0, 'ring-B-missing'),
            ('third finding', (a5, superblock(template, 1, 1, compat=2)), serial, 1, 'sets unknown compat'),
            ('wrong generation in paired B', (superblock(template, 0, 9), b1), serial, 1,
             'generation 1 has only its ring B copy'),
            ('empty paired B after a wrap', (a5,), serial, 1, 'generation 5 has only its ring A copy'),
            ('ring A missing', (b1,), serial, 1, 'generation 1 has only its ring B copy'),
            ('torn paired slot', (a5, superblock(template, 1, 1, torn=True)), serial, 1, 'error S1 slot B1'),
            ('older ring B missing', (a1, superblock(template, 0, 2), superblock(template, 1, 2)),
             serial, 1, 'generation 1 has only its ring A copy'),
            ('acknowledged record missing', (a1,), serial + ack(2), 1, 'acknowledged sequence 2 is missing'),
        ):
            with self.subTest(label=label):
                with self.image.open('r+b') as disk:
                    for start in RING_FIRST:
                        disk.seek(start * BLOCK_BYTES)
                        disk.write(bytes(4 * BLOCK_BYTES))
                    for block, data in slots:
                        disk.seek(block * BLOCK_BYTES)
                        disk.write(data)
                result = self.score('replay', capture, tools_dir=TOOLS)
                self.assertEqual(result.returncode, status, result.stderr or result.stdout)
                self.assertIn(detail, result.stderr if status else result.stdout)

    def test_bit_flip_requires_typed_failure_and_the_exact_checker_block(self):
        typed = b'[breenfs] corrupt-read block=120 result=ReadError::Data\n'
        bad_data = b'[breenfs] corrupt-read block=120 result=returned-data\n'
        cases = (
            ('verified', typed, CRC_ERROR, 0, 'block 120'),
            ('bad data', bad_data, CRC_ERROR, 1, 'returned-data'),
            ('missing read', b'', CRC_ERROR, 1, 'typed read'),
            ('different block', typed, CRC_ERROR.replace(b'block 120 ', b'block 1200 '),
             1, 'did not name injected block 120'),
            ('generic I/O error', typed.replace(b'ReadError::Data', b'EIO'), CRC_ERROR, 1, 'EIO'),
        )
        for name, read, diagnostic, status, message in cases:
            with self.subTest(name=name):
                result = self.score('bit-flip', INJECTION + read, check_status=4,
                                    check_stdout=diagnostic)
                self.assertEqual(result.returncode, status, result.stderr)
                self.assertIn(message, result.stderr if status else result.stdout)

    def test_bit_flip_rejects_other_errors_and_a_warning_exit(self):
        typed = b'[breenfs] corrupt-read block=120 result=ReadError::Data\n'
        for check_status, diagnostics in (
            (4, CRC_ERROR + CRC_ERROR.replace(b'block 120 ', b'block 121 ')),
            (4, CRC_ERROR + b'error S4 bad free count\n'),
            (1, CRC_ERROR),
        ):
            with self.subTest(check_status=check_status, diagnostics=diagnostics):
                result = self.score('bit-flip', INJECTION + typed, check_status=check_status,
                                    check_stdout=diagnostics)
                self.assertEqual(result.returncode, 1, result.stdout)


if __name__ == '__main__':
    unittest.main()
