use crate::block::{BlockDevice, BlockError};
use crate::buffer_cache::Wait;
use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

pub const SIZE: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Read(u64),
    Write(u64),
    Flush,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Issue(Op),
    Complete(Op),
    Waiting(String),
    Returned(Op),
}

#[derive(Default)]
struct State {
    events: Vec<Event>,
    blocks: BTreeMap<u64, Vec<u8>>,
    hold_write: bool,
    hold_flush: bool,
}

#[derive(Default)]
pub struct RecordingDevice {
    state: Mutex<State>,
    changed: Condvar,
}

impl RecordingDevice {
    pub fn record(&self, event: Event) {
        self.state.lock().unwrap().events.push(event);
        self.changed.notify_all();
    }

    pub fn events(&self) -> Vec<Event> {
        self.state.lock().unwrap().events.clone()
    }

    pub fn hold(&self, write: bool, flush: bool) {
        let mut s = self.state.lock().unwrap();
        s.hold_write = write;
        s.hold_flush = flush;
        self.changed.notify_all();
    }

    pub fn until(&self, ready: impl Fn(&[Event]) -> bool) {
        let s = self.state.lock().unwrap();
        let (s, timeout) = self
            .changed
            .wait_timeout_while(s, Duration::from_secs(10), |s| !ready(&s.events))
            .unwrap();
        assert!(
            !timeout.timed_out() || ready(&s.events),
            "recording watchdog: {:?}",
            s.events
        );
    }
}

impl BlockDevice for RecordingDevice {
    fn read_block(&self, block: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        let mut s = self.state.lock().unwrap();
        s.events.push(Event::Issue(Op::Read(block)));
        buf[..SIZE].copy_from_slice(
            s.blocks
                .get(&block)
                .map(Vec::as_slice)
                .unwrap_or(&[0; SIZE]),
        );
        s.events.push(Event::Complete(Op::Read(block)));
        self.changed.notify_all();
        Ok(())
    }

    fn write_block(&self, block: u64, buf: &[u8]) -> Result<(), BlockError> {
        let mut s = self.state.lock().unwrap();
        s.events.push(Event::Issue(Op::Write(block)));
        self.changed.notify_all();
        // One block is held so the test can force a flush to drain an issued write.
        while block == 1 && s.hold_write {
            s = self.changed.wait(s).unwrap();
        }
        s.blocks.insert(block, buf[..SIZE].to_vec());
        s.events.push(Event::Complete(Op::Write(block)));
        self.changed.notify_all();
        Ok(())
    }

    fn block_size(&self) -> usize {
        SIZE
    }
    fn num_blocks(&self) -> u64 {
        64
    }

    fn flush(&self) -> Result<(), BlockError> {
        let mut s = self.state.lock().unwrap();
        s.events.push(Event::Issue(Op::Flush));
        self.changed.notify_all();
        while s.hold_flush {
            s = self.changed.wait(s).unwrap();
        }
        s.events.push(Event::Complete(Op::Flush));
        self.changed.notify_all();
        Ok(())
    }
}

pub struct HostWait {
    generation: Mutex<u64>,
    changed: Condvar,
    trace: Arc<RecordingDevice>,
}

impl HostWait {
    pub fn new(trace: Arc<RecordingDevice>) -> Self {
        Self {
            generation: Mutex::new(0),
            changed: Condvar::new(),
            trace,
        }
    }
}

impl Wait for HostWait {
    fn generation(&self) -> u64 {
        *self.generation.lock().unwrap()
    }

    fn wait(&self, observed: u64) {
        let mut generation = self.generation.lock().unwrap();
        self.trace.record(Event::Waiting(
            std::thread::current().name().unwrap().to_owned(),
        ));
        while *generation == observed {
            generation = self.changed.wait(generation).unwrap();
        }
    }

    fn wake_all(&self) {
        *self.generation.lock().unwrap() += 1;
        self.changed.notify_all();
    }
}
