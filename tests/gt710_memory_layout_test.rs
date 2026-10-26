//! The display carveout excludes inherited storage and has disjoint guarded objects.
extern crate alloc;
#[path = "../kernel/src/gpu/gk208/memory.rs"]
mod memory;
use memory::{Layout, Range};

#[test]
fn allocates_two_scanouts_and_display_objects_outside_reserved_ranges() {
    let reserved = [Range { start: 0, size: 256 << 20 },
                    Range { start: 512 << 20, size: 1 << 20 },
                    Range { start: (1 << 30) - (16 << 20), size: 16 << 20 }];
    let layout = Layout::new(1 << 30, &reserved, 1280, 1024).unwrap();
    assert_eq!(layout.pitch, 5120);
    assert_eq!(layout.objects.len(), 6);
    assert_eq!(layout.objects.iter().map(|o| o.name).collect::<Vec<_>>(),
               ["scanout-a", "scanout-b", "evo", "notifier", "instance", "dma"]);
    for (i, object) in layout.objects.iter().enumerate() {
        assert_eq!(object.range.start % object.align, 0);
        assert_eq!(object.range.size % 4096, 0);
        assert!(object.range.size >= if i < 2 { 5120 * 1024 } else { 4096 });
        assert!(reserved.iter().all(|r| !r.overlaps(object.guarded())));
        assert!(layout.objects[..i].iter().all(|o| !o.guarded().overlaps(object.guarded())));
    }
    // An interior reservation forces first-fit to continue after the gap.
    let hole = layout.objects[0].guarded();
    let mut more = reserved.to_vec(); more.push(hole);
    let next = Layout::new(1 << 30, &more, 1280, 1024).unwrap();
    assert!(next.objects.iter().all(|o| !o.guarded().overlaps(hole)));
}

#[test]
fn refuses_invalid_or_exhausted_memory_without_wrapping() {
    let full = [Range { start: 0, size: 1 << 30 }];
    assert!(Layout::new(1 << 30, &full, 1280, 1024).is_err());
    for (w,h) in [(0,1024),(1280,0),(u32::MAX,1024),(1280,u32::MAX)] {
        assert!(Layout::new(1 << 30, &[], w,h).is_err());
    }
    for r in [Range { start: u64::MAX-1, size: 4096 }, Range { start: 1 << 30, size: 4096 }] {
        assert!(Layout::new(1 << 30, &[r], 1280,1024).is_err());
    }
    assert!(Layout::new(0, &[], 1280,1024).is_err());
}
