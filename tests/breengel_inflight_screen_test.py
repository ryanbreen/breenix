"""Force both composite2 orders through the production userspace compositor.

GPU calls and snapshot replies are host fixtures, not device execution or an
admission-race test. The first snapshot has full damage; the next has none, as
scene::finish acknowledges the one committed generation. Check the actual BGL
streams, retained binding and pre-helper-drop end against the gallery prose.
BREENIX_INFLIGHT_SOURCE_REV reads an older production tree without checking it out.
"""
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

from PIL import Image
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts/screens'))
spec = importlib.util.spec_from_file_location('verify_expect', ROOT / 'scripts/screens/verify_expect.py')
verify = importlib.util.module_from_spec(spec)
spec.loader.exec_module(verify)


def source(path):
    revision = os.environ.get('BREENIX_INFLIGHT_SOURCE_REV')
    if revision:
        return subprocess.run(['git', 'show', f'{revision}:{path}'], cwd=ROOT,
                              check=True, capture_output=True, text=True).stdout
    return (ROOT / path).read_text()


class InflightScreen(unittest.TestCase):
    def test_end_matches_expect_in_both_caller_orders(self):
        compositor = source('libs/libbreenix/src/compositor.rs').replace('//!', '//')
        kit = source('userspace/programs/src/suite_breengel/window_cases/kit.rs')
        screen = source('libs/libbreenix/src/suite_screen.rs')
        case = item(source('userspace/programs/src/suite_breengel/window_cases/composition.rs'),
                    'fn composition_inflight_bound(')
        geometry = re.search(r'Placed::new\((\d+), (\d+), (\d+), (\d+), (\d+), (\d+)\)', case)
        self.assertIsNotNone(geometry)
        x, y, width, height, z, _seed = map(int, geometry.groups())
        self.assertEqual((x, y, z), (0, 0, 0))
        seed = int(re.search(r'w.frame\((\d+)\)', case)[1])
        # Run the production helper-drop preservation and finish fallback selection.
        end = item(screen, 'pub(super) struct EndCheckpoint') + '\n' + item(screen, 'impl EndCheckpoint')
        selection = item(screen, 'match snapshot()').replace('END.lock().unwrap()', 'end')
        fixture = (ROOT / 'tests/fixtures/breengel_inflight_screen.rs').read_text()
        fixture = fixture.replace('// PATTERN', item(kit, 'pub(crate) fn pattern('))
        fixture = fixture.replace('// END_CHECKPOINT', end)
        fixture = fixture.replace('// END_SELECTION', selection)
        fixture = fixture.replace('WINDOW_WIDTH', str(width)).replace('WINDOW_HEIGHT', str(height))
        fixture = fixture.replace('WINDOW_SEED', str(seed))
        with tempfile.TemporaryDirectory(prefix='breengel-inflight-') as directory:
            path = Path(directory)
            (path / 'compositor.rs').write_text(compositor)
            (path / 'main.rs').write_text(fixture)
            (path / 'Cargo.toml').write_text(
                '[package]\nname="inflight-screen-host"\nversion="0.0.0"\nedition="2021"\n'
                '[workspace]\n[dependencies]\n'
                f'gpu-abi = {{path="{ROOT / "libs/gpu-abi"}", features=["alloc"]}}\n'
                f'breengel-abi = {{path="{ROOT / "libs/breengel-abi"}"}}\n'
                '[[bin]]\nname="check"\npath="main.rs"\n')
            command = (['taskpolicy', '-b'] if sys.platform == 'darwin' else []) + [
                'cargo', '-Z', 'bindeps', 'run', '--release', '--manifest-path', str(path / 'Cargo.toml'), '--', str(path)]
            env = dict(os.environ, CARGO_BUILD_JOBS='4', RUSTFLAGS='-Dwarnings')
            run = subprocess.run(command, cwd=path, env=env, capture_output=True, text=True, timeout=120)
            self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
            print(run.stdout, end='')
            manifest = json.loads(source('docs/suites/breengel.json'))
            entry = next(c for cat in manifest['categories'] if cat['id'] == 'composition'
                         for c in cat['cases'] if c['id'] == 'inflight-bound')
            for order in ('mine-first', 'theirs-first'):
                with self.subTest(order=order):
                    pixels = (path / f'{order}.bgra').read_bytes()
                    image = Image.frombytes('RGBA', (2560, 1600), pixels, 'raw', 'BGRA').convert('RGB')
                    result = verify.verify_picture(image, entry.get('expectEnd', entry['expect']),
                                                   'end', case='composition/inflight-bound', suite='breengel')
                    print(f'{order}: {result["verdict"]}; {result["failed_claims"]}')
                    self.assertEqual(result['failed_claims'], [], 'Expect contradicts the production end frame')
                    self.assertEqual(result['verdict'], 'pass', result)


if __name__ == '__main__':
    unittest.main()
