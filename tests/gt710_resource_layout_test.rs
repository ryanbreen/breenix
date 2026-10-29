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

#[test]
fn transfer_backing_alias_and_staging_stay_pinned_until_completion() {
    let mut a=Allocator::new(1<<30,&[]).unwrap(); let baseline=a.stats();
    let mut io=Model::default(); let mut r=Resources::<Vec<u8>>::new();
    r.create(&mut a,&mut io,7,1,1,48,17).unwrap();
    let backing=Arc::new(vec![0;8192]);
    r.attach(&mut a,&mut io,7,backing.clone(),&[],8192).unwrap();
    let va=r.map_backing(&mut a,&mut io,7,&backing,&[0x12345000,0x812345000],8192).unwrap();
    assert_eq!(io.words[&r.pte_address(va).unwrap()],(0x12345000>>8)|1|(1<<32)|(2<<33));
    assert_eq!(io.words[&r.pte_address(va+4096).unwrap()],(0x812345000u64>>8)|1|(1<<32)|(2<<33));
    let used=a.stats();
    assert_eq!(r.map_backing(&mut a,&mut io,7,&backing,&[0x12345000,0x812345000],8192).unwrap(),va);
    assert_eq!(a.stats(),used);
    assert!(r.map_backing(&mut a,&mut io,7,&Arc::new(vec![0;8192]),&[0,4096],8192).is_err());
    let staged=r.staging(&mut a,&mut io,1,20,10).unwrap();
    let held=[r.hold(7).unwrap(),r.hold(staged).unwrap()];
    let directory=0x30000000;
    r.bind(&mut io,directory,0x123000,17).unwrap();
    assert_eq!(r.channel(),1);assert_eq!(r.identity(7),Some(7));
    r.bind(&mut io,directory,0x123000,18).unwrap();
    assert_eq!(r.channel(),2,"a reset mints a fresh opaque incarnation");
    let pde=directory+(va/(1<<27))*8;
    assert_ne!(io.words[&pde],0);
    r.unref(7).unwrap();r.unref(staged).unwrap();
    let used=a.stats();let writes=io.writes;
    r.collect(&mut a,&mut io).unwrap();
    assert_eq!(a.stats(),used);assert_eq!(io.writes,writes);
    assert_eq!(Arc::strong_count(&backing),2);
    drop(held);r.collect(&mut a,&mut io).unwrap();
    assert_eq!(a.stats(),baseline);assert_eq!(r.vram_bytes(),0);
    assert_eq!(io.words[&pde],0);assert_eq!(Arc::strong_count(&backing),1);
}

#[test]
fn uncertain_backing_mapping_is_never_reused_or_bound() {
    let mut a=Allocator::new(1<<30,&[]).unwrap();let mut io=Model::default();
    let mut r=Resources::<Vec<u8>>::new();
    r.create(&mut a,&mut io,7,1,1,1,1).unwrap();
    let backing=Arc::new(vec![0;4096]);
    r.attach(&mut a,&mut io,7,backing.clone(),&[],4096).unwrap();
    io.fail=true;
    assert!(r.map_backing(&mut a,&mut io,7,&backing,&[0x12345000],4096).is_err());
    io.fail=false;
    assert!(r.map_backing(&mut a,&mut io,7,&backing,&[0x12345000],4096).is_err());
    assert!(r.bind(&mut io,0x30000000,0x123000,17).is_err());
    let used=a.stats();r.unref(7).unwrap();
    assert!(r.collect(&mut a,&mut io).is_err());
    assert_eq!(a.stats(),used);assert_eq!(Arc::strong_count(&backing),2);
}

#[test]
fn context_vertex_buffer_accepts_the_shared_r8_format() {
    let mut a = Allocator::new(1 << 30, &[]).unwrap();
    let mut io = Model::default(); let mut r = Resources::<Vec<u8>>::new();
    r.create(&mut a, &mut io, 7, 3, 2, 128 * 1024, 1).unwrap();
    assert_eq!(r.layout(7), None);
    let backing = Arc::new(vec![0; 128 * 1024]);
    let pages: Vec<_> = (0..32).map(|i| 0x100000 + i * 4096).collect();
    r.attach(&mut a, &mut io, 7, backing, &pages, 128 * 1024).unwrap();
    r.unref(7).unwrap(); r.collect(&mut a, &mut io).unwrap();
    assert_eq!(r.vram_bytes(), 0);
}

#[test]
fn native_console_mapping_pins_existing_linear_storage_without_owning_it() {
    let mut a = Allocator::new(1 << 30, &[]).unwrap(); let mut io=Model::default();
    let surface=a.allocate(5120*1024,65536,memory::allocator::Kind::Display).unwrap();
    let baseline=a.stats(); let mut r=Resources::<Vec<u8>>::new();
    let id=u32::MAX;
    let va=r.import_linear(&mut a,&mut io,id,surface).unwrap();
    assert_eq!(va,r.address(id).unwrap());assert!(r.layout(id).is_none());
    assert_eq!(r.import_linear(&mut a,&mut io,id,surface).unwrap(),va);
    assert!(a.release(surface).is_err(),"mapped scanout must remain pinned");
    assert_eq!((a.stats().display,a.stats().charged-r.vram_bytes()),(baseline.display,baseline.charged));
    r.unref(id).unwrap();r.collect(&mut a,&mut io).unwrap();
    assert_eq!(a.stats(),baseline);a.release(surface).unwrap();
}

