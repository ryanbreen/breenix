//! CPU observation of device memory must invalidate before its first read.
//! The transport fence has already completed when the caller reaches this boundary.
//! Host tests: `tests/gpu_observation.rs`.
pub trait Memory {
    fn invalidate(&self, offset: usize, length: usize);
    fn copy(&self, offset: usize, out: &mut [u8]);
}

pub fn read(memory: &impl Memory, offset: usize, out: &mut [u8]) {
    memory.invalidate(offset, out.len());
    memory.copy(offset, out);
}
