//! Original allocator checks: charges reflect physical storage, never BAR aliases.
#![allow(dead_code)] // This standalone harness imports the complete hardware memory module.
extern crate alloc;
#[path = "../kernel/src/gpu/gk208/memory.rs"]
mod memory;
use memory::{allocator::{Allocator, Kind}, Range, Layout};

#[test]
fn charges_rounding_guards_and_reservation_union_and_reuses_only_live_handles() {
    let mut a = Allocator::new(1<<30, &[
        Range { start: 0, size: 128<<20 },
        Range { start: 4096, size: 4096 },
        Range { start: (1<<30)-4095, size: 4095 },
    ]).unwrap();
    let initial=a.stats();
    assert_eq!(initial.reserved,(128<<20)+4096);
    assert_eq!(initial.free+initial.reserved,1<<30);
    let x=a.allocate(4097,65536,Kind::Probe).unwrap();
    let y=a.allocate(4096,4096,Kind::Engine).unwrap();
    assert_eq!(x.range.size,8192);
    assert_eq!(x.range.start%65536,0);
    assert!(!x.guarded().overlaps(y.guarded()));
    let used=a.stats();
    assert_eq!(used.payload,12288);
    assert_eq!(used.guards,16384);
    assert_eq!(used.probe,8192);
    assert_eq!(used.engine,4096);
    assert_eq!(used.display,0);
    assert_eq!(used.charged,used.payload+used.guards);
    assert_eq!(used.free+used.charged+used.reserved,1<<30);
    a.release(x).unwrap();
    let z=a.allocate(4097,65536,Kind::Probe).unwrap();
    assert_eq!(x.range,z.range);
    assert!(a.release(x).is_err(),"stale handle freed a new allocation");
    assert_eq!(a.stats(),used);
    a.release(z).unwrap();a.release(y).unwrap();
    assert_eq!(a.stats(),initial);
}

#[test]
fn invalid_and_exhausted_requests_leave_every_charge_unchanged() {
    let mut a=Allocator::new(65536,&[]).unwrap();
    for (bytes,align) in [(0,4096),(u64::MAX,4096),(4096,0),(4096,3),(4096,2048),(65536,4096)] {
        let before=a.stats();assert!(a.allocate(bytes,align,Kind::Probe).is_err());assert_eq!(a.stats(),before);
    }
    let x=a.allocate(49152,4096,Kind::Probe).unwrap();
    let before=a.stats();assert!(a.allocate(4096,4096,Kind::Probe).is_err());assert_eq!(a.stats(),before);
    a.release(x).unwrap();assert_eq!(a.stats().free,65536);
    for r in [Range{start:u64::MAX-1,size:4096},Range{start:65535,size:2},Range{start:0,size:0}] {
        assert!(Allocator::new(65536,&[r]).is_err());
    }
}

#[test]
fn display_and_its_bar_tables_are_charged_once_and_stay_reserved() {
    let mut l=Layout::new(1<<30,&[Range{start:0,size:128<<20}],1280,1024).unwrap();
    let before=l.allocator.stats();
    assert_eq!(before.display,l.objects.iter().map(|o|o.range.size).sum());
    assert_eq!(before.guards,6*8192);
    assert_eq!(before.allocations,6);
    let scratch=l.allocator.allocate((1<<20)+4096,4096,Kind::Probe).unwrap();
    assert!(l.objects.iter().all(|o|!o.guarded().overlaps(scratch.guarded())));
    l.allocator.release(scratch).unwrap();
    assert_eq!(l.allocator.stats(),before);
}
