//! Device oracle for cowfs-format section 3's writer rules.
use crate::block::{BlockDevice, BlockError};
use alloc::vec::Vec;

pub const SECTOR_SIZE: usize = 512;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    Write {
        sector: u64,
        bytes: [u8; SECTOR_SIZE],
    },
    Flush,
}

pub struct RecordingDevice;
impl RecordingDevice {
    pub fn new(_image: Vec<u8>) -> Result<Self, BlockError> {
        unimplemented!("recording device")
    }
    pub fn events(&self) -> Vec<Event> {
        unimplemented!("recording device")
    }
    pub fn durable_image(&self) -> Vec<u8> {
        unimplemented!("recording device")
    }
    pub fn durable_generations(&self) -> Vec<u64> {
        unimplemented!("recording device")
    }
}
impl BlockDevice for RecordingDevice {
    fn read_block(&self, _block: u64, _buf: &mut [u8]) -> Result<(), BlockError> {
        unimplemented!("recording device")
    }
    fn write_block(&self, _block: u64, _buf: &[u8]) -> Result<(), BlockError> {
        unimplemented!("recording device")
    }
    fn block_size(&self) -> usize {
        SECTOR_SIZE
    }
    fn num_blocks(&self) -> u64 {
        unimplemented!("recording device")
    }
    fn flush(&self) -> Result<(), BlockError> {
        unimplemented!("recording device")
    }
}

#[cfg(test)]
#[path = "recording_tests.rs"]
mod tests;
