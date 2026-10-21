//! B13 wire fixtures. These exercise the contract without depending on the lane's ABI wrappers.
use crate::*;
use super::kit::*;
use abi::Plain;

pub(crate) fn wire<const N: usize>(op: u32, words: &mut [u64; N]) -> i64 {
    call_at(op, words.as_mut_ptr() as u64, N * 8)
}

// gpu-window owns snapshot metadata and aliases; gpu-display owns restricted core resolution.
pub(crate) fn snapshot_lifetime(check_aliases: bool) -> CaseResult {
    interface()?;
    let mut claim = abi::Claim::ZERO;
    want_ok(call(abi::op::CLAIM_COMPOSITOR, &mut claim), "17.4", "gpu-window: B13 fixture claim")?;
    struct Release;
    impl Drop for Release { fn drop(&mut self) { call_at(18, 0, 0); } }
    let _release = Release;
    // The suite stage is already the display owner. TAKE is idempotent for that owner.
    // SAFETY: TAKE has no arguments.
    want_ok(unsafe { raw::syscall0(431) as i64 }, "18", "gpu-display: B13 fixture display ownership")?;
    let w = W::new(16, 16, 2)?;
    want_ok(w.register(b"B13"), "17.1", "REGISTER")?;
    w.frame(11).cite("17.3")?;
    let mut geometry = abi::Geometry::ZERO;
    geometry.id = w.id(); geometry.flags = abi::geometry_flags::VISIBLE;
    want_ok(call(abi::op::SET_GEOMETRY, &mut geometry), "18", "gpu-window: visible snapshot fixture")?;
    let mut context = bgl::Context::new().map_err(|r| format!("gpu-display: context {}", outcome(r))).cite("18")?;
    let mut query = [0u64; 12];
    query[0] = context.handle();
    want_ok(wire(29, &mut query), "17.1", "gpu-window: B13 COMPOSITE_SNAPSHOT sizing query")?;
    ensure(query[5] == 0 && query[2] >> 32 > 0, "17.1", "gpu-window: sizing query reserved a snapshot or omitted windows")?;
    let count = (query[2] >> 32) as usize;
    let damage_count = (query[4] >> 32) as usize;
    let mut windows = vec![[0u64; 10]; count];
    let mut damage = vec![[0u64; 2]; damage_count];
    let mut snapshot = [0u64; 12];
    snapshot[0] = context.handle();
    snapshot[1] = windows.as_mut_ptr() as u64;
    snapshot[2] = count as u64;
    snapshot[3] = damage.as_mut_ptr() as u64;
    snapshot[4] = damage_count as u64;
    want_ok(wire(29, &mut snapshot), "17.1", "gpu-window: B13 metadata reservation")?;
    let token = snapshot[5];
    ensure(token != 0 && snapshot[10] == claim.epoch, "17.1", "gpu-window: snapshot omitted token or claim epoch")?;
    // Cancel even on an assertion failure. Cancellation is the only alias destructor.
    struct Cancel(u64);
    impl Drop for Cancel { fn drop(&mut self) { let mut c = [0u64; 14]; c[0] = self.0; c[4] = 1; wire(19, &mut c); } }
    let _cancel = Cancel(token);
    let entry = windows.iter().find(|e| e[0] == w.id()).ok_or("gpu-window: snapshot omitted published window")?;
    let alias = entry[1];
    ensure(alias != 0 && ![0, 255].contains(&(alias >> 56)), "17.1", "gpu-window: source alias is absent or address-shaped")?;
    if check_aliases {
        let target = libbreenix::gpu::target_create(0, 16, 16).map_err(|r| format!("target {}", outcome(r)))?;
        context.begin(target, 16, 16);
        context.copy(alias, &[[0, 0, 16, 16, 0, 0]]);
        let (ret, diagnostic) = libbreenix::gpu::submit(context.handle(), context.encoded());
        libbreenix::gpu::resource_destroy(target);
        ensure(ret == -(EINVAL as i64) && diagnostic.error == 4, "17.4", "gpu-display: ordinary 416 SUBMIT resolved a snapshot alias or returned the wrong validation code")?;
        want_errno(libbreenix::gpu::resource_destroy(alias), EINVAL, "17.4", "gpu-window: RESOURCE_DESTROY of alias")?;
        let mut pixels = [0xA5; 16 * 16 * 4];
        let mut record = [0xA5; 1024];
        want_errno(libbreenix::gpu::readback(alias, [0, 0, 16, 16], &mut pixels, 64, &mut record), EINVAL, "17.4", "gpu-display: READBACK of sample alias")?;
        ensure(pixels.iter().chain(record.iter()).all(|&b| b == 0xA5), "17.4", "gpu-display: refused alias readback changed outputs")?;
    }
    // A reservation is metadata only: a stopped builder cannot exhaust a two-buffer pool.
    // Two new commits recycle the exact source named by the outstanding alias.
    for seed in [12, 13] { w.frame(seed).map_err(|e| format!("gpu-window: snapshot pinned client buffers between syscalls: {e}")).cite("17.3")?; }
    let mut cancel = [0u64; 14]; cancel[0] = token; cancel[4] = 1;
    want_ok(wire(19, &mut cancel), "17.1", "gpu-window: cancel unused snapshot")?;
    ensure(cancel[5..].iter().all(|&v| v == 0), "17.1", "gpu-display: cancellation issued work or a composite serial")?;
    let mut again = [0u64; 14]; again[0] = token; again[4] = 1;
    want_errno(wire(19, &mut again), ENOENT, "17.1", "gpu-window: cancel consumed snapshot")?;
    if !check_aliases {
        // Re-snapshot, then recycle its exact source before admission. The stream is
        // otherwise valid: scene work in C, followed by the full C-to-scanout copy.
        snapshot = [0; 12]; snapshot[0] = context.handle();
        snapshot[1] = windows.as_mut_ptr() as u64; snapshot[2] = count as u64;
        snapshot[3] = damage.as_mut_ptr() as u64; snapshot[4] = damage_count as u64;
        want_ok(wire(29, &mut snapshot), "17.1", "gpu-window: stale-source reservation")?;
        let _stale_cancel = Cancel(snapshot[5]);
        let alias = windows.iter().find(|e| e[0] == w.id()).ok_or("window missing")?[1];
        let width = snapshot[6] as u32; let height = (snapshot[6] >> 32) as u32;
        let scene = libbreenix::gpu::target_create(0,width,height).map_err(|r| format!("scene {}",outcome(r)))?;
        let scanout = libbreenix::gpu::scanout_target_create(0,width,height).map_err(|r| format!("scanout {}",outcome(r)))?;
        context.begin(scene,width,height); context.clear(0xff000000);
        context.copy(alias,&[[0,0,16,16,0,0]]);
        context.begin(scanout,width,height); context.copy(scene,&[[0,0,width as i32,height as i32,0,0]]);
        w.frame(14).cite("17.3")?;
        let mut submit = [0u64; 14]; submit[0]=snapshot[5]; submit[1]=scanout;
        submit[2]=context.encoded().as_ptr() as u64; submit[3]=context.encoded().len() as u64; submit[13]=scene;
        let result=wire(19,&mut submit);
        libbreenix::gpu::resource_destroy(scene); libbreenix::gpu::resource_destroy(scanout);
        want_errno(result,EAGAIN,"17.1","gpu-window: stale snapshot source must retry without sending")?;
        ensure(submit[5..13].iter().all(|&v|v==0),"18","gpu-display: stale snapshot created fences, record or reason")?;
    }
    Ok(())
}

