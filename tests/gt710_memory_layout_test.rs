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

use std::collections::BTreeMap;
struct Device {
    regs: BTreeMap<usize,u32>,
    words: BTreeMap<u64,u32>,
    alias: bool,
    drop_writes: bool,
    writes: Vec<u64>,
}
impl Device {
    fn new() -> Self {
        Self { regs: [(0x1700,0x1234),(0x22438,1),(0x2243c,1),(0x22554,0),(0x11020c,1024),
                      (0x610010,0x003f0009)].into(), words: BTreeMap::new(),
               alias:false,drop_writes:false,writes:Vec::new() }
    }
    fn address(&self, reg:usize) -> u64 {
        let a = (u64::from(self.regs[&0x1700]) << 16) + (reg-0x700000) as u64;
        if self.alias { a & 0xfffff } else { a }
    }
}
impl memory::Access for Device {
    fn read(&mut self, reg:usize) -> u32 {
        if (0x700000..0x800000).contains(&reg) { *self.words.get(&self.address(reg)).unwrap_or(&0) }
        else { *self.regs.get(&reg).unwrap_or(&0) }
    }
    fn write(&mut self,reg:usize,value:u32) {
        if (0x700000..0x800000).contains(&reg) {
            let a=self.address(reg);self.writes.push(a);
            if !self.drop_writes { self.words.insert(a,value); }
        } else { assert_eq!(reg,0x1700); self.regs.insert(reg,value); }
    }
}
#[test]
fn physical_windows_restore_and_full_patterns_reject_aliases_and_dropped_writes() {
    let layout=Layout::new(32<<20,&[Range{start:0,size:1<<20}],512,512).unwrap();
    for fault in 0..3 {
        let mut d=Device::new();d.alias=fault==1;d.drop_writes=fault==2;
        let mut p=memory::Pramin::new(&mut d,32<<20);
        let mut seen=0;
        let result=memory::exercise(&mut p,&layout,0x12345678,|phase,index,bytes| {
            assert!(phase<2);assert!(index<6);
            assert_eq!(bytes.len() as u64,layout.objects[index].range.size);
            seen+=bytes.len() as u64;
        });
        drop(p);
        assert_eq!(d.regs[&0x1700],0x1234);
        if fault==0 {
            assert_eq!(result.unwrap(),seen);
            assert_eq!(seen,2*layout.objects.iter().map(|o| o.range.size).sum::<u64>());
            assert!(d.writes.iter().all(|a|layout.objects.iter().any(|o|o.range.start<=*a&&*a<o.range.end().unwrap())));
        } else { assert!(result.is_err(),"fault {fault} escaped full read-back"); }
    }
    let mut d=Device::new();let mut p=memory::Pramin::new(&mut d,32<<20);
    assert!(p.read(32<<20).is_err());assert!(p.write(3,4).is_err());
    drop(p);assert_eq!(d.regs[&0x1700],0x1234);
}
#[test]
fn discovers_vram_separately_from_bar_and_reserves_inherited_instances() {
    let mut d=Device::new();
    let inventory=memory::discover(&mut d,128<<20).unwrap();
    assert_eq!(inventory.total,1<<30);
    assert_eq!(d.regs[&0x1700],0x1234);
    let layout=Layout::new(inventory.total,&inventory.reserved,1280,1024).unwrap();
    assert!(layout.objects.iter().all(|o|o.guarded().start>=128<<20 && o.guarded().end().unwrap()<=0x3f000000));
    assert!(!inventory.registers.is_empty());
    assert!(inventory.references.contains(&Range{start:0x3f000000,size:65536}));
    // A BAR instance's page-directory storage lowers the reserved upper envelope.
    d.regs.insert(0x1704,0x8003e000);
    d.words.insert(0x3e000200,0x3d000000);
    d.words.insert(0x3e000208,(128<<20)-1);
    d.words.insert(0x3d000000,0x003c0001);
    let inv=memory::discover(&mut d,128<<20).unwrap();
    assert!(inv.reserved.iter().any(|r|r.start<=0x3c000000 && r.end().unwrap()==inv.total));
    d.regs.insert(0x22438,2);d.regs.insert(0x11120c,512);
    assert!(memory::discover(&mut d,128<<20).is_err(),"mixed memory is outside bootstrap");
    d.regs.insert(0x22438,0);
    assert!(memory::discover(&mut d,128<<20).is_err());
}

