"""A multi-suite boot stays one list record even when it dies mid-sequence."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from vigil_record_fixture import isolated_commands

ROOT = Path(__file__).resolve().parents[1]


class VigilRecordSequenceTest(unittest.TestCase):
    def test_died_mid_sequence_is_never_split_into_unreached_suite_records(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            serial = root / 'serial'
            serial.write_text('SUITE directories START cases=1\n'
                              'SUITE directories CASE example PASS ms=1\n'
                              'SUITE directories DONE passed=1 failed=0 skipped=0 total=1\n'
                              'SUITE processes START cases=1\n')
            cli = root / 'vigil-cli'
            cli.write_text('#!/bin/sh\n'
                           'if [ "$#" = 1 ]; then echo "breenix start --arch --cpus"; exit 64; fi\n'
                           'printf "%s\\n" "$@" >> "$FIXTURE_ARGS"\n'
                           'echo sequence-boot\n')
            cli.chmod(0o755)
            env = {k: v for k, v in os.environ.items()
                   if k not in ('VIGIL_BOOT_ID', 'BREENIX_VIGIL_DISPLAY', 'BREENIX_VIGIL_CPUS')}
            isolated_commands(env, root)
            # Exercise the Darwin recorder on any host without touching real boot records.
            uname = root / 'fixture-tools/uname'
            uname.write_text('#!/bin/sh\necho Darwin\n')
            uname.chmod(0o755)
            env.update(VIGIL_CLI=str(cli), FIXTURE_ARGS=str(root / 'args'))
            for verb in ('start', 'record'):
                with self.subTest(verb=verb):
                    (root / 'args').unlink(missing_ok=True)
                    result = subprocess.run([str(ROOT / 'scripts/vigil-record.sh'), verb,
                        'qemu', 'suite', 'directories,processes,signals', str(serial), '', '',
                        'sequence-boot', 'a' * 40, '1', '2', '143'],
                        env=env, capture_output=True, text=True, check=True)
                    args = (root / 'args').read_text().splitlines()
                    self.assertEqual(args.count('breenix'), 1)
                    self.assertEqual(args[args.index('--suite') + 1], 'directories,processes,signals')
                    self.assertEqual(args[args.index('--serial') + 1], str(serial))
                    self.assertEqual(args[args.index('--id') + 1], 'sequence-boot')
                    self.assertEqual(result.stdout.strip(), 'sequence-boot')


if __name__ == '__main__':
    unittest.main()
