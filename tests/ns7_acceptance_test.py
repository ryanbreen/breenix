"""Desktop Gus Kit acceptance uses its selected verdict and measured reclamation."""
import json
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest

import gt710_apps_step_test as apps
from suite_verdict_test import SCORER

ROOT = apps.ROOT


class SelectedDesktopAcceptance(unittest.TestCase):
    setUp = apps.AppsStep.setUp
    uploads = apps.AppsStep.uploads

    def selected(self):
        names = [n for n in apps.NAMES if n.startswith('guskit/')]
        text = apps.serial()
        text = re.sub(r'^SUITE gui-apps CASE (?!guskit/).*\n', '', text, flags=re.M)
        text = re.sub(r'START cases=\d+', f'START cases={len(names)} only=guskit', text)
        text = re.sub(r'CASE (guskit/\S+) FAIL ms=1 msg=pending', r'CASE \1 PASS ms=1', text)
        text = re.sub(r'DONE passed=\d+ failed=\d+ skipped=0 total=\d+',
                      f'DONE passed={len(names)} failed=0 skipped=0 total={len(names)} only=guskit', text)
        requests = ('SUITE gui-apps SMBSTOP guskit/smb-share-gone seq=1\n'
                    'SUITE gui-apps SMBSTART guskit/smb-share-gone seq=2\n')
        text = text.replace('SUITE gui-apps CASE guskit/smb-share-gone',
                            requests + 'SUITE gui-apps CASE guskit/smb-share-gone')
        self.boot.update(onlyCase='guskit', watch=True, evidenceEligible=False)
        (self.path / 'run.json').write_text(json.dumps(self.boot))
        (self.path / 'exit-status').write_text('0')
        events = (self.path / 'events.jsonl').read_text().replace('"status": 1', '"status": 0')
        (self.path / 'events.jsonl').write_text(events)
        (self.path / 'serial.txt').write_text(text)
        shutil.copyfile(ROOT / 'docs/gui/gt710-step-23-5.json',
                        self.definition / 'docs/gui/gt710-step-23-5.json')
        return text, names

    def test_stop_start_and_selected_memory_are_checked(self):
        text, _ = self.selected()
        def score(wire):
            (self.path / 'serial.txt').write_text(wire)
            return SCORER.verdict(apps.MANIFEST, self.path / 'serial.txt',
                                  [self.path / 'serial.txt'], only='guskit')
        ok, why = score(text)
        self.assertTrue(ok, why)
        for bad in (text.replace('seq=1', 'seq=-1'),
                    text.replace('SMBSTOP guskit/smb-share-gone', 'SMBSTOP guskit/pencil'),
                    text.replace('reclaimed=PASS', 'reclaimed=FAIL'),
                    text.replace('SUITE gui-apps CASE guskit/pencil PASS ms=1\n', '')):
            self.assertFalse(score(bad)[0])

    def test_step_five_scores_selected_capture_without_full_suite_eligibility(self):
        text, names = self.selected()
        self.assertEqual(apps.API['record_apps_capture'](self.path, apps.SHA, 5), 0)
        report = json.loads((self.path / 'gt710-apps-step.json').read_text())
        self.assertEqual([r['id'] for r in report['selectedCases']], names)
        self.assertEqual(report['scope'], 'apps-selected-run')
        # Full-suite resource admission stays closed to this selected capture.
        with self.assertRaises(ValueError):
            apps.API['resources_identity'](self.boot, 'gui-apps', apps.SHA)
        for bad in (text.replace('reclaimed=PASS', 'reclaimed=FAIL'),
                    text.replace('SUITE gui-apps CASE guskit/pencil PASS ms=1\n', '')):
            (self.path / 'serial.txt').write_text(bad)
            self.assertEqual(apps.API['record_apps_capture'](self.path, apps.SHA, 5), 1)
        (self.path / 'serial.txt').write_text(text)
        (self.path / 'native-final-upload.json').unlink()
        self.assertEqual(apps.API['record_apps_capture'](self.path, apps.SHA, 5), 1)

    def test_gui_subreaper_records_memory_for_selected_children(self):
        source = (ROOT / 'libs/libbreenix/src/suite.rs').read_text()
        expression = source.split('let recording = ', 1)[1].split(';', 1)[0]
        rust = r'''
use std::sync::atomic::{AtomicI32, Ordering};
static PID: AtomicI32 = AtomicI32::new(1);
mod process {
    pub struct Pid(i32);
    impl Pid { pub fn raw(self) -> i32 { self.0 } }
    pub fn getpid() -> Result<Pid, ()> { Ok(Pid(super::PID.load(super::Ordering::Relaxed))) }
}
fn records(only: Option<&str>, scoped: bool, pid: i32) -> bool {
    PID.store(pid, Ordering::Relaxed);
    let recording = EXPRESSION;
    let _ = scoped;
    recording
}
#[test] fn ownership_boundary() {
    assert!(records(Some("guskit"), true, 9));
    assert!(records(None, true, 9));
    assert!(records(None, false, 1));
    assert!(!records(Some("proof"), false, 1));
    assert!(!records(None, false, 9));
}
'''.replace('EXPRESSION', expression)
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            (p / 'memory.rs').write_text(rust)
            subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Dwarnings', '--test',
                            str(p / 'memory.rs'), '-o', str(p / 'memory')], check=True)
            subprocess.run([str(p / 'memory')], check=True)


if __name__ == '__main__':
    unittest.main()
