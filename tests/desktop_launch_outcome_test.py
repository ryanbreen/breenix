"""Execute launch-each's post-attach path with a still-running System Check."""
from pathlib import Path
import os
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class LaunchOutcome(unittest.TestCase):
    def test_system_check_completes_before_clean_close(self):
        source = (ROOT / 'userspace/programs/src/guiapps_suite/desktop.rs').read_text()
        start = source.index('            t.attach(title)?;', source.index('pub fn launch_each('))
        start += len('            t.attach(title)?;')
        end = source.index('            t.close_exit()?;', start) + len('            t.close_exit()?;')
        observed = source[start:end]
        harness = r'''
use serde_json::{json, Value};
struct App { complete: bool, failed: bool }
impl App {
 fn expect_text(&mut self, _: &str) -> Result<(), String> { Ok(()) }
 fn type_line(&mut self, _: &str) -> Result<(), String> { Ok(()) }
 fn wait_node(&mut self, key: &str, check: impl Fn(&Value) -> bool) -> Result<(), String> {
  assert_eq!(key, "rows");
  for state in ["pending", "running", "done"] {
   let value = json!({"rows":[{"state":state,"verdict":
    if state != "done" { "" } else if self.failed { "exit 1" } else { "pass" }}]});
   if check(&value) {
    assert_eq!(state, "done", "accepted an unfinished check");
    self.complete = true; return Ok(());
   }
  }
  Err("no successful completed outcome".into())
 }
 fn close_exit(&mut self) -> Result<(), String> {
  if !self.complete { Err("active Runner cancelled: raw exit 256".into()) }
  else if self.failed { Err("completed failure: raw exit 256".into()) }
  else { Ok(()) }
 }
}
fn outcome(t: &mut App) -> Result<(), String> {
 let name = "System Check";
 let outcome = "";
 // Preserve the diagnostic operation variable around the production closure.
 let mut operation = "attach";
 std::hint::black_box(operation);
 let result = (|| {
 // PATH
 Ok(())
 })();
 std::hint::black_box(operation);
 result
}
fn main() {
 let mut success = App { complete: false, failed: false };
 outcome(&mut success).expect("launch-each must await the self-check outcome before close");
 let mut failure = App { complete: false, failed: true };
 assert!(outcome(&mut failure).is_err(), "failed self-check was accepted");
}
'''.replace('// PATH', observed)
        # name is also read by the production outer loop's timing record.
        harness = harness.replace(' let name = "System Check";', ' let name = "System Check";\n std::hint::black_box(name);')
        with tempfile.TemporaryDirectory(prefix='desktop-outcome-') as tmp:
            path = Path(tmp)
            (path / 'src').mkdir()
            (path / 'src/main.rs').write_text(harness)
            (path / 'Cargo.toml').write_text('[package]\nname="desktop-outcome"\nversion="0.0.0"\nedition="2021"\n[dependencies]\nserde_json="1.0"\n')
            result = subprocess.run(['taskpolicy', '-b', 'cargo', 'run', '--offline', '--quiet',
                                     '--manifest-path', str(path / 'Cargo.toml')],
                                    env=dict(os.environ, CARGO_BUILD_JOBS='4', RUSTFLAGS='-Dwarnings'),
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == '__main__':
    unittest.main()
