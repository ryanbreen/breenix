import hashlib
import json
import shutil
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
EXPORT = ROOT / 'tools/screen-gallery/export.py'

class ExportTest(unittest.TestCase):
    def test_export_and_refuse_digest_mismatch(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            pixels = bytes([3, 2, 1, 255])
            digest = hashlib.sha256(pixels).hexdigest()
            names = b'gpudraw/oneend'
            rec = bytearray(512)
            rec[:4] = b'SCR1'
            struct.pack_into('<IIIIIHHH', rec, 4, 512, 0, 4, 1, 1, 3, 8, 3)
            rec[32:64] = bytes.fromhex(digest)
            struct.pack_into('<I', rec, 64, 4)
            rec[80:80+len(names)] = names
            rec[80+len(names):84+len(names)] = pixels
            disk = root / 'disk'; disk.write_bytes(b'SCRDISK1' + bytes(504) + rec + bytes(512))
            serial = root / 'serial'
            line = f'SUITE gpu SCREEN draw/one end sha256={digest} mode=1x1 binding=2x2\n'
            serial.write_text(line + line.replace('draw/one end', 'draw/two intent'))
            manifests = root / 'manifests'; manifests.mkdir()
            (manifests / 'gpu.json').write_text(json.dumps({'id':'gpu','categories':[{'id':'draw','cases':[
                {'id':'one','title':'First picture'}, {'id':'two','title':'Second picture'}]}]}))
            output = root / 'out'
            repo = root / 'repo'
            script = repo / 'tools/screen-gallery/export.py'
            script.parent.mkdir(parents=True)
            shutil.copyfile(EXPORT, script)
            shutil.move(str(manifests), repo / 'manifests')
            manifests = repo / 'manifests'
            def git(*args):
                return subprocess.check_output(['git', '-C', str(repo), *args], text=True).strip()
            git('init', '-q')
            git('add', '.')
            git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.com', 'commit', '-qm', 'Capture manifest')
            capture_commit = git('rev-parse', 'HEAD')
            cmd = [sys.executable, str(script), str(disk), str(serial), '--boot-id', 'boot-1',
                   '--commit', 'a'*40, '--manifests', str(manifests), '--output', str(output)]
            result = subprocess.run(cmd, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            dirs = list(output.iterdir()); self.assertEqual(len(dirs), 1)
            doc = json.loads((dirs[0] / 'manifest.json').read_text())
            self.assertEqual(set(doc), {'version', 'candidates_id', 'rows'})
            self.assertEqual(doc['candidates_id'], dirs[0].name)
            self.assertEqual(len(doc['rows']), 2)
            for row in doc['rows']:
                self.assertEqual(set(row), {'suite','case','title','contract_section','checkpoint','mode','binding','sha256','boot_id','commit','intent','png'})
                self.assertEqual(row['sha256'], digest)
                self.assertEqual(row['binding'], '2x2')
                self.assertTrue((dirs[0] / row['png']).read_bytes().startswith(b'\x89PNG\r\n\x1a\n'))
            previous_pngs = {row['png']: (dirs[0] / row['png']).read_bytes() for row in doc['rows']}
            manifest = manifests / 'gpu.json'
            source = json.loads(manifest.read_text())
            source['categories'][0]['cases'][0]['expect'] = 'One pixel with RGB (1, 2, 3).'
            manifest.write_text(json.dumps(source))
            git('add', '.')
            git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.com', 'commit', '-qm', 'Describe expected picture')
            expect_commit = git('rev-parse', 'HEAD')
            self.assertNotEqual(expect_commit, capture_commit)
            source['categories'][0]['cases'][0]['expect'] = 'Uncommitted text must not be exported.'
            manifest.write_text(json.dumps(source))
            refreshed = subprocess.run(cmd + ['--refresh'], capture_output=True, text=True)
            self.assertEqual(refreshed.returncode, 0, refreshed.stderr)
            updated = json.loads((dirs[0] / 'manifest.json').read_text())
            self.assertEqual(updated['candidates_id'], doc['candidates_id'])
            self.assertEqual(updated['rows'][0]['expect'], 'One pixel with RGB (1, 2, 3).')
            self.assertEqual(updated['rows'][0]['expectCommit'], expect_commit)
            self.assertNotIn('expect', updated['rows'][1])
            self.assertNotIn('expectCommit', updated['rows'][1])
            projected = [{k: v for k, v in row.items() if k not in ('expect', 'expectCommit')} for row in updated['rows']]
            self.assertEqual(projected, doc['rows'])
            self.assertEqual({name: (dirs[0] / name).read_bytes() for name in previous_pngs}, previous_pngs)
            # A refresh cannot conceal a changed retained picture.
            (dirs[0] / '00000.png').write_bytes(b'changed')
            before = (dirs[0] / 'manifest.json').read_bytes()
            refused = subprocess.run(cmd + ['--refresh'], capture_output=True, text=True)
            self.assertNotEqual(refused.returncode, 0)
            self.assertEqual((dirs[0] / 'manifest.json').read_bytes(), before)
            serial.write_text(line.replace(digest, '0'*64))
            failed = subprocess.run(cmd, capture_output=True, text=True)
            self.assertNotEqual(failed.returncode, 0)
            self.assertEqual(len(list(output.iterdir())), 1)

if __name__ == '__main__':
    unittest.main()
