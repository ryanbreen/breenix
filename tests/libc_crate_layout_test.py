#!/usr/bin/env python3
"""Compile the libc crate for both Breenix targets, which evaluates the per-arch layout
pins in libs/libc/src/unix/breenix/layout.rs.

A std build compiles the crate only for the architecture it builds, so an x86_64-only
layout error would pass every ARM64 build; this checks both from the host.
"""
import os
from pathlib import Path
import re
import subprocess
import unittest

ROOT = Path(__file__).resolve().parent.parent
BREENIX = ROOT / 'libs/libc/src/unix/breenix'
TARGET_DIR = Path(os.environ.get('TMPDIR', '/tmp')) / 'breenix-libc-layout'


class LayoutTests(unittest.TestCase):
    def check(self, arch):
        env = {k: v for k, v in os.environ.items() if k not in ('CARGO_TARGET_DIR', 'RUSTFLAGS')}
        result = subprocess.run(
            ['cargo', 'check', '--manifest-path', str(ROOT / 'libs/libc/Cargo.toml'),
             '--target', str(ROOT / f'{arch}-breenix.json'), '-Z', 'build-std=core',
             '--target-dir', str(TARGET_DIR)],
            cwd=ROOT, env=env, capture_output=True, text=True)
        errors = [line for line in result.stderr.splitlines() if line.startswith('error')]
        self.assertEqual(result.returncode, 0, f'{arch}: ' + '\n'.join(errors or result.stderr.splitlines()[-20:]))

    def test_layouts_hold_on_aarch64(self):
        self.check('aarch64')

    def test_layouts_hold_on_x86_64(self):
        self.check('x86_64')

    def test_the_pins_are_compiled(self):
        source = (BREENIX / 'mod.rs').read_text()
        self.assertRegex(source, re.compile(r'^mod layout;$', re.M))
        pins = (BREENIX / 'layout.rs').read_text()
        for name in ('stat', 'stat64', 'epoll_event', 'dirent', 'sigaction', 'msghdr', 'cmsghdr',
                     'sockaddr', 'sockaddr_in', 'sockaddr_in6', 'sockaddr_un', 'sockaddr_storage',
                     'timespec', 'timeval', 'cpu_set_t'):
            self.assertRegex(pins, rf'pin!\({name},', name)


if __name__ == '__main__':
    unittest.main()