#[test]
fn low_firmware_instances_are_already_inside_the_low_reservation() {
    let mut d=Device::new();d.regs.insert(0x610010,0x00001009);
    let inventory=memory::discover(&mut d,128<<20).unwrap();
    assert!(inventory.references.contains(&Range{start:1<<20,size:65536}));
    assert_eq!(inventory.reserved,[Range{start:0,size:128<<20},
                                  Range{start:(1<<30)-(1<<20),size:1<<20}]);
    let layout=Layout::new(inventory.total,&inventory.reserved,1280,1024).unwrap();
    assert!(layout.objects.iter().all(|o|o.guarded().start>=128<<20));
}

#[test]
fn sums_enabled_fbpa_units_within_each_partition() {
    let mut d=Device::new();d.regs.insert(0x2243c,2);d.regs.insert(0x11120c,1024);
    let both=memory::discover(&mut d,128<<20).unwrap();
    assert_eq!(both.total,2<<30);
    assert!(both.registers.contains(&(0x2243c,2)));
    assert!(both.registers.contains(&(0x11120c,1024)));
    d.regs.insert(0x22554,2);
    assert_eq!(memory::discover(&mut d,128<<20).unwrap().total,1<<30);
    d.regs.insert(0x22554,0);d.regs.insert(0x22438,2);
    d.regs.insert(0x11120c,512);
    assert!(memory::discover(&mut d,128<<20).is_err());
}

#[test]
fn idle_post_display_needs_no_inherited_evo_instance() {
    let mut d=Device::new();d.regs.insert(0x610010,0);
    let inv=memory::discover(&mut d,128<<20).unwrap();
    assert!(inv.references.is_empty());
    assert_eq!(inv.reserved,[Range{start:0,size:128<<20},Range{start:(1<<30)-(1<<20),size:1<<20}]);
    assert_eq!(Layout::new(inv.total,&inv.reserved,1280,1024).unwrap().objects.len(),6);
    d.regs.insert(0x610490,0x13);
    assert!(memory::discover(&mut d,128<<20).is_err(),"active channel without an instance escaped");
}

#[test]
fn opaque_post_control_uses_only_the_idle_reserved_carveout() {
    let mut d=Device::new();
    d.regs.insert(0x610010,1);
    d.regs.insert(0x1704,0x7ff00); // Direct VRAM mode, not a VM instance.
    d.regs.insert(0x1714,0x40000000);
    let before=d.regs.clone();
    let inv=memory::discover(&mut d,128<<20).unwrap();
    assert_eq!(d.regs,before,"discovery changed inherited controls");
    assert!(d.writes.is_empty(),"discovery wrote inherited storage");
    assert!(inv.references.is_empty());
    assert_eq!(inv.reserved,[Range{start:0,size:128<<20},
                             Range{start:(1<<30)-(1<<20),size:1<<20}]);
    let layout=Layout::new(inv.total,&inv.reserved,1280,1024).unwrap();
    assert!(layout.objects.iter().all(|o|o.guarded().start>=128<<20 &&
        o.guarded().end().unwrap()<=(1<<30)-(1<<20)));
    for i in 0..9 {
        let mut active=Device::new();active.regs.insert(0x610010,1);
        active.regs.insert(0x610490+i*16,0x13);
        assert!(memory::discover(&mut active,128<<20).is_err(),"opaque active channel {i}");
    }
    for unknown in [2,3,5,0x1001] {
        d.regs.insert(0x610010,unknown);
        assert!(memory::discover(&mut d,128<<20).is_err(),"unbounded opaque control {unknown:#x}");
    }
}
