#!/usr/bin/env python3
"""Exercise production byte components and getcwd with explicit directory-frame shims."""
from pathlib import Path
import os
import subprocess
import tempfile
ROOT = Path(__file__).resolve().parents[1]
source = (ROOT / "kernel/src/fs/namei.rs").read_text()
def function(name):
    start = source.index("fn " + name + "(")
    # These functions end at a top-level closing brace.
    end = source.index("\n}", start) + 2
    return source[start:end]
production = "\n".join(function(n) for n in ["copy_bytes", "join", "push_components", "is_root", "working_dir_path_bytes"])
harness = r'''
use core::sync::atomic::{AtomicBool, Ordering};
const ENOENT: i32 = 2;
const ENAMETOOLONG: i32 = 36;
const PATH_MAX: usize = 4096;
const NAME_MAX: usize = 255;
static ROOT_CHANGED: AtomicBool = AtomicBool::new(true);
#[derive(Clone)]
struct Frame { name: Vec<u8>, node: u64 }
enum WorkingDir { Root, Frames(Vec<Frame>) }
struct Held(Option<()>);
impl Drop for Held { fn drop(&mut self) { self.0.take(); } }
fn try_current_dirs() -> Result<(WorkingDir, WorkingDir), u64> {
    Ok((WorkingDir::Root, WorkingDir::Frames(vec![Frame { name: b"jail".to_vec(), node: 2 }])))
}
fn frames(dir: &WorkingDir, _: &mut Held) -> Result<Vec<Frame>, u64> {
    match dir { WorkingDir::Root => Ok(vec![]), WorkingDir::Frames(f) => Ok(f.clone()) }
}
#[test]
fn non_utf8_components_and_cwd_remain_exact() {
    let mut pending = vec![];
    push_components(&mut pending, b"/jail/child\xff").unwrap();
    assert_eq!(pending.pop().unwrap(), b"jail");
    let name = pending.pop().unwrap();
    assert_eq!(name, b"child\xff");
    let dir = WorkingDir::Frames(vec![Frame { name: b"jail".to_vec(), node: 2 }, Frame { name, node: 3 }]);
    assert_eq!(working_dir_path_bytes(&dir).unwrap(), b"/child\xff");
}
#[test]
fn cwd_outside_root_returns_enoent() {
    assert_eq!(working_dir_path_bytes(&WorkingDir::Root).unwrap_err(), 2);
    let other = WorkingDir::Frames(vec![Frame { name: b"other".to_vec(), node: 4 }]);
    assert_eq!(working_dir_path_bytes(&other).unwrap_err(), 2);
    // A same-spelled directory with a different inode is also outside.
    let different = WorkingDir::Frames(vec![Frame { name: b"jail".to_vec(), node: 5 }]);
    assert_eq!(working_dir_path_bytes(&different).unwrap_err(), 2);
}
#[test]
fn component_limit_remains_enametoolong() {
    assert_eq!(push_components(&mut vec![], &vec![0xff; 256]).unwrap_err(), 36);
    assert!(push_components(&mut vec![], &vec![0xff; 255]).is_ok());
}
'''
with tempfile.TemporaryDirectory(prefix="breenix-namei-path-") as tmp:
    test = Path(tmp) / "tests.rs"
    test.write_text(harness + production)
    binary = Path(tmp) / "tests"
    env = dict(os.environ, CARGO_BUILD_JOBS="6")
    subprocess.run(["rustc", "--edition=2021", "--test", str(test), "-o", str(binary)], cwd=ROOT, env=env, check=True)
    subprocess.run([str(binary)], check=True)
