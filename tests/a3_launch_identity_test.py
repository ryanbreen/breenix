"""Run Desktop::launch's baseline and acknowledgment against readback records."""
from pathlib import Path
import json
import tempfile
import unittest

from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class LaunchIdentity(unittest.TestCase):
    def test_new_composite_with_same_native_context_and_bounded_failure(self):
        desktop = (ROOT / 'userspace/programs/src/wm_suite/desktop.rs').read_text()
        window = (ROOT / 'libs/libbreenix/src/window.rs').read_text()
        launch = item(desktop, 'pub fn launch(')
        baseline = launch[launch.index('        let previous_'):launch.index('        graphics::take_over_display()')]
        start = launch.index('            Ok(observation.read().is_ok_and(')
        ack = launch[start:launch.index('\n        });', start)]
        screen = item(desktop, 'impl Screen {')
        observation = item(desktop, 'impl WaitObservation {')
        fixture = (ROOT / 'tests/fixtures/a3_launch_identity.rs').read_text()
        for token, body in {
            '@BASELINE@': baseline,
            '@ACK@': ack,
            '@SCREEN@': '\n'.join(item(screen, sig) for sig in [
                'pub fn read(', 'fn read_kind(', 'pub fn serial(', 'pub fn pixel(']),
            '@OBSERVATION@': observation,
            '@VALIDATE@': item(window, 'pub enum ObservationKind') + '\n'
                + item(window, 'pub fn validate_scanout_observation('),
            '@QUAD@': item(desktop, 'pub fn quad('),
            '@DIAGNOSTIC@': item(desktop, 'pub fn failure_diagnostic('),
        }.items():
            fixture = fixture.replace(token, body)
        with tempfile.TemporaryDirectory(prefix='launch-identity-') as directory:
            evidence = Path(directory) / 'evidence'
            evidence.touch()
            fixture = fixture.replace('"/dev/screen-evidence"', json.dumps(str(evidence)))
            run_rust(self, fixture.replace('@EVIDENCE@', json.dumps(str(evidence))))


if __name__ == '__main__':
    unittest.main()
