"""Exercise the kernel's compositor event validation for click counts."""
from pathlib import Path
import subprocess
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class WindowClicks(unittest.TestCase):
    def test_clicks_are_only_valid_on_pointer_buttons(self):
        source = (ROOT / 'kernel/src/window/events.rs').read_text()
        validator = item(source, 'pub fn writable(')
        fixture = '''
mod event_kind {
    pub const KEY_PRESS:u16=1; pub const POINTER_BUTTON:u16=5;
    pub const RESIZED:u16=10; pub const RESIZE_FAILED:u16=11;
    pub const FONT_CHANGED:u16=12; pub const COMPOSITOR_CHANGED:u16=13;
    pub const RESIZE_REQUESTED:u16=15; pub const POINTER_ENTER:u16=16;
    pub const POINTER_LEAVE:u16=17; pub const POINTER_CANCEL:u16=18;
}
mod limits { pub const MAX_DIMENSION:u32=4096; pub const EVENT_TEXT:usize=240; }
pub struct WindowEvent { kind:u16, width:u32, height:u32, text_len:u32, clicks:u32 }
'''
        main = '''
fn main() {
    use event_kind::*;
    let mut e = WindowEvent { kind: POINTER_BUTTON, width: 1, height: 1, text_len: 0, clicks: 2 };
    assert!(writable(&e), "button clicks must be accepted");
    for kind in [KEY_PRESS, RESIZE_REQUESTED, POINTER_ENTER, POINTER_LEAVE, POINTER_CANCEL] {
        e.kind = kind;
        assert!(!writable(&e), "nonzero clicks accepted for kind {}", kind);
    }
}
'''
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'check.rs').write_text(fixture + validator + main)
            subprocess.run(['rustc', str(path / 'check.rs'), '-o', str(path / 'check')], check=True)
            subprocess.run([str(path / 'check')], check=True)


if __name__ == '__main__':
    unittest.main()
