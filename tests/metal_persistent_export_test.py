"""Persistent export checks with synthetic partitions and in-memory HTTP requests."""
import hashlib
import io
import re
import stat
import unittest
from unittest.mock import patch

import metal_tools_test as existing
import metal_persist

m = existing.m


class Upload:
    def __init__(self, path, body, count):
        self.path, self.rfile, self.connection = path, io.BytesIO(body), self
        self.headers = {'Content-Length': str(count)}
        self.status = None

    def settimeout(self, _):
        pass

    def send_response(self, status):
        self.status = status

    def send_error(self, status, *_):
        self.status = status

    def send_header(self, *_):
        pass

    def end_headers(self):
        pass


class PersistentExportTest(existing.StateTest):
    boot_id = '20261121T000000Z-metal'

    def setUp(self):
        super().setUp()
        self.directory = m.boot_dir(self.boot_id)
        self.directory.mkdir(parents=True)
        m.write_json(self.directory / 'run.json', {'id': self.boot_id})
        m.write_json(m.STATE / m.LEASE, dict(id=self.boot_id, payload='persistent-export', state='claimed'))
        self.layout = metal_persist.new_layout()
        self.parts = self.layout['partitions']
        self.body = {role: role.encode().ljust(1024, b'\0') for role in ('system', 'home')}
        self.digests = {role: hashlib.sha256(data).hexdigest() for role, data in self.body.items()}

    def prepare(self):
        import metal_export
        # Shrink only the fixture layout to drive complete streaming PUTs.
        sizes = {role: len(body) for role, body in self.body.items()}
        with patch.dict(metal_persist.SIZES, sizes), \
             patch.object(metal_export.metal_image, 'assemble', side_effect=lambda image, _: image.touch()), \
             patch.object(metal_export.metal_image, 'stub_efi', return_value={}):
            for role, size in sizes.items():
                self.parts[role]['last'] = self.parts[role]['first'] + size // metal_persist.SECTOR - 1
            m.write_json(m.STATE / 'disk.json', dict(layout=self.layout))
            metal_export.prepare(self.directory, self.boot_id, {'BOOT_URL': 'http://firmware.invalid/boot.efi'},
                                 self.directory / 'stub.efi')
        private = m.STATE / 'persistent-exports' / (self.boot_id + '.partial')
        request = m.read_json(private / 'request.json')
        m.update_lease(self.boot_id, state='powered')
        return private, {part['role']: part['route'] for part in request}

    def test_selection_refuses_test_overlap_and_unknown_guid(self):
        import metal_export
        self.parts['system']['first'] = self.parts['test']['last'] + 1 - 2048
        with self.assertRaises(ValueError):
            self.prepare()
        self.parts['system']['first'] = self.parts['test']['last'] + 1
        private, routes = self.prepare()
        unknown = routes['system'].rsplit('/', 1)[0] + '/' + self.layout['diskGuid'] + '/0000000000000000'
        reply = Upload(unknown, self.body['system'], 1024)
        metal_export.receive(reply)
        self.assertEqual(reply.status, 404)
        self.assertFalse((private / 'system.img').exists())
        reply = Upload(routes['system'] + '/0000000000000000', self.body['system'], 1024)
        m.update_lease(self.boot_id, state='claimed')
        metal_export.receive(reply)
        self.assertEqual(reply.status, 404)
        self.assertFalse((private / 'system.img').exists())

    def test_firmware_export_has_no_block_write_or_flush_call(self):
        source = (existing.TOOLS / 'efi-stub/src/persistent_export.rs').read_text()
        self.assertRegex(source, r'\bread_blocks\b')
        self.assertNotRegex(source, r'(?i)\b(?:write_?blocks|flush_?blocks)\b\s*\)?\s*\(')

    def test_firmware_export_disables_watchdog_before_read_blocks(self):
        directory = existing.TOOLS / 'efi-stub/src'
        main = (directory / 'main.rs').read_text()
        source = (directory / 'persistent_export.rs').read_text()
        self.assertIn('persistent_export::run(boot, volume, image)', main)
        dispatch = source.split('unsafe fn run(', 1)[1].split('\n}', 1)[0]
        transfer = source.split('unsafe fn export(', 1)[1].split('\n}', 1)[0]
        self.assertRegex(transfer, r'\bread_blocks\)\s*\(')

        before_first_export = dispatch.split('\n    export(', 1)[0]
        self.assertNotEqual(before_first_export, dispatch)
        self.assertRegex(
            before_first_export,
            r'(?m)^    ok\(\(boot\.set_watchdog_timer\)\(0,\s*0,\s*0,\s*null\(\)\)\)\?;',
            'the export must successfully disable the UEFI watchdog before reading a partition',
        )

    def test_firmware_export_transport_is_read_only(self):
        directory = existing.TOOLS / 'efi-stub/src'
        export = (directory / 'persistent_export.rs').read_text()
        transport = (directory / 'suite_disk.rs').read_text()
        shared = {'open_root', 'read_file', 'upload'}
        self.assertEqual(set(re.findall(r'suite_disk::(\w+)\(', export)), shared)
        for name in sorted(shared | {'boot_http_binding', 'new', 'drop'}):
            end = r'    }' if name in {'new', 'drop'} else r'}'
            found = re.search(rf'(?ms)^ *[^\n]*\bfn {name}\([^\n]*.*?^{end}$', transport)
            with self.subTest(function=name):
                self.assertIsNotNone(found)
                self.assertNotRegex(found[0], r'(?i)\b(?:write_?blocks|flush_?blocks|FILE_MODE_WRITE|FILE_MODE_CREATE'
                                             r'|install|operate|run)\b|\.\s*(?:write|flush|delete|set_info)\b')
                if name == 'read_file':
                    self.assertIn('FILE_MODE_READ', found[0])

    def test_multipart_copy_checks_order_each_digest_and_the_whole(self):
        import metal_export
        # Two unequal sector-aligned parts without a GiB allocation.
        split = 1024
        self.body = {role: body + bytes(range(256)) * 2 for role, body in self.body.items()}
        self.digests = {role: hashlib.sha256(data).hexdigest() for role, data in self.body.items()}
        with patch.object(metal_export, 'PART_BYTES', split, create=True):
            private, routes = self.prepare()
            parts = {}
            for role, route in routes.items():
                self.assertEqual(self.put(route, 1, self.body[role][split:]), 400)
                for index, data in enumerate((self.body[role][:split], self.body[role][split:])):
                    reply = Upload(f'{route}/{index:016x}', data[:-1], len(data))
                    metal_export.receive(reply)
                    self.assertEqual(reply.status, 400)
                    self.assertEqual(self.put(route, index, data), 200)
                    self.assertEqual(self.put(route, index, data), 400)
                    parts[role, index] = (len(data), hashlib.sha256(data).hexdigest())
                parts[role, None] = (len(self.body[role]), self.digests[role])

            for key, receipt in (
                (('system', 1), None),
                (('system', 1), (511, parts['system', 1][1])),
                (('system', 0), (split, '0' * 64)),
                (('home', None), (1536, '0' * 64)),
            ):
                bad = dict(parts)
                if receipt is None:
                    del bad[key]
                else:
                    bad[key] = receipt
                with self.subTest(key=key, receipt=receipt), self.assertRaises(ValueError):
                    metal_export.publish(self.directory, self.boot_id, bad)
                self.assertNotIn('persistentExport', m.read_json(self.directory / 'run.json'))
                self.assertFalse(private.with_suffix('').exists())

            metal_export.publish(self.directory, self.boot_id, parts)
            rows = m.read_json(self.directory / 'run.json')['persistentExport']['partitions']
            for row in rows:
                role = row['role']
                self.assertEqual(row['hostSha256'], self.digests[role])
                self.assertEqual(row['firmwareSha256'], self.digests[role])
                self.assertEqual(len(row['parts']), 2)
                for index, part in enumerate(row['parts']):
                    count, digest = parts[role, index]
                    self.assertEqual((part['index'], part['offset']), (index, index * split))
                    self.assertEqual((part['hostBytes'], part['firmwareBytes']), (count, count))
                    self.assertEqual((part['hostSha256'], part['firmwareSha256']), (digest, digest))
                self.assertEqual((private.with_suffix('') / f'{role}.img').read_bytes(), self.body[role])

    def put(self, route, index, data):
        import metal_export
        reply = Upload(f'{route}/{index:016x}', data, len(data))
        metal_export.receive(reply)
        return reply.status

    def test_copy_requires_full_counts_and_matching_digests_before_recording(self):
        import metal_export
        private, routes = self.prepare()
        self.assertEqual(stat.S_IMODE(private.stat().st_mode), 0o700)
        short = Upload(routes['system'] + '/0000000000000000', self.body['system'][:-1], 1024)
        metal_export.receive(short)
        self.assertEqual(short.status, 400)
        self.assertFalse((private / 'system.img').exists())
        for role, route in routes.items():
            self.assertEqual(self.put(route, 0, self.body[role]), 200)

        firmware = {(role, index): (1024, digest) for role, digest in self.digests.items() for index in (0, None)}
        for bad, reason in (((1023, self.digests['system']), 'byte count'),
                            ((1024, '0' * 64), 'digest')):
            with self.subTest(reason=reason), self.assertRaisesRegex(ValueError, reason):
                metal_export.publish(self.directory, self.boot_id, firmware | {('system', None): bad})
            self.assertNotIn('persistentExport', m.read_json(self.directory / 'run.json'))
            self.assertFalse(private.with_suffix('').exists())

        metal_export.publish(self.directory, self.boot_id, firmware)
        rows = m.read_json(self.directory / 'run.json')['persistentExport']['partitions']
        self.assertEqual([row['role'] for row in rows], ['system', 'home'])
        for row in rows:
            role = row['role']
            part = self.parts[role]
            self.assertEqual((row['guid'], row['firstLba'], row['lastLba']),
                             (part['guid'], part['first'], part['last']))
            self.assertEqual((row['hostBytes'], row['firmwareBytes']), (1024, 1024))
            self.assertEqual((row['hostSha256'], row['firmwareSha256']), (self.digests[role],) * 2)
            image = private.with_suffix('') / (role + '.img')
            self.assertEqual(stat.S_IMODE(image.stat().st_mode), 0o600)
            self.assertEqual(image.read_bytes(), self.body[role])


if __name__ == '__main__':
    unittest.main()
