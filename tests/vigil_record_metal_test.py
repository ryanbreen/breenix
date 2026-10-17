"""Metal registration uses the shared best-effort Vigil start path, without a boot."""

import os
from pathlib import Path
import platform
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


@unittest.skipUnless(platform.system() == 'Darwin', 'registration is macOS-only')
class VigilRecordMetalTest(unittest.TestCase):
    def test_metal_platform_and_modes_reach_cli(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            cli = root / 'vigil-cli'
            args = root / 'args'
            cli.write_text('#!/bin/sh\n'
                           'if [ "$#" -eq 1 ]; then echo "usage: vigil-cli breenix start"; exit 64; fi\n'
                           'printf "%s\\n" "$@" > "$METAL_TEST_ARGS"\n'
                           'if [ "$METAL_TEST_REJECT" = 1 ]; then exit 64; fi\n'
                           'echo metal-test-boot\n')
            cli.chmod(0o755)
            env = {k: v for k, v in os.environ.items()
                   if k not in ('VIGIL_BOOT_ID', 'BREENIX_VIGIL_CPUS')}
            env.update(VIGIL_CLI=str(cli), METAL_TEST_ARGS=str(args))
            for mode, suite in [('default', ''), ('shell', ''), ('probe', ''), ('suite', 'smoke')]:
                for reject in ('0', '1'):
                    with self.subTest(mode=mode, reject=reject):
                        env['METAL_TEST_REJECT'] = reject
                        result = subprocess.run(
                            [str(ROOT / 'scripts/vigil-record.sh'), 'start', 'metal', mode, suite,
                             str(root / 'serial.txt'), '', '', '', '0' * 40],
                            env=env, capture_output=True, text=True, check=True)
                        self.assertEqual(result.stdout.strip(), '' if reject == '1' else 'metal-test-boot')
                        forwarded = args.read_text().splitlines()
                        self.assertEqual(forwarded[forwarded.index('--platform') + 1], 'metal')
                        if suite:
                            self.assertEqual(forwarded[forwarded.index('--suite') + 1], suite)
                        elif mode != 'default':
                            self.assertEqual(forwarded[forwarded.index('--mode') + 1], mode)
                        else:
                            self.assertNotIn('--mode', forwarded)


if __name__ == '__main__':
    unittest.main()
