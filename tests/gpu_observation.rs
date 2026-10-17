//! The readback observation boundary (`docs/gui/gpu.md` section 1.8): every CPU read of
//! device-written backing invalidates first, and the readback path stages through frames
//! rather than kernel heap.
#[path = "../kernel/src/gpu/observation.rs"]
mod observation;

use observation::{read, Memory};
use std::cell::{Cell, RefCell};

/// Memory whose CPU view is a cache that only invalidation refreshes from the device.
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
        let memory = Noncoherent { device: [11, 22, 33, 44, 55, 66, 77, 88], cache: RefCell::new([poison; 8]), observed: Cell::new(false) };
        let mut out = [0; 3];
        read(&memory, 2, &mut out);
        assert_eq!(out, [33, 44, 55]);
        read(&memory, 5, &mut out);
        assert_eq!(out, [66, 77, 88]);
    }
}

const CORE: &str = include_str!("../kernel/src/gpu/mod.rs");

/// The source of the readback path: `op_readback` through `observe`.
fn readback_path() -> &'static str {
    CORE.split("fn op_readback(").nth(1).unwrap().split("fn op_counters(").next().unwrap()
}

#[test]
fn readback_has_no_cpu_read_outside_the_observation_boundary() {
    // Section 1.8 requires a structure check as well as the pixel cases: coherent VM
    // memory cannot expose a missing invalidation.
    let observe = CORE.split("fn observe(").nth(1).unwrap().split("fn op_counters(").next().unwrap();
    assert!(!observe.contains("backing.read("));
    assert_eq!(observe.matches("observation::read(").count(), 2);
    let dma = include_str!("../kernel/src/gpu/dma.rs");
    let boundary = dma.split("impl super::observation::Memory for Pages").nth(1).unwrap();
    assert!(boundary.contains("self.maintain(offset, length, true)"));
    assert!(boundary.contains("self.read(offset, out)"));
}

#[test]
fn readback_stages_through_charged_frames_not_heap() {
    let path = readback_path();
    for heap in ["Vec", "vec!", "try_reserve", "Box::"] {
        assert!(!path.contains(heap), "the readback path uses {heap}");
    }
    let staging = path.split("impl Staging").nth(1).unwrap().split("impl Drop for Staging").next().unwrap();
    assert!(staging.contains("MEMORY_QUOTA"));
    assert!(staging.contains("reserve_gpu_bytes"));
    assert!(staging.contains("Pages::new(frames)"));
}

#[test]
fn readback_probe_never_stores_a_byte_it_did_not_read() {
    let path = readback_path();
    let probe = path.split("fn probe_writable(").nth(1).unwrap().split("const STAGING_CHUNK").next().unwrap();
    assert!(!probe.contains("write_user_bytes"));
    assert!(probe.contains("cmpxchg_user_u32(word, seen, seen)"));
}
