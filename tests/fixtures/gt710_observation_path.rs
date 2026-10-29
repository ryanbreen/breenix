//! Host substitutions for production CAPS/install and READBACK serialization.
use std::cell::RefCell;
use gpu_abi::{self as abi, capability, record, format, Plain};
#[path = "completion.rs"] pub mod gpu_completion;
#[path = "observation.rs"] pub mod observation;
#[path = "caps.rs"] pub mod caps;
const EIO: i64 = 5;
const ENODEV: i64 = 19;
const POISON: u32 = 0x3c5aa5c3;
mod gk208 {
    pub mod adapter {
        pub struct Device(pub crate::caps::BoardState);
        impl Device {
            pub fn caps(&self) -> gpu_abi::Caps {
                crate::caps::Profile::from_board(&self.0, self.0.channel).record(None)
            }
            pub fn needs_reset(&self) -> bool { false }
            pub fn queue_size(&self) -> usize { 4 }
        }
    }
}
enum Device { Gk208(gk208::adapter::Device), Virtual }
struct Core {
    dev: Option<Device>, caps: abi::Caps, status: u32, reason: &'static str, next_obs: u64,
    queue_size: usize,
}
impl Core {
    fn admission(&self) -> Result<(), (u32, u32)> {
        let c = match &self.dev { Some(Device::Gk208(d)) => d.caps(), _ => self.caps };
        if c.status == abi::status::ADMITTED { Ok(()) } else { Err((c.status, c.missing)) }
    }
    // CAPS
}
thread_local! { static CORE: RefCell<Core> = RefCell::new(Core {
    dev: None, caps: abi::Caps::ZERO, status: abi::status::NO_DEVICE, reason: "absent",
    next_obs: 700, queue_size: 0,
}); }
fn with_core<R>(f: impl FnOnce(&mut Core) -> R) -> Option<R> { Some(CORE.with(|c| f(&mut c.borrow_mut()))) }
// INSTALL
mod dev {
    thread_local! { pub static BACKEND: std::cell::Cell<u32> = const { std::cell::Cell::new(6) }; }
    pub fn backend() -> u32 { BACKEND.with(std::cell::Cell::get) }
    pub fn is_svga() -> bool { backend() == 5 }
    pub fn resource_id(_: u32) -> Option<u32> { Some(0x456) }
}
// Existing virtual wire definitions are compiled directly; the fixture does not
// supply the encoder's backend numbers, boundary or command-kind constants.
#[path = "@ROOT@/kernel/src/gpu/virgl/wire.rs"] pub mod wire;
pub mod svga { #[path = "@ROOT@/kernel/src/gpu/svga/wire.rs"] pub mod wire; }
#[derive(Clone, Copy)]
struct LastWrite { device_ctx: Option<u32>, serial: u64, fence: u64, answer: u32, kind: u32 }
#[derive(Clone, Copy)]
struct Binding { fence: u64, answer: u32 }
mod display {
    pub fn binding(_: &mut crate::Core, _: u64, _: u32, _: bool) -> Result<Option<crate::Binding>, i64> {
        Ok(Some(crate::Binding { fence: 401, answer: 0x60000000 }))
    }
}
mod readback_reason { pub const BINDING_UNKNOWN: u32 = 4; }
#[derive(Debug)]
pub enum Refused { Errno(i64), Eio(u32, u64) }
impl From<i64> for Refused { fn from(e: i64) -> Self { Self::Errno(e) } }
struct Composition { serial: u64, windows: Vec<[u64; 4]> }
fn row(last: LastWrite, answer: u32, unwritten: u32) -> Result<[u8; record::SIZE], Refused> {
    let (tgid, id, window) = (42, 8, false);
    let (fmt, tw, th, generation) = (format::R8G8B8A8, 64, 32, 9);
    let last = Some(last);
    let (transfer_fence, transfer_answer, readback_surface) = (123, answer, (71, 72));
    let composition = Some(Composition { serial: 99, windows: vec![[21, 22, 23, 24]] });
    let a = abi::Readback { target: 19, x: 2, y: 3, width: 4, height: 5, stride: 32, ..abi::Readback::ZERO };
    let release = || {};
    let refuse = |reason: u32, serial: u64| Err(Refused::Eio(reason, serial));
    // RECORD
    Ok(rec)
}
fn word(r: &[u8], at: usize) -> u32 { u32::from_le_bytes(r[at..at+4].try_into().unwrap()) }
fn wide(r: &[u8], at: usize) -> u64 { u64::from_le_bytes(r[at..at+8].try_into().unwrap()) }
fn board() -> caps::BoardState {
    let mut b = caps::BoardState::empty();
    b.chipset = 0x106; b.channel = (0x123000, 17); b.graphics_ready = true;
    b.created_classes = abi::gk208::CAPSET_IDS;
    b
}
#[test]
fn production_caps_and_readback_publish_native_fields() {
    with_core(|c| { c.dev = None; c.status = abi::status::NO_DEVICE; });
    assert!(install_native(gk208::adapter::Device(board())).is_ok());
    assert_eq!(with_core(|c| (c.status, c.queue_size)), Some((abi::status::MISSING_CAPABILITY, 4)));
    let c = with_core(|c| c.caps_record()).unwrap();
    assert_eq!((c.backend, c.status, c.missing), (6, abi::status::MISSING_CAPABILITY, capability::BGRA_RENDER));
    assert!(c.reason.starts_with(b"missing b8g8r8a8-render\0"));
    assert_eq!(c.capset_ids[..4], [0x927d, 0xa06f, 0xa0b5, 0xa197]);
    for kind in [abi::gk208::DRAW_RELEASE, abi::gk208::TRANSFER_RELEASE] {
        let r = row(LastWrite { device_ctx: Some(0x123), serial: 9001, fence: 42,
            answer: abi::gk208::SUCCESS, kind }, abi::gk208::SUCCESS, 0).unwrap();
        assert_eq!((word(&r, record::BACKEND), word(&r, record::BOUNDARY)), (6, record::BOUNDARY_NATIVE));
        assert_eq!((wide(&r, record::CONTEXT), wide(&r, record::RESOURCE), wide(&r, record::GENERATION)), (0x123, 0x456, 9));
        assert_eq!((wide(&r, record::RENDER_SERIAL), wide(&r, record::RENDER_FENCE), wide(&r, record::READBACK_FENCE)), (9001,42,123));
        assert_eq!(word(&r, record::RENDER_KIND), kind);
        for at in [record::READBACK_KIND, record::COPY_KIND] { assert_eq!(word(&r, at), abi::gk208::TRANSFER_RELEASE); }
        for at in [record::RENDER_STATUS, record::READBACK_STATUS, record::COPY_STATUS] { assert_eq!(word(&r, at), abi::gk208::SUCCESS); }
        for at in [record::READBACK_SURFACE, record::READBACK_MOB] { assert_eq!(wide(&r, at), 0); }
        assert_eq!((word(&r, record::X),word(&r, record::Y),word(&r, record::WIDTH),word(&r, record::HEIGHT)), (2,3,4,5));
        assert_eq!((word(&r, record::FLAGS),wide(&r, record::PRESENT_FENCE),wide(&r, record::COMPOSITE_SERIAL)), (3,401,99));
        assert_eq!(wide(&r, record::WINDOWS),21);
        assert!(r[record::RESERVED..].iter().all(|b| *b == 0));
    }
    // Serialization preserves an observed fault/unwritten count; it cannot
    // turn an unsuccessful transfer into the native success evidence tag.
    let r = row(LastWrite { device_ctx: Some(3), serial: 2, fence: 1,
        answer: 0x60010002, kind: abi::gk208::DRAW_RELEASE }, 0x60010002, 7).unwrap();
    for at in [record::RENDER_STATUS, record::READBACK_STATUS, record::COPY_STATUS] { assert_eq!(word(&r, at), 0x60010002); }
    assert_eq!(word(&r, record::UNWRITTEN), 7);
}
#[test]
fn installation_names_the_missing_live_capability() {
    for (class, missing) in [(0xa197, capability::FEATURE_3D), (0xa0b5, capability::CAPABILITY_SET)] {
        let mut b = board();
        b.created_classes.iter_mut().filter(|id| **id == class).for_each(|id| *id = 0);
        assert_eq!(install_native(gk208::adapter::Device(b)), Err(capability::name(missing)));
    }
}
#[test]
fn production_virtual_rows_keep_their_own_evidence() {
    with_core(|c| c.dev = Some(Device::Virtual));
    for (backend, answer, draw, read, copy, surface) in [(4, 0x1100, 0x207, 0x206, 0, (0,0)), (5, 1, 1152, 1104, 1042, (71,72))] {
        dev::BACKEND.with(|b| b.set(backend));
        let r = row(LastWrite { device_ctx: Some(3), serial: 2, fence: 1, answer, kind: draw }, answer, 0).unwrap();
        assert_eq!((word(&r, record::BACKEND),word(&r, record::BOUNDARY)),(backend,record::BOUNDARY_HOST));
        assert_eq!((word(&r,record::RENDER_KIND),word(&r,record::READBACK_KIND),word(&r,record::COPY_KIND)),(draw,read,copy));
        assert_eq!((wide(&r,record::READBACK_SURFACE),wide(&r,record::READBACK_MOB)),surface);
    }
}