// B13 op 22 is 48 bytes; record v2 is independent of the old window ABI crate.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Readback {
    pub dst: u64, pub dst_len: u64, pub record: u64,
    pub record_len: u32, pub reserved: u32, pub serial: u64,
    pub reason: u32, pub reserved2: u32,
}
// SAFETY: repr(C), 48 bytes of integer fields without padding.
unsafe impl abi::Plain for Readback { const ZERO: Self = unsafe { core::mem::zeroed() }; }
const _: () = assert!(core::mem::size_of::<Readback>() == 48);
pub(crate) fn word(record: &[u8; 1024], offset: usize) -> u32 {
    u32::from_le_bytes(record[offset..offset+4].try_into().unwrap())
}
pub(crate) fn quad(record: &[u8; 1024], offset: usize) -> u64 {
    u64::from_le_bytes(record[offset..offset+8].try_into().unwrap())
}
pub(crate) fn entries(record: &[u8; 1024]) -> Vec<(u64,u64,u64,u64)> {
    (0..word(record,196).min(16) as usize).map(|i| {
        let o=200+i*32; (quad(record,o),quad(record,o+8),quad(record,o+16),quad(record,o+24))
    }).collect()
}

// gpu-display owns the B13 descriptor preflight and post-send exception boundary.
pub(crate) fn descriptor_faults() -> CaseResult {
    let mut claim=abi::Claim::ZERO;
    want_ok(call(abi::op::CLAIM_COMPOSITOR,&mut claim),"17.4","gpu-window: descriptor fixture claim")?;
    struct Release;
    impl Drop for Release {fn drop(&mut self){call_at(18,0,0);}}
    let _release=Release;
    // SAFETY: TAKE has no arguments and is idempotent for a current owner.
    want_ok(unsafe {raw::syscall0(431) as i64},"18","gpu-display: descriptor fixture ownership")?;
    let before=counter("composites").cite("18")?;
    want_errno(call_at(19,unmapped(),112),EFAULT,"17.5","gpu-display: B13 unmapped 112-byte COMPOSITE")?;
    let page=Pages::filled(1,0,false,|b|b.fill(0)).cite("17.5")?;
    want_errno(call_at(19,page.addr(),112),EFAULT,"17.5","gpu-display: B13 read-only 112-byte COMPOSITE")?;
    want_errno(call_at(29,unmapped(),96),EFAULT,"17.5","gpu-window: unmapped SceneSnapshot")?;
    ensure(counter("composites").cite("18")?==before,"17.5","gpu-display: preflight EFAULT consumed a composite")
}
