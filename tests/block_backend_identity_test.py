#!/usr/bin/env python3
"""C4 (E24 ledger 309/314): execute backend identity methods without hardware."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def braced(source, start):
    opening = source.index('{', start)
    depth, end = 1, opening + 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[start:end]


class BackendIdentity(unittest.TestCase):
    def execute(self, path, implementation, fields, values, prelude=''):
        source = (ROOT / path).read_text()
        body = braced(source, source.index(implementation))
        self.assertTrue('fn identity(&self)' in body, f'{path}: stable identity override missing')
        method = braced(body, body.index('fn identity(&self)'))
        read_fields = '\n'.join(f'let _ = &a.{field.split(":")[0].strip()};' for field in fields.split(','))
        harness = f'''{prelude}
struct Device {{ {fields} }}
impl Device {{ {method} }}
fn main() {{
    let a = Device {{ {values[0]} }};
    let reopened = Device {{ {values[0]} }};
    let other = Device {{ {values[1]} }};
    {read_fields}
    assert_eq!(a.identity(), reopened.identity(), "same backend reopened");
    assert_ne!(a.identity(), other.identity(), "different backend coordinates");
}}
'''
        with tempfile.TemporaryDirectory(prefix='block-identity-') as tmp:
            src, exe = Path(tmp) / 'identity.rs', Path(tmp) / 'identity'
            src.write_text(harness)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + [os.environ.get('RUSTC', 'rustc'), '--edition=2021',
                           '-Dwarnings', str(src), '-o', str(exe)], check=True)
            subprocess.run([str(exe)], check=True)

    def test_virtio_both_architectures(self):
        source = (ROOT / 'kernel/src/block/virtio.rs').read_text()
        implementations = source.count('impl BlockDevice for VirtioBlockWrapper')
        self.assertEqual(implementations, 2)
        # Each architecture's actual method is executed; constructor wiring is
        # checked as well so the coordinate cannot be left uninitialized.
        for arch, section in zip(('x86_64', 'aarch64'), source.split('impl BlockDevice for VirtioBlockWrapper')[1:]):
            with self.subTest(arch=arch):
                prefix = source[:source.index(section)]
                self.assertTrue('device_index,' in prefix, f'{arch}: constructor must retain backend index')
                # Select this impl by its full unique prefix in the source.
                marker = 'impl BlockDevice for VirtioBlockWrapper' + section[:section.index('fn read_block')]
                self.execute('kernel/src/block/virtio.rs', marker, 'device_index: usize',
                             ('device_index: 0', 'device_index: 1'))

    def test_ahci_single_controller_ports(self):
        self.execute('kernel/src/drivers/ahci/mod.rs', 'impl BlockDevice for AhciBlockDevice',
                     'port_num: usize', ('port_num: 0', 'port_num: 1'))

    def test_nvme_controller_namespaces(self):
        self.execute('kernel/src/block/nvme.rs', 'impl BlockDevice for NvmeBlockDevice',
                     'device_index: usize', ('device_index: 0', 'device_index: 1'))

    def test_immutable_fixture_reopens_the_same_image(self):
        source = (ROOT / 'kernel/src/block/testing_fixture.rs').read_text()
        body = braced(source, source.index('impl BlockDevice for ReadOnlyFixture'))
        self.assertTrue('fn identity(&self)' in body, 'fixture must identify its backing image')
        method = braced(body, body.index('fn identity(&self)'))
        self.assertIn('IMAGE.as_ptr()', method)

    def test_partition_start_and_length(self):
        self.execute('kernel/src/block/partition.rs', 'impl BlockDevice for Partition',
                     'disk: Parent, first: u64, sectors: u64',
                     ('disk: Parent(7), first: 128, sectors: 64',
                      'disk: Parent(7), first: 128, sectors: 32'),
                     'struct Parent(usize); impl Parent { fn identity(&self) -> usize { self.0 } }')
        self.execute('kernel/src/block/partition.rs', 'impl BlockDevice for Partition',
                     'disk: Parent, first: u64, sectors: u64',
                     ('disk: Parent(7), first: 128, sectors: 64',
                      'disk: Parent(7), first: 256, sectors: 64'),
                     'struct Parent(usize); impl Parent { fn identity(&self) -> usize { self.0 } }')


if __name__ == '__main__':
    unittest.main()
