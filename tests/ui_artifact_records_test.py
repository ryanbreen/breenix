"""Saved FRAME paths extend the grammar without relaxing its case binding."""
import importlib.util
from pathlib import Path
import unittest
ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('artifact_verdict', ROOT/'scripts/suite-verdict.py')
VERDICT = importlib.util.module_from_spec(spec)
spec.loader.exec_module(VERDICT)

class ArtifactRecords(unittest.TestCase):
    def test_saved_pass_and_failure_paths_and_refusals(self):
        base = ('SUITE ui-testing FRAME diagnosis/local pixels def=12345678 ref=formula '
                'source=window:app commit=41 epoch=0 w=2 h=1 sha256=' + 'a'*64)
        path = ' path=/var/uitest/ui-testing/diagnosis/local'
        mismatch = ' result=mismatch compared=2 differing=1 first=1,0 got=#283263 want=#28323c'
        parse = lambda suffix: VERDICT.parse_result(base + suffix, 'ui-testing', 'diagnosis/local', ['pixels'], False)
        self.assertEqual(parse(mismatch + ' artifacts=complete' + path), ('FRAME:pixels', None, 'pixels'))
        self.assertEqual(parse(' result=match artifacts=complete' + path), ('FRAME:pixels', 'pixels', None))
        self.assertEqual(parse(' result=match artifacts=none:io'), ('FRAME:pixels', 'pixels', None))
        for suffix in [
            mismatch + ' artifacts=none:cap' + path,
            mismatch + ' artifacts=complete' + path.replace('/local', '/other'),
            mismatch + ' artifacts=complete' + path.replace('/local', '/../local'),
            mismatch + ' artifacts=complete path=relative',
            ' result=match artifacts=complete',
            ' result=match artifacts=complete' + path + ' extra=1',
        ]:
            with self.subTest(suffix=suffix), self.assertRaises(ValueError):
                parse(suffix)

if __name__ == '__main__': unittest.main()
