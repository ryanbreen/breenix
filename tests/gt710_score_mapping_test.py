"""Every GT 710 record reaches the milestone named by its producer."""
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts/metal'))
import metal_score


class GT710ScoreMapping(unittest.TestCase):
    def test_every_manifest_category_uses_its_record_milestone(self):
        manifest = json.loads((ROOT / 'docs/suites/gt710.json').read_text())
        categories = {category['id']: category['cases'] for category in manifest['categories']}
        suite = {'CATEGORIES': categories, 'score': lambda *args: 'passed',
                 'capture_serial': lambda directory: ''}
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            def write(name, value):
                path = directory / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(json.dumps(value))
            write('run.json', {'mode': 'console'})
            write('definition/docs/metal/boot-path.json', {'milestones': []})
            write('definition/tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json', {'stages': []})
            write('definition/docs/boot-path.json', {'fatal': []})
            write('definition/docs/efforts/metal.json', {})
            write('definition/docs/suites/gt710.json', manifest)
            for name, value in [('serial.txt', ''), ('events.jsonl', ''), ('commit', 'abc'),
                                ('ended', ''), ('exit-status', '0'), ('dirty', '0')]:
                (directory / name).write_text(value)
            for category in categories:
                write('gt710-' + category + '.json', {'milestone': 'metal-gt710/' + category})
            with patch.object(metal_score.runpy, 'run_path', return_value=suite):
                results = metal_score.score(directory)['milestones']
            for category in categories:
                with self.subTest(category=category):
                    self.assertTrue(results['metal-gt710/' + category]['passed'])


if __name__ == '__main__':
    unittest.main()