#[test]
fn failed_console_vm_creation_keeps_its_pin_and_accounts_retained_tables() {
    let mut a=Allocator::new(1<<30,&[]).unwrap();let mut io=Model{fail:true,..Model::default()};
    let surface=a.allocate(5120*1024,65536,memory::allocator::Kind::Display).unwrap();
    let before=a.stats().charged;let mut r=Resources::<Vec<u8>>::new();
    assert!(r.import_linear(&mut a,&mut io,u32::MAX,surface).is_err());
    assert_eq!(r.vram_bytes(),a.stats().charged-before);
    assert!(a.release(surface).is_err());
}

#[test]
fn fault_owner_survives_release_without_retaining_backing() {
    let mut a=Allocator::new(1<<30,&[]).unwrap(); let baseline=a.stats();
    let mut io=Model::default(); let mut r=Resources::<Vec<u8>>::new();
    r.create(&mut a,&mut io,41,3,0,8192,1).unwrap();
    let backing=Arc::new(vec![0;8192]);
    r.attach(&mut a,&mut io,41,backing.clone(),&[0x100000,0x101000],8192).unwrap();
    let va=r.address(41).unwrap();
    r.describe_frame(41,vec![resource::FrameRegion {start:4096,end:4352,name:"constants",command:7}]).unwrap();
    let held=r.hold(41).unwrap();
    r.unref(41).unwrap();r.collect(&mut a,&mut io).unwrap();
    let owner=r.owner(va+4096).unwrap();
    assert_eq!((owner.resource,owner.role,owner.region,owner.command),(41,"frame","constants",Some(7)));
    assert!(owner.unref && owner.mapped && !owner.released);
    drop(held);r.collect(&mut a,&mut io).unwrap();
    let owner=r.owner(va+4096).unwrap();
    assert_eq!((owner.resource,owner.region,owner.command),(41,"constants",Some(7)));
    assert!(owner.unref && !owner.mapped && owner.released);
    assert_eq!(Arc::strong_count(&backing),1);
    assert_eq!(a.stats(),baseline);
    assert!(r.owner(va-1).is_none());assert!(r.owner(va+8192).is_none());
    // Reusing a resource ID must not relabel a retired address.
    r.create(&mut a,&mut io,41,1,0,1,1).unwrap();
    assert_eq!(r.owner(va+4096).unwrap().region,"constants");
    assert_eq!(r.owner(r.address(41).unwrap()).unwrap().role,"target");
}

#[test]
fn fault_owner_distinguishes_texture_and_transfer_backing() {
    let mut a=Allocator::new(1<<30,&[]).unwrap();let mut io=Model::default();
    let mut r=Resources::<Vec<u8>>::new();
    r.create(&mut a,&mut io,9,2,2,8,8).unwrap();
    let backing=Arc::new(vec![0;4096]);
    r.attach(&mut a,&mut io,9,backing.clone(),&[],4096).unwrap();
    let image=r.address(9).unwrap();
    let upload=r.map_backing(&mut a,&mut io,9,&backing,&[0x100000],4096).unwrap();
    assert_eq!(r.owner(image).unwrap().role,"texture");
    assert_eq!(r.owner(upload).unwrap().region,"transfer-backing");
    r.unref(9).unwrap();r.collect(&mut a,&mut io).unwrap();
    assert!(r.owner(image).unwrap().released);
    assert!(r.owner(upload).unwrap().released);
}

#[test]
fn repeated_draws_reuse_retired_va_with_a_live_console() {
    let mut a=Allocator::new(1<<30,&[]).unwrap(); let mut io=Model::default();
    let surface=a.allocate(5120*1024,65536,memory::allocator::Kind::Display).unwrap();
    let mut r=Resources::<Vec<u8>>::new();
    r.import_linear(&mut a,&mut io,u32::MAX,surface).unwrap();
    let mut steady=None;
    // Hold a frame across collection; its VA and backing cannot be reused.
    let held_id=r.draw_buffer(&mut a,&mut io,128*1024).unwrap();
    let held_va=r.address(held_id).unwrap(); let held=r.hold(held_id).unwrap();
    r.unref(held_id).unwrap();
    for _ in 0..2050 {
        let id=r.draw_buffer(&mut a,&mut io,128*1024).unwrap();
        let va=r.address(id).unwrap();
        assert_eq!(va % (1<<17),0); assert_ne!(va,held_va);
        let pages:Vec<_>=(0..32).map(|i|0x100000+i*4096).collect();
        r.attach(&mut a,&mut io,id,Arc::new(vec![0;128*1024]),&pages,128*1024).unwrap();
        let used=a.stats();
        if let Some(previous)=steady { assert_eq!(used,previous,"draw VM charge must plateau"); }
        steady=Some(used);
        r.unref(id).unwrap(); r.collect(&mut a,&mut io).unwrap();
    }
    drop(held); r.collect(&mut a,&mut io).unwrap();
    let id=r.draw_buffer(&mut a,&mut io,128*1024).unwrap();
    assert_eq!(r.address(id),Some(held_va));
}
