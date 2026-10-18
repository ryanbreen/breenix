//! Host checks of the SVGA encoders, using byte storage in place of DMA frames.
extern crate alloc;

mod gpu {
    pub mod dma {
        use std::sync::{Arc, Mutex};
        thread_local! { static LAST: std::cell::RefCell<Option<Arc<Mutex<Vec<u8>>>>> = const { std::cell::RefCell::new(None) }; }
        pub const PAGE: usize = 4096;
        pub struct Pages { bytes: Arc<Mutex<Vec<u8>>> }
        impl Pages {
            pub fn new(count: usize) -> Option<Self> {
                let bytes = Arc::new(Mutex::new(vec![0; count * PAGE]));
                LAST.with(|last| *last.borrow_mut() = Some(bytes.clone()));
                Some(Self { bytes })
            }
            pub fn count(&self) -> usize { self.bytes() / PAGE }
            pub fn bytes(&self) -> usize { self.bytes.lock().unwrap().len() }
            pub fn page_addr(&self, index: usize) -> u64 { 0x1234_0000_0000 + (index * PAGE) as u64 }
            pub fn write(&self, offset: usize, bytes: &[u8]) {
                self.bytes.lock().unwrap()[offset..offset + bytes.len()].copy_from_slice(bytes);
            }
            pub fn maintain(&self, _: usize, _: usize, _: bool) {}
        }
        pub fn latest_allocation() -> Vec<u8> {
            LAST.with(|last| last.borrow().as_ref().unwrap().lock().unwrap().clone())
        }
    }
}

#[allow(dead_code)]
#[path = "../kernel/src/gpu/svga/wire.rs"]
mod wire;
#[allow(dead_code)]
#[path = "../kernel/src/gpu/svga/memory.rs"]
mod memory;
#[allow(dead_code)]
#[path = "../kernel/src/gpu/svga/commands.rs"]
mod commands;

use alloc::sync::Arc;
use commands::Commands;
use gpu::dma::{Pages, PAGE};
use memory::PageTree;

#[test]
fn copy_and_fence_keep_native_field_order_and_byte_lengths() {
    let mut c = Commands::new();
    c.copy(7, 9, [2, 3, 11, 13], [17, 19]);
    c.fence(23, 0x1234_5678_9abc_def0);
    assert_eq!(c.words(), &[
        1042, 60, 7, 0, 0, 9, 0, 0, 17, 19, 0, 11, 13, 1, 2, 3, 0,
        1216, 16, 0x9abc_def0, 0x1234_5678, 23, 0,
    ]);
}

#[test]
fn object_packets_preserve_64_bit_page_numbers_and_unbind_before_destroy() {
    let tree = PageTree::new(Arc::new(Pages::new(1).unwrap())).unwrap();
    let mut c = Commands::new();
    c.table(5, &tree, 8192);
    c.mob(29, &tree, 4096);
    c.surface(31, 1 << 24, 141, 73, 17);
    c.context(37, 41);
    c.cotable(37, 43, 2);
    c.destroy_context(37);
    assert_eq!(c.words(), &[
        1115, 24, 5, 0x2340_0000, 1, 8192, 0, 5,
        1135, 20, 29, 5, 0x2340_0000, 1, 4096,
        1097, 36, 31, 1 << 24, 141, 1, 0, 0, 73, 17, 1,
        1143, 4, 37, 1145, 12, 37, 41, 0,
        1207, 16, 37, 43, 2, 0,
        1145, 12, 37, u32::MAX, 0, 1144, 4, 37,
    ]);
}

#[test]
fn surface_and_screen_packets_use_published_payloads() {
    let mut c = Commands::new();
    c.bind_surface(7, 11);
    c.update_surface(7);
    c.readback_surface(7);
    c.destroy_surface(7);
    c.destroy_mob(11);
    c.screen(1280, 800);
    c.bind_screen(7);
    c.update_screen(1280, 800);
    assert_eq!(c.words(), &[
        1099, 8, 7, 11, 1102, 4, 7, 1104, 4, 7, 1098, 4, 7, 1094, 4, 11,
        1124, 28, 0, 1280, 800, 0, 0, 1, 96,
        1126, 16, 0, 7, 0, 0, 1127, 20, 0, 0, 0, 1280, 800,
    ]);
}

#[test]
fn page_tree_switches_depth_after_512_pages_and_rejects_empty_backing() {
    assert!(PageTree::new(Arc::new(Pages::new(0).unwrap())).is_none());
    for count in [1, 512, 513] {
        let data = Arc::new(Pages::new(count).unwrap());
        let tree = PageTree::new(data.clone()).unwrap();
        assert_eq!(Arc::strong_count(&data), 2);
        assert_eq!(tree.depth(), if count <= 512 { 5 } else { 6 });
        assert_eq!(tree.root_ppn(), 0x1_2340_0000 + if count <= 512 { 0 } else { 2 });
        let tables = gpu::dma::latest_allocation();
        let word = |at: usize| u64::from_le_bytes(tables[at..at + 8].try_into().unwrap());
        for i in 0..count { assert_eq!(word(i * 8), data.page_addr(i) >> 12); }
        if count > 512 {
            assert_eq!(word(2 * PAGE), 0x1_2340_0000);
            assert_eq!(word(2 * PAGE + 8), 0x1_2340_0001);
            assert!(tables[2 * PAGE + 16..].iter().all(|&v| v == 0));
        }
        assert_eq!(tree.data.count(), count);
    }
}
