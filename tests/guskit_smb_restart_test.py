import json
import pathlib
import re
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]


class GuskitRestartTest(unittest.TestCase):
    def test_guest_requests_match_restart_helper(self):
        source = (ROOT / 'userspace/programs/src/guiapps_suite/guskit.rs').read_text()
        helper = (ROOT / 'scripts/metal/smb-restart').read_text()
        template = re.search(r'const SMB_REQUEST: &str = "([^"]+)";', source).group(1)
        pattern = re.search(r"GUI_REQUEST = re.compile\(r'([^']+)'\)", helper).group(1)
        for action, seq in [('STOP', 1), ('START', 2)]:
            line = template.replace('{}', action, 1).replace('{}', str(seq), 1)
            self.assertIsNotNone(re.fullmatch(pattern, line))
        self.assertEqual(source.count('SMB_REQUEST.replace('), 1)

    def test_case_is_registered(self):
        manifest = json.loads((ROOT / 'docs/suites/gui-apps.json').read_text())
        step = json.loads((ROOT / 'docs/gui/gt710-step-23-5.json').read_text())
        self.assertIn('"smb-share-gone"', json.dumps(manifest))
        self.assertIn('guskit/smb-share-gone', step['cases'])
        source = (ROOT / 'userspace/programs/src/suite_gui_apps.rs').read_text()
        self.assertIn('case("smb-share-gone"', source)


if __name__ == '__main__':
    unittest.main()
