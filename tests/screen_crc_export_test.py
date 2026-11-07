"""Exercise uploaded CRC records through export and native approval."""
import hashlib
import io
from unittest.mock import patch
import json
import struct
import subprocess
import sys
from pathlib import Path
from screens_approve_test import ApproveTest
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts/metal"))
import metal_lib as m
import gt710_upload as upload

ROOT = Path(__file__).resolve().parents[1]

class ExportTest(ApproveTest):
    def test_export_and_refusals(self):
        self.run_import()
        boot, evidence = self.native_fixture()
        run = json.loads((boot / 'run.json').read_text())
        run['screenProof'] = True
        (boot / 'run.json').write_text(json.dumps(run))
        (boot / 'commit').write_text(run['commit'] + '\n')
        serial = (boot / 'serial.txt').read_text().replace('START cases=1', 'START cases=1 screen_proof=on')
        capture = evidence['checkpoints'][0]
        names = [capture[k].encode() for k in ('suite', 'case', 'checkpoint')]
        label = 'crc-gpu-' + hashlib.sha256(names[1] + b'\0' + names[2]).hexdigest()[:16]
        def record(samples=5, moved=False):
            header = bytearray(116)
            header[:8] = b'SCRCRC01'
            total = 116 + sum(map(len, names)) + 16 + 12*samples
            struct.pack_into('<III6H', header, 8, 1, total, 6, *map(len, names), 2, samples, 1)
            struct.pack_into('<QQQQ', header, 32, 7, 9, 7, 10 if moved else 9)
            struct.pack_into('<13I', header, 64, 85000, 0, 1, 2, 1, 1280, 1024, 0, 112, 1280, 800, 0, 0)
            return bytes(header) + b''.join(names) + struct.pack('<4I', 0x404, 0, 0x4d0, 0) + struct.pack('<3I', 1, 0x12345678, 0xabcdef12)*samples
        def write(raw, digest=None):
            artifact = boot / ('diagnostic-' + label + '.bin')
            receipt = boot / ('native-diagnostic-' + label + '.json')
            artifact.unlink(missing_ok=True); receipt.unlink(missing_ok=True)
            route = '/gt710-upload/' + 'a'*64
            class Handler:
                path = route + '/diagnostics/' + label
                headers = {'Content-Length': str(len(raw)), 'X-Content-SHA256': hashlib.sha256(raw).hexdigest()}
                rfile = io.BytesIO(raw)
                connection = None
                def __init__(self): self.connection = self; self.code = None
                def settimeout(self, value): pass
                def send_error(self, code, *args): self.code = code
                def send_response(self, code): self.code = code
                def send_header(self, *args): pass
                def end_headers(self): pass
            state = self.root/'state'; state.mkdir(exist_ok=True)
            m.write_json(state/'gt710-upload.json', dict(id=run['id'], route=route, suite='gpu', complete=True))
            handler = Handler()
            with patch.object(m, 'STATE', state), patch.object(m, 'boot_dir', return_value=boot), patch.object(m, 'read_lease', return_value=dict(id=run['id'], state='powered')):
                upload.receive(handler)
            self.assertEqual(handler.code, 200)

            anchor = f'SCREEN_CRC_UPLOAD gpu draw/picture-0 end label={label} bytes={len(raw)} sha256={digest or hashlib.sha256(raw).hexdigest()} result=ok\n'
            (boot / 'serial.txt').write_text(serial + anchor)
        def export(ok):
            result = subprocess.run([sys.executable, str(ROOT/'scripts/metal/screen-crc-export.py'), str(boot)], capture_output=True, text=True)
            self.assertEqual(result.returncode == 0, ok, result.stderr)
        write(record()); export(True)
        self.run_import(native=boot)
        write(record(), '0'*64); export(False)
        write(record(4)); export(False)
        write(record(moved=True)); export(False)
        write(record()); export(True)
        (boot/'commit').write_text('c'*40); export(False)
        (boot/'commit').write_text(run['commit'])
        receipt = boot/('native-diagnostic-'+label+'.json')
        saved = receipt.read_text(); bad_receipt = json.loads(saved); bad_receipt['id'] = 'wrong'
        receipt.write_text(json.dumps(bad_receipt)); export(False); receipt.write_text(saved)
        sidecar = json.loads((boot/'screen-crc.json').read_text())
        for field in ('run', 'commit'):
            bad = dict(sidecar); bad[field] = 'wrong'
            (boot/'screen-crc.json').write_text(json.dumps(bad))
            self.run_import(native=boot, good=False)
