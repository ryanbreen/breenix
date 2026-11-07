"""Startup records belong to the kernel-selected producer, not every metal suite."""
from pathlib import Path
import runpy
import unittest

ROOT=Path(__file__).resolve().parents[1]
API=runpy.run_path(str(ROOT/'scripts/screen-proof-acceptance.py'))

class StartupProducers(unittest.TestCase):
    def test_breengl_does_not_claim_unproduced_bootstrap_records(self):
        def unavailable(boot):
            self.fail('BreenGL did not select native::suite_requested bootstrap probes')
        self.assertEqual(API['startup_categories']({'suite':'breengel'},
                         {'capture_categories':unavailable}),[])

    def test_gpu_and_console_keep_their_complete_existing_inventory(self):
        categories=['graphics','gr-contexts','draw','shaders','recovery','contexts','core','device']
        for suite in ['gpu','console']:
            boot={'suite':suite};seen=[]
            def supplied(value):
                seen.append(value)
                return categories
            self.assertEqual(API['startup_categories'](boot,{'capture_categories':supplied}),categories)
            self.assertEqual(seen,[boot])

    def test_unknown_producer_is_refused(self):
        with self.assertRaises(ValueError):
            API['startup_categories']({'suite':'other'}, {})

if __name__=='__main__':unittest.main()
