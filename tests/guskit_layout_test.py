"""Run Gus Kit's production drawing, semantic publication and pointer mapping on the host.

GUSKIT_LAYOUT_BASELINE selects an older revision for the same behavioral check.
Only GPU/window I/O and unrelated document/session operations are replaced.
"""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


def source(path):
    rev = os.environ.get('GUSKIT_LAYOUT_BASELINE')
    if rev:
        return subprocess.check_output(['git', 'show', f'{rev}:{path}'], cwd=ROOT, text=True)
    return (ROOT / path).read_text()


class GusKitLayout(unittest.TestCase):
    def test_scaled_drawing_bounds_and_tool_centres(self):
        app = source('userspace/programs/src/guskit.rs')
        code = (ROOT / 'tests/fixtures/guskit_layout.rs').read_text()
        layout = source('userspace/programs/src/guskit/layout.rs') if 'mod layout;' in app else ''
        code = code.replace('// LAYOUT_MODULE', 'mod layout {\n' + layout + '\n}\nuse layout::Layout;' if layout else '')
        code = code.replace('// APP_LAYOUT_FIELD', 'layout: Layout, damage: Option<Clip>,' if layout else '')
        code = code.replace('// APP_LAYOUT_INIT', 'layout: Layout::new(scale), damage: None,' if layout else '')
        code = code.replace('// APP_METHODS', '\n'.join(item(app, f'    fn {name}(') for name in (['point', 'press', 'stroke_bounds'] if 'fn stroke_bounds(' in app else ['point', 'press'])))
        code = code.replace('// PRODUCTION', '\n'.join(item(app, f'fn {name}(') for name in ['draw_label', 'render', 'publish']))
        code = code.replace('// NODE_MODULE', source('userspace/programs/src/guskit/node.rs'))
        code = code.replace('// CONSTANTS', '\n'.join(line for line in app.splitlines() if line.startswith(('const TOP:', 'const NAMES:', 'const LABELS:'))))
        with tempfile.TemporaryDirectory(prefix='guskit-layout-') as tmp:
            path = Path(tmp)
            (path / 'src').mkdir()
            (path / 'src/lib.rs').write_text(code)
            (path / 'Cargo.toml').write_text(f'''[package]
name = "guskit-layout-check"
version = "0.0.0"
edition = "2021"
[workspace]
[dependencies]
libbui = {{ path = "{ROOT / 'libs/libbui'}" }}
libgfx = {{ path = "{ROOT / 'libs/libgfx'}" }}
serde_json = "=1.0.154"
''')
            env = dict(os.environ, CARGO_BUILD_JOBS='4',
                       CARGO_TARGET_DIR=str(ROOT / 'tools/ui-host-tests/target'), RUSTFLAGS='-Dwarnings')
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            result = subprocess.run(priority + ['cargo', 'test', '--offline', '--manifest-path', str(path / 'Cargo.toml'), '--lib', '--', '--nocapture'], env=env)
            self.assertEqual(result.returncode, 0)


if __name__ == '__main__':
    unittest.main()
