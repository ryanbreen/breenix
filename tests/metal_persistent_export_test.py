"""Persistent export checks with synthetic partitions and in-memory HTTP requests."""
import hashlib
import io
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
        with patch.dict(metal_persist.SIZES, system=1024, home=1024), \
             patch.object(metal_export.metal_image, 'assemble', side_effect=lambda image, _: image.touch()), \
             patch.object(metal_export.metal_image, 'stub_efi', return_value={}):
            self.parts['system']['last'] = self.parts['system']['first'] + 1
            self.parts['home']['last'] = self.parts['home']['first'] + 1
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
        unknown = routes['system'].rsplit('/', 1)[0] + '/' + self.layout['diskGuid']
        reply = Upload(unknown, self.body['system'], 1024)
        metal_export.receive(reply)
        self.assertEqual(reply.status, 404)
        self.assertFalse((private / 'system.img').exists())
        reply = Upload(routes['system'], self.body['system'], 1024)
        m.update_lease(self.boot_id, state='claimed')
        metal_export.receive(reply)
        self.assertEqual(reply.status, 404)
        self.assertFalse((private / 'system.img').exists())

    def test_firmware_export_has_no_block_write_or_flush_call(self):
        source = (existing.TOOLS / 'efi-stub/src/persistent_export.rs').read_text()
        self.assertRegex(source, r'\bread_blocks\b')
        self.assertNotRegex(source, r'(?i)\b(?:write_?blocks|flush_?blocks)\b\s*\)?\s*\(')

    def test_copy_requires_full_counts_and_matching_digests_before_recording(self):
        import metal_export
        private, routes = self.prepare()
        self.assertEqual(stat.S_IMODE(private.stat().st_mode), 0o700)
        short = Upload(routes['system'], self.body['system'][:-1], 1024)
        metal_export.receive(short)
        self.assertEqual(short.status, 400)
        self.assertFalse((private / 'system.img').exists())
        for role, route in routes.items():
            reply = Upload(route, self.body[role], 1024)
            metal_export.receive(reply)
            self.assertEqual(reply.status, 200)

        firmware = {role: (1024, digest) for role, digest in self.digests.items()}
        for bad, reason in (((1023, self.digests['system']), 'byte count'),
                            ((1024, '0' * 64), 'digest')):
            with self.subTest(reason=reason), self.assertRaisesRegex(ValueError, reason):
                metal_export.publish(self.directory, self.boot_id, dict(firmware, system=bad))
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
