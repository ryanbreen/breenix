"""Exercise the session-start title update from Gus Kit's dispatch loop."""
from pathlib import Path
import subprocess
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class SessionTitle(unittest.TestCase):
    def test_starting_session_updates_registered_window_title(self):
        source = (ROOT / 'userspace/programs/src/guskit.rs').read_text()
        after_dispatch = source.split('        let count=result.', 1)[1].split('\n', 1)[1]
        update = after_dispatch.split('        if reload_font {', 1)[0]
        title = item(source, 'fn session_title(')
        program = r'''
use std::sync::Mutex;
static TITLES: Mutex<Vec<(u64, Vec<u8>)>> = Mutex::new(Vec::new());
mod libbreenix { pub mod window {
    pub mod abi { pub mod limits { pub const MAX_TITLE: usize = 64; } }
    pub fn set_title(id: u64, title: &[u8]) -> Result<u64, String> {
        crate::TITLES.lock().unwrap().push((id, title.to_vec())); Ok(0)
    }
} }
struct Model { name: String }
struct Core { model: Model }
struct Session { core: Core }
struct App { session: Option<Session> }
struct Window;
impl Window { fn id(&self) -> u64 { 42 } }
// TITLE
fn main() {
    let win = Window;
    for (collaborating, name, previous, expected) in [
        (false, None, "Gus Kit", None),
        (false, Some("Host"), "Gus Kit", Some("Gus Kit (Host)")),
        (true, Some("Host"), "Gus Kit (Host)", None),
        (false, Some("Peer"), "Gus Kit (Host)", Some("Gus Kit (Peer)")),
        (false, Some("Host"), "Gus Kit (Host)", None),
    ] {
        let app = App { session: name.map(|name| Session {
            core: Core { model: Model { name: name.into() } }
        }) };
        let mut title = previous.to_string();
        TITLES.lock().unwrap().clear();
        // UPDATE
        let titles = TITLES.lock().unwrap();
        let wanted = expected.map(|title: &str| vec![(42, title.as_bytes().to_vec())]).unwrap_or_default();
        assert_eq!(*titles, wanted, "session start must update the registered title");
        assert_eq!(title, expected.unwrap_or(previous));
        // An empty update must fail the assertion rather than an unused-code lint.
        assert_eq!(win.id(), 42);
        let _ = (collaborating, app.session.map(|s| s.core.model.name));
        let _setter = libbreenix::window::set_title;
    }
    assert_eq!(session_title(&"x".repeat(64)), format!("Gus Kit ({})", "x".repeat(54)));
    let unicode = session_title(&"\u{1f3a8}".repeat(16));
    assert_eq!(unicode, format!("Gus Kit ({})", "\u{1f3a8}".repeat(13)));
    assert!(unicode.len() <= libbreenix::window::abi::limits::MAX_TITLE);
}
'''.replace('// UPDATE', update).replace('// TITLE', title)
        with tempfile.TemporaryDirectory(prefix='guskit-session-title-') as directory:
            path = Path(directory)
            (path / 'main.rs').write_text(program)
            subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Dwarnings',
                            str(path / 'main.rs'), '-o', str(path / 'check')], check=True)
            result = subprocess.run([str(path / 'check')], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
