//! Wide aperture snapshots preserve complete bytes and restore the firmware bank.
extern crate alloc;
#[path = "../kernel/src/gpu/gk208/memory.rs"] pub mod memory;
struct Aperture { bank:u32, pairs:usize }
impl memory::Access for Aperture {
    fn read(&mut self,reg:usize)->u32 {
        if reg==0x1700 {self.bank} else {
            let address=(u64::from(self.bank)<<16)+(reg-0x700000) as u64;
            (address as u32).wrapping_mul(0x10203)^0xaabbccdd
        }
    }
    fn write(&mut self,reg:usize,value:u32) {assert_eq!(reg,0x1700);self.bank=value;}
    fn read_pair(&mut self,reg:usize)->u64 {
        assert_eq!(reg%8,0); assert!((0x700000..=0x7ffff8).contains(&reg));
        self.pairs+=1;
        u64::from(self.read(reg))|u64::from(self.read(reg+4))<<32
    }
}
#[test]
fn wide_snapshot_preserves_unaligned_edges_bank_crossings_and_the_last_word() {
    for (start,size) in [(0xffffc,20),(0x100000,4096),(0x1ffffc,4)] {
        let mut a=Aperture{bank:0x1234,pairs:0};
        let mut p=memory::Pramin::new(&mut a,2<<20);
        let bytes=p.bytes(memory::Range{start,size}).unwrap();
        assert_eq!(memory::Access::read(p.access(),0x1700),(((start+size-1)&!0xfffff)>>16) as u32);
        drop(p);
        let expected:Vec<u8>=(start..start+size).step_by(4).flat_map(|v|((v as u32).wrapping_mul(0x10203)^0xaabbccdd).to_le_bytes()).collect();
        assert_eq!(bytes,expected);assert_eq!(a.bank,0x1234);
        if size>=8 {assert!(a.pairs>0);}
        assert!(memory::Pramin::new(&mut a,2<<20).bytes(memory::Range{start:(2<<20)-4,size:8}).is_err());
    }
}
