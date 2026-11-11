#!/usr/bin/env python3
"""Check the desktop deployment inputs and build-only boundary."""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class BootArtifactsTest(unittest.TestCase):
    def test_manifest_and_launcher_paths(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            (root / 'scripts').mkdir()
            shutil.copy(ROOT / 'scripts/boot-artifacts.py', root / 'scripts')
            kernel = root / 'target/aarch64-breenix-kernel/release/kernel-aarch64'
            disk = root / 'target/ext2-aarch64.img'
            bundle = root / 'cache/QEMU-GL.app'
            binary = bundle / 'Contents/MacOS/qemu-system-aarch64'
            for path, content in ((kernel, b'kernel'), (disk, b'disk'), (binary, b'qemu')):
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(content)
            builder = root / 'scripts/build-qemu-gl.sh'
            builder.write_text('#!/bin/sh\nprintf "%s\\n" "$FAKE_QEMU"\n')
            builder.chmod(0o755)
            env = dict(os.environ, FAKE_QEMU=str(binary))
            output = subprocess.check_output(['python3', str(root / 'scripts/boot-artifacts.py'),
                                              '--mode', 'desktop', '--gpu-gl', '--json'], env=env)
            items = json.loads(output)
            self.assertEqual({item['destination'] for item in items}, {
                'target/aarch64-breenix-kernel/release/kernel-aarch64',
                'target/ext2-aarch64.img', 'QEMU-GL.app'})
            by_path = {item['path']: item for item in items}
            for path in (kernel, disk):
                self.assertEqual(by_path[str(path)]['sha256'], hashlib.sha256(path.read_bytes()).hexdigest())
            self.assertRegex(by_path[str(bundle)]['sha256'], r'^[0-9a-f]{64}$')
            binary.write_bytes(b'changed')
            changed = json.loads(subprocess.check_output(['python3', str(root / 'scripts/boot-artifacts.py'),
                                                           '--mode', 'desktop', '--gpu-gl', '--json'], env=env))
            self.assertNotEqual(by_path[str(bundle)]['sha256'],
                                {item['path']: item for item in changed}[str(bundle)]['sha256'])
            launcher = (ROOT / 'scripts/boot-interactive.sh').read_text()
            target_paths = set(re.findall(r'\$ROOT/(target/[A-Za-z0-9_./-]+)', launcher))
            sdk_suite_only = {'target/ext2-aarch64-sdk.img'}
            self.assertEqual(target_paths, {item['destination'] for item in items if item['destination'].startswith('target/')} | sdk_suite_only)
            self.assertRegex(launcher, r'(?s)\[ -f "\$KERNEL" \].*\[ -f "\$DISK" \].*if \[ "\$BUILD_ONLY" -eq 1 \]; then\s+exit 0\s+fi.*host_slot_acquire mac-boot')
            self.assertRegex(launcher, r'--build-only\) BUILD_ONLY=1; shift ;;')


if __name__ == '__main__':
    unittest.main()
