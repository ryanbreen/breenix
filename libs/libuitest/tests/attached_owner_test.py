"""Run App's real selection and exit methods with deterministic transport replies."""
import pathlib
import subprocess
import tempfile
import unittest

APP = pathlib.Path(__file__).resolve().parents[1] / "src/app.rs"


def function(source, signature):
    start = source.index(signature)
    brace = source.index("{", start)
    depth = 1
    end = brace + 1
    while depth:
        depth += (source[end] == "{") - (source[end] == "}")
        end += 1
    return source[start:end]


class AttachedOwner(unittest.TestCase):
    def test_close_first_after_launching_second_records_first_exit(self):
        source = APP.read_text()
        methods = "\n".join(function(source, signature) for signature in [
            "pub fn launch_next(", "pub fn attach(", "pub fn key_exit(",
            "fn input_exit(", "pub fn finish(",
        ])
        verdicts = "\n".join(function(source, "pub fn " + name + "(") for name in [
            "exit_input_verdict", "desktop_exit_input_verdict", "exit_release_verdict", "finish_verdict",
        ])
        # Only the proc-file transport is redirected; all selection, waiting,
        # exit recording and finish logic below comes verbatim from App.
        with tempfile.TemporaryDirectory(prefix="attached-owner-") as directory:
            root = pathlib.Path(directory)
            ledger = root / "ledger"
            ledger.write_text("")
            methods = methods.replace('"/proc/breenix/input-ledger"', 'LEDGER')
            harness = r'''
use std::collections::BTreeSet;
const LEDGER: &str = @LEDGER@;
struct App { keys: Keys, launched: Option<u64>, tracked: Vec<(u64, Option<i32>)>, window: u64 }
struct Keys { live: Live }
impl Keys { fn backend_mut(&mut self) -> &mut Live { &mut self.live } }
struct Live { session: Session, selected: u64 }
struct Session { next: u64, closed: BTreeSet<u64> }
struct Window { id: u64 }
struct Info { owner: u32 }
struct Exit { pid: u64, status: i32 }
struct Event { serial: u64 }
impl Session {
    fn launch_app(&mut self, _: &[&[u8]], _: u32) -> Result<u64, String> { self.next += 1; Ok(self.next) }
    fn wait_window_until(&mut self, title: &str, _: u64) -> Result<Window, String> {
        Ok(Window { id: if title == "first" { 1 } else { 2 } })
    }
    fn wait_exit(&mut self, pid: u64) -> Result<Exit, String> {
        if !self.closed.contains(&pid) { return Err(format!("waited on running app pid={pid}")); }
        Ok(Exit { pid, status: 0 })
    }
    fn teardown(&mut self) -> Result<Vec<Exit>, String> {
        Ok((1..=self.next).map(|pid| Exit { pid, status: if self.closed.contains(&pid) { 0 } else { 9 } }).collect())
    }
}
impl Live {
    fn session(&mut self) -> &mut Session { &mut self.session }
    fn info(&self, window: u64) -> Result<Info, String> { Ok(Info { owner: window as u32 }) }
    fn watch(&mut self, _: u64) -> Result<(), String> { Ok(()) }
    fn activate(&mut self, window: u64) -> Result<(), String> { self.selected = window; Ok(()) }
    fn send_key(&mut self, _: u16, pressed: bool) -> Result<Event, String> {
        if pressed { self.session.closed.insert(self.selected); }
        Ok(Event { serial: self.selected })
    }
    fn send_button(&mut self, code: u16, pressed: bool) -> Result<Event, String> { self.send_key(code, pressed) }
}
enum Wait { AppOutput, InputAck }
struct Watchdog;
impl Watchdog {
    fn new(_: Wait) -> Result<Self, String> { Ok(Self) }
    fn deadline_ns(&self) -> u64 { 10_000_000_000 }
}
fn now() -> Result<u64, String> { Ok(0) }
mod time { pub fn sleep_ms(_: u64) -> Result<(), String> { Ok(()) } }
mod input {
    pub struct Row { pub disposition: &'static str, pub window: u64, pub reason: &'static str }
    pub fn wait_pointer_idle(_: u64) -> Result<(), String> { Ok(()) }
    pub fn ledger_entry(_: &str, serial: u64) -> Result<Option<Row>, String> {
        Ok(Some(Row { disposition: "delivered", window: serial, reason: "none" }))
    }
}
@VERDICTS@
impl App { @METHODS@ }
fn main() {
    let mut app = App { keys: Keys { live: Live { session: Session { next: 0, closed: BTreeSet::new() }, selected: 0 } },
        launched: None, tracked: Vec::new(), window: 0 };
    app.launch_next(&[b"app"], "first").unwrap();
    app.launch_next(&[b"app"], "second").unwrap();
    app.attach("first").unwrap();
    app.key_exit(32, 0).unwrap();
    assert_eq!(app.tracked, vec![(1, Some(0)), (2, None)]);
    assert_eq!(app.keys.live.session.closed, BTreeSet::from([1]));
    app.finish().unwrap();
}
'''
            harness = harness.replace("@LEDGER@", '"' + str(ledger) + '"')
            harness = harness.replace("@VERDICTS@", verdicts).replace("@METHODS@", methods)
            path = root / "main.rs"
            path.write_text(harness)
            binary = root / "check"
            compiled = subprocess.run(["rustc", "--edition=2021", "-Dwarnings", str(path), "-o", str(binary)],
                                      capture_output=True, text=True)
            self.assertEqual(compiled.returncode, 0, compiled.stderr)
            result = subprocess.run([str(binary)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()
