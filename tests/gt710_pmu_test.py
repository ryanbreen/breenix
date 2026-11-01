"""C4 checks for the MIT nouveau PMU init sequence and committed source-built images."""
from pathlib import Path
import hashlib
import json
import subprocess
import sys
import tempfile
import unittest
ROOT=Path(__file__).resolve().parents[1]
class Pmu(unittest.TestCase):
    def test_init_sequence(self):
        with tempfile.TemporaryDirectory() as tmp:
            cmd=(['taskpolicy','-b'] if sys.platform=='darwin' else [])+['rustc','--edition=2021','-Dwarnings','--test',str(ROOT/'tests/gt710_pmu_test.rs'),'-o',tmp+'/pmu']
            subprocess.run(cmd,check=True)
            subprocess.run([tmp+'/pmu'],check=True)
    def test_pinned_firmware(self):
        assets=ROOT/'kernel/assets/kepler'
        manifest=json.loads((assets/'manifest.json').read_text())
        lock=json.loads((ROOT/'docs/gui/provenance/kepler-toolchain.sha256.json').read_text())
        fw=next(f for f in manifest['firmware'] if f['name']=='pmu')
        self.assertEqual(fw['entry_source'],'nouveau-pmu/gk208.fuc5')
        self.assertEqual(fw['falcon'],5)
        for part in ('code','data'):
            name=f'pmu.{part}.bin';data=(assets/name).read_bytes()
            self.assertTrue(data and len(data)%256==0)
            self.assertEqual(hashlib.sha256(data).hexdigest(),lock[name])
            self.assertEqual(manifest['files'][name],dict(bytes=len(data),license='MIT',sha256=lock[name]))
if __name__=='__main__': unittest.main()
