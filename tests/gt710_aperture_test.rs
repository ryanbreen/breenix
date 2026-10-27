//! CPU aperture and DMA admission checks for E21 M5.3.
#![allow(dead_code)] // The harness imports complete pure hardware modules.
extern crate alloc;
#[path="../kernel/src/gpu/gk208/edid.rs"] mod edid;
#[path="../kernel/src/gpu/gk208/modeset.rs"] mod modeset;
#[path="../kernel/src/gpu/gk208/aperture.rs"] mod aperture;
#[path="../kernel/src/gpu/gk208/dma_policy.rs"] mod dma_policy;

#[test]
fn dma_checks_the_entire_page_without_truncation_or_assuming_iommu_identity() {
    use dma_policy::identity_page;
    assert_eq!(identity_page((1<<40)-4096, &[]).unwrap(), (1<<40)-4096);
    assert_eq!(identity_page(0x12345000, &[0,0x40000000]).unwrap(),0x12345000);
    for address in [0,1,4095,(1<<40)-1,1<<40,u64::MAX-4095] {
        assert!(identity_page(address,&[]).is_err(),"bad DMA page {address:x}");
    }
    for status in [1<<31,u32::MAX] {
        assert!(identity_page(4096,&[status]).is_err(),"active translation accepted");
    }
    assert_eq!(dma_policy::coherent_pte(0x12345000).unwrap(),0x500123451);
    assert!(dma_policy::coherent_pte(1<<40).is_err());
}

#[test]
fn dmar_walk_is_bounded_and_retains_all_remapping_units() {
    let mut bytes=vec![0u8;48];bytes[..4].copy_from_slice(b"DMAR");
    for address in [0xfed90000u64,0xfed91000] {
        let mut unit=[0u8;16];unit[2..4].copy_from_slice(&16u16.to_le_bytes());
        unit[8..].copy_from_slice(&address.to_le_bytes());bytes.extend(unit);
    }
    let len=bytes.len() as u32;bytes[4..8].copy_from_slice(&len.to_le_bytes());
    bytes[9]=0u8.wrapping_sub(bytes.iter().fold(0u8,|a,b|a.wrapping_add(*b)));
    assert_eq!(dma_policy::dmar_units(&bytes).unwrap(),[0xfed90000,0xfed91000]);
    for at in [0,4,9,50,56] {
        let mut bad=bytes.clone();bad[at]^=1;
        assert!(dma_policy::dmar_units(&bad).is_err());
    }
    // Invalid structure lengths with otherwise valid checksums must fail closed.
    for length in [0u16,3,15,4096] {
        let mut bad=bytes.clone();bad[50..52].copy_from_slice(&length.to_le_bytes());
        bad[9]=0;bad[9]=0u8.wrapping_sub(bad.iter().fold(0u8,|a,b|a.wrapping_add(*b)));
        assert!(dma_policy::dmar_units(&bad).is_err());
    }
}

#[derive(Default)]
struct Registers { events:Vec<(usize,u32)>, starved:bool, unacknowledged:bool }
impl modeset::Io for Registers {
    fn read(&mut self,r:usize)->u32 {
        if r==0x100c80 {(if self.starved {0}else{0x10000})|(if self.unacknowledged {0}else{0x8000})}
        else {0}
    }
    fn write(&mut self,r:usize,v:u32){self.events.push((r,v));}
    fn delay_us(&mut self,u:u32){assert_eq!(u,100);}
}
impl aperture::Io for Registers {
    fn drain_wc(&mut self){self.events.push((usize::MAX,0));}
}
#[test]
fn both_bar_controls_drain_wc_before_binding_and_invalidate_the_selected_directory() {
    for (bar,control) in [(1,0x1704),(2,0x1714)] {
        let mut r=Registers::default();
        aperture::activate(&mut r,bar,0x12345000,0x23456000).unwrap();
        assert_eq!(r.events[0],(usize::MAX,0));
        let binding=r.events.iter().position(|e|*e==(control,0x80012345)).unwrap();
        let pdb=r.events.iter().position(|e|*e==(0x100cb8,0x234560)).unwrap();
        assert!(binding>0 && pdb>binding);
        assert_eq!(r.events[pdb+1],(0x100cbc,0x80000005));
        assert!(r.events[..binding].contains(&(0x70000,1)));
    }
    for (bar,instance,pd) in [(0,4096,8192),(3,4096,8192),(1,1,8192),(2,1<<40,8192),(2,4096,3)] {
        let mut r=Registers::default();assert!(aperture::activate(&mut r,bar,instance,pd).is_err());
        assert!(r.events.is_empty(),"invalid request touched a device");
    }
    for (starved,unacknowledged) in [(true,false),(false,true)] {
        let mut r=Registers{starved,unacknowledged,..Registers::default()};
        assert!(aperture::activate(&mut r,2,4096,8192).is_err());
    }
}
