//! PROM access must work without an inherited VRAM ROM window and restore control.
#[path = "../kernel/src/gpu/gk208/rom.rs"]
mod rom;

struct Access {
    image: Vec<u8>,
    control: u32,
    writes: Vec<(usize, u32)>,
}
impl rom::Access for Access {
    fn read(&self, reg: usize) -> u32 {
        if reg == 0x88050 {
            return self.control;
        }
        assert!((0x300000..0x340000).contains(&reg), "unexpected register {reg:x}");
        assert_eq!(self.control & 1, 0, "PROM read while shadowed");
        let p = reg - 0x300000;
        u32::from_le_bytes(self.image[p..p + 4].try_into().unwrap())
    }
    fn write(&mut self, reg: usize, value: u32) {
        assert_eq!(reg, 0x88050);
        self.control = value;
        self.writes.push((reg, value));
    }
}
fn access() -> Access {
    let mut image = vec![0; 4096];
    image[..2].copy_from_slice(&[0x55, 0xaa]);
    image[0x18..0x1a].copy_from_slice(&0x40u16.to_le_bytes());
    image[0x40..0x44].copy_from_slice(b"PCIR");
    image[0x50..0x52].copy_from_slice(&8u16.to_le_bytes());
    image[4092..].copy_from_slice(&[1, 2, 3, 4]);
    Access { image, control: 0xa501, writes: Vec::new() }
}
#[test]
fn reads_the_complete_prom_and_restores_the_original_shadow_control() {
    let mut a = access();
    assert_eq!(rom::read(&mut a).unwrap(), a.image);
    assert_eq!(a.control, 0xa501);
    assert_eq!(a.writes, [(0x88050, 0xa500), (0x88050, 0xa501)]);
}
#[test]
fn malformed_headers_cannot_escape_the_aperture_or_leave_shadow_disabled() {
    for (offset, value) in [(0, 0), (0x19, 2), (0x40, 0), (0x50, 0), (0x51, 3)] {
        let mut a = access();
        a.image[offset] = value;
        assert!(rom::read(&mut a).is_err(), "offset {offset:x}");
        assert_eq!(a.control, 0xa501);
        assert_eq!(a.writes.last(), Some(&(0x88050, 0xa501)));
    }
}
