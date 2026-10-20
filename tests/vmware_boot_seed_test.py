#!/usr/bin/env python3
"""Check the real host seed writer without booting a VM."""
import importlib.util
from pathlib import Path
import struct
import tempfile
from unittest.mock import patch
import uuid
import unittest

spec = importlib.util.spec_from_file_location('boot_seed', Path(__file__).resolve().parents[1] / 'scripts/vmware/boot-seed.py')
boot_seed = importlib.util.module_from_spec(spec)
spec.loader.exec_module(boot_seed)


def gpt_image():
    image = bytearray(4096)
    image[512:520] = b'EFI PART'
    struct.pack_into('<QII', image, 512 + 72, 2, 2, 128)
    # The ESP follows another partition, so parsing must inspect each entry.
    image[1152:1168] = uuid.UUID('C12A7328-F81F-11D2-BA4B-00A0C93EC93B').bytes_le
    struct.pack_into('<Q', image, 1152 + 32, 2048)
    return image


class HostSeedTest(unittest.TestCase):
    def test_each_write_replaces_the_previous_seed_with_64_fresh_bytes(self):
        with tempfile.TemporaryDirectory() as work:
            seed = Path(work) / 'seed'
            boot_seed.write_seed(seed)
            first = seed.read_bytes()
            boot_seed.write_seed(seed)
            second = seed.read_bytes()
            self.assertEqual(len(first), 64)
            self.assertEqual(len(second), 64)
            self.assertNotEqual(first, second)

    def test_gpt_esp_and_malformed_tables(self):
        with tempfile.TemporaryDirectory() as work:
            disk = Path(work) / 'boot.img'
            disk.write_bytes(gpt_image())
            self.assertEqual(boot_seed.esp_offset(disk), 2048 * 512)
            for data in [b'', bytes(4096), gpt_image()[:1200]]:
                disk.write_bytes(data)
                with self.assertRaises(ValueError):
                    boot_seed.esp_offset(disk)
            for count, size in [(4097, 128), (2, 127), (2, 4097)]:
                data = gpt_image()
                struct.pack_into('<II', data, 512 + 80, count, size)
                disk.write_bytes(data)
                with self.assertRaises(ValueError):
                    boot_seed.esp_offset(disk)
            disk.write_bytes(gpt_image()[:1152] + bytes(4096 - 1152))
            with self.assertRaisesRegex(ValueError, 'no EFI System Partition'):
                boot_seed.esp_offset(disk)

    def test_stage_targets_only_the_per_run_esp(self):
        with tempfile.TemporaryDirectory() as work:
            cached = Path(work) / 'cached.vmdk'
            bundle = Path(work) / 'run.vmwarevm'
            bundle.mkdir()
            destination = bundle / 'boot.vmdk'
            cached.write_bytes(b'seed-free cached image')
            calls = []

            def run(args, check):
                self.assertTrue(check)
                calls.append(args)
                if len(calls) == 1:
                    self.assertEqual(args[:7], ['qemu-img', 'convert', '-f', 'vmdk', '-O', 'raw', str(cached)])
                    raw = Path(args[7])
                    self.assertNotEqual(raw.parent.parent, bundle)
                    raw.write_bytes(gpt_image())
                elif len(calls) == 2:
                    raw = Path(calls[0][7])
                    self.assertEqual(args[:4], ['mcopy', '-o', '-i', f'{raw}@@1048576'])
                    self.assertEqual(args[5], '::/EFI/BREENIX/random-seed')
                    self.assertEqual(len(Path(args[4]).read_bytes()), 64)
                else:
                    self.assertEqual(args, ['qemu-img', 'convert', '-f', 'raw', '-O', 'vmdk', calls[0][7], str(destination)])

            with patch.object(boot_seed.subprocess, 'run', side_effect=run):
                boot_seed.stage(str(cached), str(destination))
            self.assertEqual(len(calls), 3)
            self.assertEqual(cached.read_bytes(), b'seed-free cached image')
            self.assertFalse(Path(calls[0][7]).exists())
            with patch.object(boot_seed.subprocess, 'run') as command:
                with self.assertRaises(ValueError):
                    boot_seed.stage(str(cached), str(cached.parent / '.' / cached.name))
                command.assert_not_called()


if __name__ == '__main__':
    unittest.main()
