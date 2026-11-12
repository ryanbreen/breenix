"""Exercise bterm painting and TTF batching without a device or timing threshold."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class TerminalRender(unittest.TestCase):
    def test_log_viewer_frame_work_and_incremental_pixels(self):
        terminal = (ROOT / 'userspace/programs/src/bterm.rs').read_text()
        font = (ROOT / 'libs/libgfx/src/ttf_font.rs').read_text()
        fixture = (ROOT / 'tests/fixtures/bterm_render_host.rs').read_text()
        font_items = ['pub struct TextRun', 'pub fn draw_runs(', 'fn rasterize_text_runs(', 'fn draw_runs_mode(']
        # The terminal calls its production API. Include a new bulk API when present;
        # old source still executes the original production draw_runs path.
        for marker in ['pub struct CellGlyph', 'pub fn draw_cells(']:
            if marker in font:
                font_items.append(marker)
        code = '\n'.join(item(font, marker) for marker in font_items)
        model = terminal[terminal.index('struct TermEmu {'):terminal.index('// ─── Tab ─')]
        if 'mod paint;' in terminal:
            model = 'mod paint {\n' + (ROOT / 'userspace/programs/src/bterm/paint.rs').read_text() + '\n}\n' + model
        fixture = fixture.replace('SELECTION_PATH', str(ROOT / 'userspace/programs/src/bterm/selection.rs'))
        fixture = fixture.replace('// FONT_CODE', code).replace('// TERMINAL_CODE', model)
        with tempfile.TemporaryDirectory(prefix='bterm-render-') as directory:
            path = Path(directory)
            (path / 'check.rs').write_text(fixture)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            env = dict(os.environ, CARGO_BUILD_JOBS='4')
            subprocess.run(priority + ['rustc', '--edition=2021', '-O', '-Dwarnings',
                '--crate-type=rlib', '--crate-name=libvt', str(ROOT / 'libs/libvt/src/lib.rs'),
                '-o', str(path / 'libvt.rlib')], check=True, env=env)
            subprocess.run(priority + ['rustc', '--edition=2021', '-O', '-Dwarnings',
                '--extern', f'libvt={path / "libvt.rlib"}', str(path / 'check.rs'),
                '-o', str(path / 'check')], check=True, env=env)
            subprocess.run([str(path / 'check')], check=True)


if __name__ == '__main__':
    unittest.main()
