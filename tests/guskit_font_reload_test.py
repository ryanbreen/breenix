"""Run Gus Kit's reload block against the production font-slot ownership methods."""
from pathlib import Path
import subprocess
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class FontReload(unittest.TestCase):
    def test_mono_change_retains_display_and_display_change_replaces_it(self):
        app = (ROOT / 'userspace/programs/src/guskit.rs').read_text()
        watcher = (ROOT / 'libs/breengel/src/font.rs').read_text()
        reload = item(app, '        if reload_font {')
        methods = item(watcher, '    pub fn take_display_font(') + item(watcher, '    pub fn put_display_font(')
        harness = r'''
type CachedFont = u32;
struct Window { display_font: Option<CachedFont> }
impl Window {
    // METHODS
    fn display_size(&self) -> f32 { 17.0 }
}
fn main() {
    let mut win = Window { display_font: None };
    win.put_display_font(Some(1));
    let mut font = win.take_display_font();
    let mut font_size = 0.0;
    assert_eq!(font, Some(1));
    for (replacement, expected) in [(None, 1), (Some(2), 2), (None, 2)] {
        if replacement.is_some() { win.display_font = replacement; }
        let reload_font = true;
        // RELOAD
        assert_eq!(font, Some(expected));
        assert_eq!(font_size, 17.0);
    }
}
'''.replace('// METHODS', methods).replace('// RELOAD', reload)
        with tempfile.TemporaryDirectory(prefix='guskit-font-') as tmp:
            path = Path(tmp)
            (path / 'check.rs').write_text(harness)
            subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Dwarnings',
                            str(path / 'check.rs'), '-o', str(path / 'check')], check=True)
            subprocess.run([str(path / 'check')], check=True)


if __name__ == '__main__':
    unittest.main()
