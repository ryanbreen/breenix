//! A device that records its BlockDevice calls, for the run checks of
//! E24 ledger line 368 (#3040). A multi-block call counts as one call.
#![allow(dead_code)] // Each integration test binary uses its own subset.

use block_host_tests::block::{BlockDevice, BlockError};
use block_host_tests::cache::Wait;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

pub const SIZE: usize = 512;
/// The cache's run cap the brief names: one call carries at most 64 KiB.
pub const RUN_CAP_BYTES: usize = 64 * 1024;
pub const RUN_CAP_BLOCKS: usize = RUN_CAP_BYTES / SIZE;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Call {
    Read { start: u64, count: usize },
    Write { start: u64, count: usize },
    Flush,
}

pub struct CountingDevice {
    media: Mutex<Vec<u8>>,
    blocks: u64,
    calls: Mutex<Vec<Call>>,
    fail: Mutex<Option<u64>>,
}

pub fn pattern(block: u64) -> [u8; SIZE] {
    let mut bytes = [0; SIZE];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = (block as usize * 31 + i) as u8;
    }
    bytes
}

impl CountingDevice {
    pub fn new(blocks: u64) -> Self {
        Self {
            media: Mutex::new(vec![0; blocks as usize * SIZE]),
            blocks,
            calls: Mutex::new(Vec::new()),
            fail: Mutex::new(None),
        }
    }

    /// Fill blocks 0..blocks with pattern(block), without recording a call.
    pub fn patterned(blocks: u64) -> Self {
        let device = Self::new(blocks);
        {
            let mut media = device.media.lock().unwrap();
            for block in 0..blocks {
                media[block as usize * SIZE..][..SIZE].copy_from_slice(&pattern(block));
            }
        }
        device
    }

    pub fn media(&self, block: u64) -> Vec<u8> {
        self.media.lock().unwrap()[block as usize * SIZE..][..SIZE].to_vec()
    }

    /// A call whose range covers `block` fails before transferring bytes.
    pub fn fail_on(&self, block: Option<u64>) {
        *self.fail.lock().unwrap() = block;
    }

    pub fn take_calls(&self) -> Vec<Call> {
        std::mem::take(&mut *self.calls.lock().unwrap())
    }

    fn transfer(&self, call: Call, start: u64, count: usize) -> Result<(), BlockError> {
        self.calls.lock().unwrap().push(call);
        if start.checked_add(count as u64).is_none_or(|end| end > self.blocks) {
            return Err(BlockError::OutOfBounds);
        }
        match *self.fail.lock().unwrap() {
            Some(bad) if (start..start + count as u64).contains(&bad) => Err(BlockError::IoError),
            _ => Ok(()),
        }
    }
}

impl BlockDevice for CountingDevice {
    fn read_block(&self, block: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        self.transfer(Call::Read { start: block, count: 1 }, block, 1)?;
        buf[..SIZE].copy_from_slice(&self.media(block));
        Ok(())
    }

    fn read_blocks(&self, start: u64, count: usize, buf: &mut [u8]) -> Result<(), BlockError> {
        self.transfer(Call::Read { start, count }, start, count)?;
        let media = self.media.lock().unwrap();
        buf[..count * SIZE].copy_from_slice(&media[start as usize * SIZE..][..count * SIZE]);
        Ok(())
    }

    fn write_block(&self, block: u64, buf: &[u8]) -> Result<(), BlockError> {
        self.transfer(Call::Write { start: block, count: 1 }, block, 1)?;
        self.media.lock().unwrap()[block as usize * SIZE..][..SIZE].copy_from_slice(&buf[..SIZE]);
        Ok(())
    }

    fn write_blocks(&self, start: u64, count: usize, buf: &[u8]) -> Result<(), BlockError> {
        self.transfer(Call::Write { start, count }, start, count)?;
        self.media.lock().unwrap()[start as usize * SIZE..][..count * SIZE]
            .copy_from_slice(&buf[..count * SIZE]);
        Ok(())
    }

    fn block_size(&self) -> usize {
        SIZE
    }

    fn num_blocks(&self) -> u64 {
        self.blocks
    }

    fn flush(&self) -> Result<(), BlockError> {
        self.calls.lock().unwrap().push(Call::Flush);
        Ok(())
    }
}

/// These checks run on one thread, so admission is uncontended; a wait yields.
#[derive(Default)]
pub struct YieldWait(AtomicU64);

impl Wait for YieldWait {
    fn generation(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }

    fn wait(&self, observed: u64) {
        while self.0.load(Ordering::SeqCst) == observed {
            std::thread::yield_now();
        }
    }

    fn wake_all(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
