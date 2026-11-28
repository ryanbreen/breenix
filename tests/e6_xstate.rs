//! Host fixtures for the x86 state layouts and user signal images.
#[path = "../kernel/src/arch_impl/x86_64/xstate_layout.rs"]
mod xstate_layout;

use xstate_layout::*;

const AVX_MASK: u64 = BASELINE | AVX;
const WIDE: u64 = AVX_MASK | AVX512;
const TRACE: u64 = 1 << 8;

fn fixture(instructions: u32) -> CpuState {
    CpuState::probe(
        BASELINE | AVX | AVX512,
        TRACE,
        2688,
        instructions,
        |bit| match bit {
            2 => (256, 576, 0),
            5 => (64, 1088, 0),
            6 => (512, 1152, 2),
            7 => (1024, 1664, 2),
            8 => (56, 0, 3),
            _ => unreachable!(),
        },
    )
    .unwrap()
}

#[test]
fn cpuid_layouts() {
    assert_eq!(Layout::legacy().image_size(), 512);
    for (instructions, backend) in [
        (0, Backend::Xsave),
        (1, Backend::Xsaveopt),
        (8, Backend::Xsaves),
    ] {
        let cpu = fixture(instructions);
        let wide = cpu.layout(WIDE, 0, backend).unwrap();
        assert_eq!(wide.standard_size, 2688);
        assert_eq!(wide.offset(2), 576);
        assert_eq!(wide.offset(5), if backend == Backend::Xsaves { 832 } else { 1088 });
        assert_eq!(wide.image_size(), if backend == Backend::Xsaves { 2432 } else { 2688 });
        assert!(wide.check_reported(2688, wide.image_size()));
        assert!(!wide.check_reported(2687, wide.image_size()));
        assert!(cpu.layout(WIDE | (1 << 63), 0, backend).is_none());
    }
    let cpu = fixture(8);
    let compacted = cpu.layout(AVX_MASK, TRACE, Backend::Xsaves).unwrap();
    assert_eq!(compacted.offset(8), 832);
    assert_eq!(compacted.image_size(), 888);
    assert!(compacted.check_reported(832, 888));
    let wide = cpu.layout(WIDE, 0, Backend::Xsaves).unwrap();
    assert!(!fixture(1).supports(&wide));
    assert!(cpu.supports(&wide));
    assert!(CpuState::probe(AVX_MASK, 0, u32::MAX, 0, |_| (128, u32::MAX - 63, 0)).is_none());
    assert!(CpuState::probe(AVX_MASK, 0, 832, 0, |_| (256, 576, 4)).is_none());
}

#[test]
fn standard_frames_and_legacy_import() {
    let layout = fixture(8).layout(WIDE, 0, Backend::Xsaves).unwrap();
    let mut user = vec![0; layout.standard_size];
    initialize(&mut user, false, 0);
    let bv = AVX | (1 << 7);
    user[512..520].copy_from_slice(&bv.to_le_bytes());
    user[576..832].fill(0x2a);
    user[1664..2688].fill(0x7b);
    user[832..1088].fill(0xdb); // Gap must never be published.
    let mut backing = vec![0; layout.image_size()];
    let mut exported = vec![0; layout.standard_size];
    assert!(layout.import(&mut backing, &mut user, true, 0xffbf));
    layout.export(&mut exported, &backing);
    assert_eq!(u64::from_le_bytes(exported[512..520].try_into().unwrap()), bv);
    assert!(exported[520..576].iter().all(|&byte| byte == 0));
    assert!(exported[576..832].iter().all(|&byte| byte == 0x2a));
    assert!(exported[832..1664].iter().all(|&byte| byte == 0));
    assert!(exported[1664..2688].iter().all(|&byte| byte == 0x7b));
    assert!(!layout.import(&mut backing, &mut user[..2687], true, 0xffbf));
    user[527] = 0x80;
    assert!(!layout.import(&mut backing, &mut user, true, 0xffbf));
    user[527] = 0;
    user[513] = 1; // Supervisor state.
    assert!(!layout.import(&mut backing, &mut user, true, 0xffbf));
    user[24..28].fill(0xff);
    assert!(layout.import(&mut backing, &mut user[..512], false, 0xffbf));
    layout.export(&mut exported, &backing);
    assert_eq!(&exported[24..28], &0xffbfu32.to_le_bytes());
    assert!(exported[576..].iter().all(|&byte| byte == 0));
}

#[test]
fn replacement_and_rollback_preserve_optimized_saves() {
    struct Registers {
        user: u8,
        last_restore: *const u8,
        modified: bool,
    }
    impl Registers {
        fn save(&mut self, image: &mut Box<u8>) {
            if self.modified || self.last_restore != &**image {
                **image = self.user;
            }
        }
        fn restore(&mut self, image: &Box<u8>) {
            self.user = **image;
            self.last_restore = &**image;
            self.modified = false;
        }
    }
    let mut registers = Registers { user: 0, last_restore: core::ptr::null(), modified: true };
    let mut user = Box::new(7);
    let mut prepared = Box::new(53);
    let mut kernel = Box::new(232);
    registers.restore(&user);
    registers.save(&mut user);
    registers.restore(&kernel);
    registers.user = 231;
    registers.modified = true;
    registers.save(&mut kernel);
    registers.restore(&user);

    assert!(!replace_live(&mut user, &mut prepared, &mut registers, Registers::save,
        |cpu, incoming| {
            cpu.restore(incoming); // Models a partial restore before #GP.
            false
        }, Registers::restore));
    assert_eq!((registers.user, *user, *kernel), (7, 7, 231));

    assert!(replace_live(&mut user, &mut prepared, &mut registers, Registers::save,
        |cpu, incoming| {
            cpu.restore(incoming);
            true
        }, Registers::restore));
    assert_eq!(registers.last_restore, &*user as *const u8);
    registers.save(&mut user);
    registers.restore(&kernel);
    assert_eq!(registers.user, 231);
    registers.restore(&user);
    assert_eq!(registers.user, 53);
}

#[test]
fn nested_altstack_exact_fit() {
    let high = 0x10000;
    let sp = high - 32;
    let frame = signal_placement(sp, 0x9000, high, false, 2688, 440, Some(16)).unwrap();
    assert_eq!(frame.image % 64, 0);
    assert_eq!(frame.rsp % 16, 8);
    assert!(frame.image + 2688 <= sp - 128);
    assert_eq!(
        signal_placement(sp, frame.rsp, high, false, 2688, 440, Some(16)),
        Some(frame),
    );
    assert!(signal_placement(sp, frame.rsp + 1, high, false, 2688, 440, Some(16)).is_none());
}
