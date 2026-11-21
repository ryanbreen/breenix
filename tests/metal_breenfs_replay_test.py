"""Score synthetic evidence through the cold-readback checker CLI."""

import hashlib
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest


CHECKER = Path(__file__).resolve().parents[1] / 'scripts/metal/breenfs_replay.py'
START = b'SUITE filesystem START cases=1 only=power-cut/writer\n'
INJECTION = b'[breenfs] corrupt-inject block=120 offset=14 bit=2\n'
CRC_ERROR = b'error C1 block 120 (data REF 120+1): CRC 0x12345678, REF records 0x87654321\n'


def record(seq, *, wrong_payload=False):
    words = [seq ^ index for index in range(507)]
    if wrong_payload:
        words[0] ^= 1
    body = struct.pack('<508Q', seq, *words)
    return body + hashlib.sha256(body).digest()


def ack(seq):
    return f'FSYNC {seq} {record(seq)[-32:].hex()}\n'.encode()


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

    def score(self, mode, serial, log=b'', *, check_status=0, check_stdout=b''):
        (self.work / 'crash-log').write_bytes(log)
        (self.work / 'serial.txt').write_bytes(serial)
        (self.work / 'check-stdout').write_bytes(check_stdout)
        (self.work / 'check-status').write_text(str(check_status))
        return subprocess.run(
            [sys.executable, str(CHECKER), mode, 'serial.txt', self.image.name,
             '--tools-dir', str(self.work)], cwd=self.work,
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

    def test_replay_requires_clean_full_check(self):
        result = self.score('replay', START + ack(1), record(1), check_status=1,
                            check_stdout=b'warning abandoned slot\n')
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn('breenfs-check --full exited 1', result.stderr)

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


if __name__ == '__main__':
    unittest.main()
