//! Native storage geometry and retirement, exercised with the real allocator/VM.
#![allow(dead_code)] // Standalone harness includes complete hardware memory modules.
extern crate alloc;
#[path = "../kernel/src/gpu/gk208/memory.rs"] mod memory;
#[path = "../kernel/src/gpu/gk208/vm.rs"] mod vm;
#[path = "../kernel/src/gpu/gk208/resource.rs"] mod resource;
use memory::allocator::Allocator;
use resource::{Layout, Resources};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Default)]
struct Model { words: BTreeMap<u64, u64>, writes: usize, invalidates: usize, fail: bool }
impl vm::Io for Model {
    fn read(&mut self, at: u64) -> Result<u64, &'static str> { Ok(*self.words.get(&at).unwrap_or(&0)) }
    fn write(&mut self, at: u64, value: u64) -> Result<(), &'static str> {
        self.writes += 1; self.words.insert(at, value); Ok(())
    }
    fn flush(&mut self) -> Result<(), &'static str> { Ok(()) }
    fn invalidate(&mut self, _: u64) -> Result<u32, &'static str> {
        self.invalidates += 1; if self.fail { Err("invalidate failed") } else { Ok(0) }
    }
    fn register(&mut self, _: usize) -> u32 { 0 }
}

#[test]
fn block_linear_geometry_and_literal_byte_offsets() {
    assert_eq!((resource::GOB_WIDTH, resource::GOB_HEIGHT, resource::GOB_BYTES), (64, 8, 512));
    // width, height, bytes/texel, pitch, block height in GOBs, padded bytes
    for (w, h, b, pitch, bh, total) in [
        (1, 1, 4, 64, 1, 512), (1, 1, 1, 64, 1, 512),
        (48, 17, 4, 192, 4, 6144), (64, 9, 1, 64, 2, 1024),
        (2560, 1600, 4, 10240, 16, 17039360),
        (16384, 16384, 4, 65536, 16, 1073741824),
    ] {
        let l = Layout::new(w, h, b).unwrap();
        assert_eq!((l.pitch, l.block_height, l.bytes), (pitch, bh, total));
        assert_eq!(l.tile_mode, bh.trailing_zeros() << 4);
        assert_eq!(l.block_bytes(), 512 * u64::from(bh));
        assert_eq!(l.allocation_bytes(), total.next_multiple_of(4096));
        assert!(l.offset(w - 1, h - 1, b - 1).unwrap() < total);
        assert!(l.offset(w, 0, 0).is_none()); assert!(l.offset(0, h, 0).is_none());
        assert!(l.offset(0, 0, b).is_none());
    }
    let l = Layout::new(48, 17, 4).unwrap();
    for (x, y, c, offset) in [(0,0,0,0), (3,0,3,15), (4,0,0,32),
        (8,0,0,256), (0,1,0,16), (0,2,0,64), (0,7,0,208),
        (0,8,0,512), (16,0,0,2048), (32,16,0,5120), (47,16,3,5423)] {
        assert_eq!(l.offset(x,y,c), Some(offset));
    }
    let l = Layout::new(64, 257, 1).unwrap();
    assert_eq!(l.offset(0,128,0), Some(8192));
    assert_eq!(l.offset(63,256,0), Some(16687));
    for (w,h,b) in [(0,1,4),(1,0,4),(16385,1,4),(1,16385,1),(1,1,2),(u32::MAX,1,4)] {
        assert!(Layout::new(w,h,b).is_err());
    }
}

#[test]
fn unref_with_a_held_fence_releases_no_vram_mapping_or_backing() {
    let mut a = Allocator::new(1 << 30, &[]).unwrap(); let baseline = a.stats();
    let mut io = Model::default(); let mut r = Resources::<Vec<u8>>::new();
    r.create(&mut a, &mut io, 7, 1, 1, 48, 17).unwrap();
    let backing = Arc::new(vec![0; 256 * 17]);
    r.attach(&mut a, &mut io, 7, backing.clone(), &[], backing.len() as u64).unwrap();
    let held = r.hold(7).unwrap();
    let used = a.stats(); let charged = r.vram_bytes();
    assert!(charged >= 8192); assert_eq!(Arc::strong_count(&backing), 2);
    let writes = io.writes; let invalidates = io.invalidates;
    r.unref(7).unwrap(); r.collect(&mut a, &mut io).unwrap();
    assert_eq!(a.stats(), used); assert_eq!(r.vram_bytes(), charged);
    assert_eq!(io.writes, writes); assert_eq!(io.invalidates, invalidates);
    assert_eq!(Arc::strong_count(&backing), 2);
    assert!(r.hold(7).is_err());
    drop(held); r.collect(&mut a, &mut io).unwrap();
    assert_eq!(a.stats(), baseline); assert_eq!(r.vram_bytes(), 0);
    assert_eq!(Arc::strong_count(&backing), 1);
}

#[test]
fn native_formats_and_pinned_buffers_keep_their_storage_classes() {
    let mut a = Allocator::new(1 << 30, &[]).unwrap(); let baseline = a.stats();
    let mut io = Model::default(); let mut r = Resources::<Vec<u8>>::new();
    // gpu-abi kind TARGET=1, TEXTURE=2, BUFFER=3; formats BGRA=0 RGBA=1 R8=2.
    for (id,k,f) in [(1,1,0),(2,1,1),(3,2,0),(4,2,1),(5,2,2)] {
        r.create(&mut a,&mut io,id,k,f,64,16).unwrap();
        assert!(r.layout(id).is_some());
        let va = r.address(id).unwrap(); let pte = r.pte_address(va).unwrap();
        assert_eq!((io.words[&pte] >> 36) & 255, 0xfe, "uncompressed color kind");
    }
    let used = a.stats(); let charge = r.vram_bytes();
    r.create(&mut a,&mut io,6,3,0,8192,1).unwrap();
    assert_eq!(a.stats(), used); assert_eq!(r.vram_bytes(), charge);
    assert!(r.layout(6).is_none());
    let backing = Arc::new(vec![0;8192]);
    r.attach(&mut a,&mut io,6,backing.clone(),&[0x12345000,0x812345000],8192).unwrap();
    let va = r.address(6).unwrap();
    assert_eq!(io.words[&r.pte_address(va).unwrap()], (0x12345000 >> 8) | 1 | (1<<32) | (2<<33));
    let held = r.hold(6).unwrap();
    for id in 1..=6 { r.unref(id).unwrap(); }
    r.collect(&mut a,&mut io).unwrap(); assert_eq!(Arc::strong_count(&backing), 2);
    drop(held); r.collect(&mut a,&mut io).unwrap();
    assert_eq!(a.stats(),baseline); assert_eq!(Arc::strong_count(&backing),1);
}

#[test]
fn failed_unmap_retains_storage_and_all_charges() {
    let mut a=Allocator::new(1<<30,&[]).unwrap(); let mut io=Model::default();
    let mut r=Resources::<Vec<u8>>::new();
    r.create(&mut a,&mut io,1,1,1,1,1).unwrap();
    let before=a.stats(); let charged=r.vram_bytes();
    r.unref(1).unwrap(); io.fail=true;
    assert!(r.collect(&mut a,&mut io).is_err());
    assert_eq!(a.stats(),before); assert_eq!(r.vram_bytes(),charged);
}
