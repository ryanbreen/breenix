//! CPU observation of device memory must invalidate before its first read.
//! The transport fence has already completed when the caller reaches this boundary.
pub trait Memory {
    fn invalidate(&self, offset: usize, length: usize);
    fn copy(&self, offset: usize, out: &mut [u8]);
}

pub fn read(memory: &impl Memory, offset: usize, out: &mut [u8]) {
    memory.invalidate(offset, out.len());
    memory.copy(offset, out);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    struct Noncoherent {
        device: [u8; 8],
        cache: RefCell<[u8; 8]>,
        observed: Cell<bool>,
    }
    impl Memory for Noncoherent {
        fn invalidate(&self, offset: usize, length: usize) {
            self.cache.borrow_mut()[offset..offset + length].copy_from_slice(&self.device[offset..offset + length]);
            self.observed.set(true);
        }
        fn copy(&self, offset: usize, out: &mut [u8]) {
            assert!(self.observed.replace(false), "CPU read before invalidation");
            out.copy_from_slice(&self.cache.borrow()[offset..offset + out.len()]);
        }
    }
    #[test]
    fn each_observation_sees_device_bytes_instead_of_cached_poison() {
        for poison in [0xA5, 0x5A] {
            let memory = Noncoherent { device: [11, 22, 33, 44, 55, 66, 77, 88],
                cache: RefCell::new([poison; 8]), observed: Cell::new(false) };
            let mut out = [0; 3];
            read(&memory, 2, &mut out);
            assert_eq!(out, [33, 44, 55]);
            read(&memory, 5, &mut out);
            assert_eq!(out, [66, 77, 88]);
        }
    }
    #[test]
    fn readback_has_no_cpu_read_outside_the_observation_boundary() {
        // Contract section 1.8 explicitly requires a structure check as well as the
        // pixel cases: coherent VM memory cannot expose a missing invalidation.
        let core = include_str!("mod.rs");
        let observe = core.split("fn observe(").nth(1).unwrap().split("fn op_counters(").next().unwrap();
        assert!(!observe.contains("backing.read("));
        assert_eq!(observe.matches("observation::read(").count(), 2);
        let dma = include_str!("dma.rs");
        let boundary = dma.split("impl super::observation::Memory for Pages").nth(1).unwrap();
        assert!(boundary.contains("self.maintain(offset, length, true)"));
        assert!(boundary.contains("self.read(offset, out)"));
    }
}
