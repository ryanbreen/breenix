#!/usr/bin/env python3
"""Exercise the production functions with host synchronization and device doubles."""
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def function(source, name):
    start = source.index("fn " + name + "(")
    opened = source.index("{", start)
    depth = 1
    end = opened + 1
    while depth:
        depth += (source[end] == "{") - (source[end] == "}")
        end += 1
    return source[start:end]


class Regression(unittest.TestCase):
    def test_race(self):
        source = (ROOT / "kernel/src/drivers/ahci/mod.rs").read_text()
        functions = "\n".join(function(source, name) for name in ("detect_completed_slots", "poll_ports"))
        constants = "\n".join(re.findall(r"const PORT_IRQ_[\s\S]*?;", source))
        harness = r'''

use std::sync::{Mutex, MutexGuard, atomic::{AtomicU32, AtomicBool, Ordering}, mpsc};
use std::time::Duration;
const MAX_PORTS: usize = 1;
const AHCI_TRACKED_SLOT_MASK:u32 = 1;
const PORT_IS:u32 = 0;
const PORT_CI:u32 = 1;
struct Lock(Mutex<()>);
impl Lock { fn try_lock(&self)->Option<MutexGuard<'_, ()>> { self.0.try_lock().ok() } }
static PORT_POLL:[Lock;1] = [Lock(Mutex::new(()))];
static AHCI_ABAR:AtomicU32 = AtomicU32::new(1);
static AHCI_PORTS_IMPLEMENTED:AtomicU32 = AtomicU32::new(1);
static PORT_ACTIVE_MASK:[AtomicU32;1] = [AtomicU32::new(1)];
static PORT_ACTIVE_CMD_NUM:[AtomicU32;1] = [AtomicU32::new(1)];
static CI:AtomicU32 = AtomicU32::new(0);
static IS:AtomicU32 = AtomicU32::new(1);
static COMPLETED:AtomicU32 = AtomicU32::new(0);
static PAUSE:AtomicBool = AtomicBool::new(true);
static SAMPLED:Mutex<Option<mpsc::Sender<()>>> = Mutex::new(None);
static RELEASE:Mutex<Option<mpsc::Receiver<()>>> = Mutex::new(None);
struct Completion;
static AHCI_COMPLETIONS:[[Completion;1];1] = [[Completion]];
impl Completion { fn complete(&self, cmd:u32) {
    COMPLETED.store(cmd,Ordering::Release);
    // The awakened waiter immediately issues command N+1 on the same slot.
    CI.store(1,Ordering::Release);
    IS.store(0,Ordering::Release);
    PORT_ACTIVE_CMD_NUM[0].store(cmd+1,Ordering::Release);
    PORT_ACTIVE_MASK[0].store(1,Ordering::Release);
} }
fn port_read(_:u32, _:usize, reg:u32)->u32 {
    let value = if reg == PORT_IS { IS.load(Ordering::Acquire) } else { CI.load(Ordering::Acquire) };
    if reg == PORT_CI && std::thread::current().name() == Some("delayed") && PAUSE.swap(false,Ordering::AcqRel) {
        SAMPLED.lock().unwrap().take().unwrap().send(()).unwrap();
        RELEASE.lock().unwrap().take().unwrap().recv_timeout(Duration::from_secs(5)).unwrap();
    }
    value
}
fn ack_port_interrupt(_:u32, _:usize, _:u32) { IS.store(0,Ordering::Release); }
mod task { pub mod scheduler { pub fn yield_current() {} } }
fn main() {
    let (sampled_tx,sampled_rx) = mpsc::channel();
    let (release_tx,release_rx) = mpsc::channel();
    *SAMPLED.lock().unwrap() = Some(sampled_tx);
    *RELEASE.lock().unwrap() = Some(release_rx);
    let delayed = std::thread::Builder::new().name("delayed".into()).spawn(poll_ports).unwrap();
    sampled_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    // Another CPU must not retire the slot while the delayed CPU holds its sample.
    assert!(poll_ports());
    assert_eq!(COMPLETED.load(Ordering::Acquire),0);
    release_tx.send(()).unwrap();
    delayed.join().unwrap();
    assert_eq!(COMPLETED.load(Ordering::Acquire),1);
    // A later pass samples N+1's own busy status, rather than N's completion.
    assert!(poll_ports());
    assert_eq!(PORT_ACTIVE_CMD_NUM[0].load(Ordering::Acquire),2);
    assert_eq!(PORT_ACTIVE_MASK[0].load(Ordering::Acquire),1);
    assert_eq!(COMPLETED.load(Ordering::Acquire),1);
}
'''
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / "race.rs").write_text(constants + "\n" + functions + "\n" + harness)
            subprocess.run(["rustc", "--edition=2021", "-A", "dead_code", str(path / "race.rs"), "-o", str(path / "race")], check=True)
            subprocess.run([str(path / "race")], check=True, timeout=20)


if __name__ == "__main__":
    unittest.main()
