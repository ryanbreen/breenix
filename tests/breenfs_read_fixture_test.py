"""Exercise the launcher's fixture selection without building or starting QEMU."""
import json
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ReadFixture(unittest.TestCase):
    def test_only_filesystem_gets_the_last_readonly_disk(self):
        source = (ROOT / 'scripts/boot-interactive.sh').read_text()
        self.assertIn('BREENFS_READ_ARGS=()', source)
        block = source.split('BREENFS_READ_ARGS=()', 1)[1].split('# End breenfs read fixture.', 1)[0]
        for mode, suites, enabled in (
            ('suite', 'filesystem', True),
            ('suite', 'filesystem,directories,files-io', True),
            ('suite', 'directories,filesystem,files-io', True),
            ('suite', 'directories,files-io', False),
            ('suite', 'not-filesystem', False),
            ('tests', 'filesystem', False),
            ('default', '', False),
        ):
            with self.subTest(mode=mode, suites=suites):
                script = '''set -euo pipefail
ROOT=/repo
SERIAL_LOG=/run/serial.log
WRITABLE=/run/root.img
run_build() { :; }
python3() { :; }
BREENFS_READ_ARGS=()
'''+block+'''
printf '%s\\n' "${BREENFS_READ_ARGS[@]+${BREENFS_READ_ARGS[@]}}"
'''
                result = subprocess.run(['bash', '-c', script], text=True, capture_output=True,
                                        env={'PATH': '/usr/bin:/bin', 'MODE': mode, 'SUITE': suites})
                self.assertEqual(result.returncode, 0, result.stderr)
                args = result.stdout.splitlines() if enabled else result.stdout.strip().splitlines()
                self.assertEqual(args, [
                    '-device', 'virtio-blk-device,drive=breenfs-read', '-drive',
                    'if=none,id=breenfs-read,format=raw,readonly=on,file=/run/breenfs-read.img'
                ] if enabled else [])
        # MMIO allocation descends: placing this first keeps root/home indices intact.
        launch = source[source.index('python3 "$ROOT/scripts/watch-qemu.py"'):]
        fixture = launch.index('${BREENFS_READ_ARGS[@]+"${BREENFS_READ_ARGS[@]}"}')
        self.assertLess(fixture, launch.index('${FIXTURE_ARGS[@]+"${FIXTURE_ARGS[@]}"}'))
        self.assertLess(fixture, launch.index('${CAPTURE_ARGS[@]+"${CAPTURE_ARGS[@]}"}'))
        self.assertLess(fixture, launch.index('-device virtio-blk-device,drive=ext2'))


if __name__ == '__main__':
    unittest.main()
