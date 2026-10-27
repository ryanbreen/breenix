"""Completion records must route to the declared metal milestone."""
import json
from pathlib import Path
import shutil
import sys
import tempfile
import unittest
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts/metal'))
import metal_score


class CompletionMilestone(unittest.TestCase):
    def test_completion_record_reaches_its_milestone_validator(self):
        with tempfile.TemporaryDirectory() as temporary:
            d = Path(temporary)
            for name in ('docs/metal/boot-path.json', 'docs/boot-path.json', 'docs/suites/gt710.json',
                         'docs/efforts/metal.json', 'scripts/metal/gt710-suite',
                         'tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json'):
                target = d / 'definition' / name
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(ROOT / name, target)
            for name, value in {'run.json': dict(mode='suite', suite='console', disk='root'),
                                'gt710-completion.json': dict(milestone='metal-gt710/completion')}.items():
                (d / name).write_text(json.dumps(value))
            for name, value in {'serial.txt': '', 'events.jsonl': '', 'commit': 'a'*40,
                                'dirty': '0', 'exit-status': '0', 'ended': ''}.items():
                (d / name).write_text(value)
            milestones = metal_score.score(d)['milestones']
            self.assertIn('metal-gt710/completion', milestones)
            result = milestones['metal-gt710/completion']
            self.assertFalse(result['passed'])  # Routing must not certify absent evidence.
            self.assertNotIn('records another milestone', result['reason'])
            self.assertNotIn('gt710/completion', milestones)


if __name__ == '__main__': unittest.main()
