#!/usr/bin/env python3
"""Census integrity tests: corrupt, omitted and ineligible evidence must fail."""
import base64
import hashlib
import json
from pathlib import Path
import sys
import tempfile
import unittest

TOOLS = Path(__file__).resolve().parents[1] / 'scripts/metal'
sys.path.insert(0, str(TOOLS))
from metal_census import Census, REQUIRED, extract_record, validate_config, REQUIRED_CONFIG, FORBIDDEN_CONFIG


def frame(name, data):
    digest = hashlib.sha256(data).hexdigest()
    return [f'FILE {name} {len(data)} {digest}', f'BEGIN {name}',
            *[base64.b64encode(data[i:i + 57]).decode() for i in range(0, len(data), 57)], f'END {name}']


def capture(omit=None):
    files = {name: b'fixture\x00\xff' for name in REQUIRED if name != 'census/manifest'}
    files.update({'pci/0000:00:00.0/config': bytes(range(256)), 'memmap/0/type': b'System RAM\n'})
    manifest = ''.join(f'{k} {len(v)} {hashlib.sha256(v).hexdigest()}\n' for k, v in files.items()).encode()
    files['census/manifest'] = manifest
    lines = ['firmware text', 'metal-linux-start']
    for name, data in files.items():
        if name != omit:
            lines.extend(frame(name, data))
    return lines + ['metal-linux-files-done', 'metal-linux-ok'], files


class CensusTest(unittest.TestCase):
    def test_config_accepts_smp_hotplug_and_rejects_drivers(self):
        config = '\n'.join('CONFIG_' + name + '=y' for name in REQUIRED_CONFIG)
        config += '\nCONFIG_INITRAMFS_SOURCE="fixture"\nCONFIG_HOTPLUG_CPU=y\n'
        validate_config(config)
        for symbol in FORBIDDEN_CONFIG:
            for value in ('y', 'm'):
                with self.subTest(symbol=symbol, value=value), self.assertRaises(ValueError):
                    validate_config(config + 'CONFIG_' + symbol + '=' + value + '\n')
        for symbol in REQUIRED_CONFIG:
            with self.subTest(missing=symbol), self.assertRaises(ValueError):
                validate_config(config.replace('CONFIG_' + symbol + '=y', 'CONFIG_' + symbol + '=n'))
        with self.assertRaises(ValueError):
            validate_config(config.replace('CONFIG_INITRAMFS_SOURCE="fixture"', 'CONFIG_INITRAMFS_SOURCE=""'))

    def parse(self, lines):
        census = Census()
        for line in lines:
            census.feed(line)
        return census

    def test_binary_round_trip_and_atomic_output(self):
        lines, files = capture()
        census = self.parse(lines)
        with tempfile.TemporaryDirectory() as d:
            out = Path(d) / 'facts'
            census.write(out)
            self.assertEqual({str(p.relative_to(out)): p.read_bytes() for p in out.rglob('*') if p.is_file()}, files)
            with self.assertRaises(FileExistsError):
                census.write(out)

    def test_corrupt_valid_base64_fails_checksum(self):
        lines, _ = capture()
        index = lines.index('BEGIN acpi/APIC') + 1
        lines[index] = 'AAAA'
        with self.assertRaises(ValueError):
            self.parse(lines)

    def test_dropped_whole_optional_file_fails_manifest(self):
        lines, _ = capture(omit='pci/0000:00:00.0/config')
        with self.assertRaises(ValueError):
            self.parse(lines)

    def test_missing_required_file_and_early_success_fail(self):
        for lines in (capture(omit='acpi/APIC')[0], ['metal-linux-start', 'metal-linux-ok']):
            with self.subTest(lines=lines[-2:]), self.assertRaises(ValueError):
                self.parse(lines)

    def test_path_traversal_and_duplicate_fail(self):
        for name in ('../escape', '/escape', 'a/../escape', 'a//escape'):
            with self.subTest(name=name), self.assertRaises(ValueError):
                self.parse(['metal-linux-start', *frame(name, b'')])
        with self.assertRaises(ValueError):
            self.parse(['metal-linux-start', *frame('acpi/APIC', b''), *frame('acpi/APIC', b'')])

    def test_truncated_frame_cannot_be_saved(self):
        census = self.parse(['metal-linux-start', *frame('acpi/APIC', b'abc')[:-1]])
        with self.assertRaises(ValueError):
            census.write('unused')

    def test_console_noise_inside_frame_fails(self):
        with self.assertRaises(ValueError):
            self.parse(['metal-linux-start', *frame('acpi/APIC', b'abc')[:2], 'kernel warning'])

    def test_capture_gap_and_interrupted_record_are_ineligible(self):
        lines, _ = capture()
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            record = root / 'record'
            record.mkdir()
            (record / 'ended').touch()
            (record / 'serial.txt').write_text('\n'.join(lines))
            (record / 'events.jsonl').write_text(json.dumps({'event': 'console-gap-start'}) + '\n')
            (record / 'exit-status').write_text('0\n')
            with self.assertRaises(ValueError):
                extract_record(record, root / 'facts')
            (record / 'events.jsonl').write_text('')
            (record / 'exit-status').write_text('137\n')
            with self.assertRaises(ValueError):
                extract_record(record, root / 'facts')
            (record / 'exit-status').write_text('0\n')
            self.assertEqual(extract_record(record, root / 'facts'), len(capture()[1]))


if __name__ == '__main__':
    unittest.main()
