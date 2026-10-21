//! isolation: window authority and EFAULT safety (contract sections 17.4 to 17.6, 21, 22).

use crate::*;

pub(crate) const ISOLATION: &[Case] = &[
    pictured!("other-register", "An unrelated process registering another's window fails with EPERM", isolation_other_register),
    pictured!("other-commit", "An unrelated process committing another's window fails with EPERM", isolation_other_commit),
    pictured!("other-position", "A non-compositor setting a window's position fails with EPERM", isolation_other_position),
    pictured!("other-inject", "A non-compositor writing events into a window fails with EPERM", isolation_other_inject),
    pictured!("other-read", "An unrelated non-root process capturing another's window fails with EPERM, and root succeeds", isolation_other_read),
    pictured!("other-map", "Foreign GPU target handles are refused before submission or readback", isolation_other_map),
    pictured!("other-resize", "An unrelated process resizing another's window fails with EPERM", isolation_other_resize),
    pictured!("other-destroy", "An unrelated process destroying another's window fails with EPERM", isolation_other_destroy),
    pictured!("other-tree", "An unrelated non-root process mapping another's tree page fails with EPERM", isolation_other_tree),
    pictured!("texture-map", "An unrelated process cannot sample the compositor scanout through GPU resource handles", isolation_texture_map),
    pictured!("composite-denied", "A non-compositor compositing fails with EPERM", isolation_composite_denied),
    pictured!("claim-unprivileged", "A uid 1000 process claiming the compositor fails with EPERM, and root's claim succeeds", isolation_claim_unprivileged),
    pictured!("claim-twice", "A second compositor claim fails with EBUSY while the first lives", isolation_claim_twice),
    pictured!("claim-after-death", "The compositor role can be claimed again once its holder dies, with a higher epoch", isolation_claim_after_death),
    pictured!("compositor-authorized", "The compositor's position, inject, capture and composite calls succeed on every window", isolation_compositor_authorized),
    pictured!("pid-reuse", "A process that reuses a dead owner's PID number gains no authority over any window", isolation_pid_reuse),
    pictured!("efault-create", "An unmapped output pointer for create fails with EFAULT and creates nothing", isolation_efault_create),
    pictured!("efault-register", "An unmapped title pointer fails with EFAULT", isolation_efault_register),
    pictured!("efault-list", "An unmapped list buffer fails with EFAULT", isolation_efault_list),
    pictured!("efault-capture", "An unmapped capture destination fails with EFAULT", isolation_efault_capture),
    pictured!("efault-events", "An unmapped event-read buffer, or an argument structure made read-only while the read blocks, fails with EFAULT and consumes no event", isolation_efault_events),
    pictured!("efault-inject", "An unmapped event pointer given to the compositor's write fails with EFAULT", isolation_efault_inject),
    pictured!("efault-composite", "An unmapped composite descriptor fails with EFAULT", isolation_efault_composite),
    pictured!("efault-mouse", "An unmapped or read-only pointer-position or key-state buffer fails with EFAULT", isolation_efault_mouse),
    pictured!("efault-commit", "An unmapped commit metadata pointer fails with EFAULT, metadata with a nonzero reserved field fails with EINVAL, and neither commits anything", isolation_efault_commit),
    pictured!("efault-readback", "An unmapped readback destination fails with EFAULT", isolation_efault_readback),
    pictured!("efault-claim", "An unmapped claim argument fails with EFAULT and claims nothing", isolation_efault_claim),
    pictured!("efault-cross-page", "A buffer that runs from a mapped page into an unmapped one fails with EFAULT and has no side effect", isolation_efault_cross_page),
    pictured!("efault-overflow", "A pointer whose range overflows the address space fails with EFAULT", isolation_efault_overflow),
    pictured!("efault-readonly", "A read-only output buffer fails with EFAULT and is left unchanged", isolation_efault_readonly),
    pictured!("cow-output", "Output written into a copy-on-write page after fork lands only in the caller's copy", isolation_cow_output),
    pictured!("destroy-during-commit", "Destroying a window while another thread commits it ends with one outcome and no fault", isolation_destroy_during_commit),
    pictured!("test-hooks-absent", "The wait-stress ops fail with ENOSYS in the production kernel", isolation_test_hooks_absent),
    pictured!("retired-ops", "The retired demo ops and version 1 window ops fail with ENOSYS", isolation_retired_ops),
    pictured!("unknown-op", "An unknown op fails with EINVAL", isolation_unknown_op),
    pictured!("killable", "SIGKILL ends a process blocked in acquire or wait_event within 250 ms", isolation_killable),
    pictured!("quiet-errors", "1,000 failing window calls on one thread add nothing to that thread's kernel-output counter or to the unattributed counter; with `emit-diagnostic(path)` armed, each writer path of section 12.1 (direct writers, nested wrappers, the logger, a notice written out by another thread, and `emergency_print` into the unattributed counter) adds exactly its bytes, once, to the right counter, and a second emitting thread moves only its own", isolation_quiet_errors),
    pictured!("controls-latched", "In this suite boot the test controls are enabled because PID 1 is a suite binary, no op can clear or set the latch, and a child running `/sbin/init desktop` reports it is not PID 1 and makes no absence claim", isolation_controls_latched),
];

// ---------------------------------------------------------------------------------------
// isolation: window authority & EFAULT safety (contract sections 17.4 to 17.6, 21, 22)
// ---------------------------------------------------------------------------------------

use super::kit::*;
use abi::{op, Plain};

/// A registered window of this process with one committed frame of pattern `seed`.
fn shown(width: u32, height: u32, seed: u32, title: &[u8]) -> Result<W, libbreenix::suite::CaseError> {
    let w = W::new(width, height, 2)?;
    want_ok(w.register(title), "17.1", "REGISTER")?;
    w.frame(seed).cite("17.3")?;
    Ok(w)
}

/// `ret` must be `-errno`, in a helper's words.
fn errno_is(ret: i64, errno: i32, who: &str, what: &str) -> Result<(), String> {
    if ret == -(errno as i64) {
        Ok(())
    } else {
        Err(format!("{who}'s {what} {}; the contract gives {}", outcome(ret), errno_name(errno as i64)))
    }
}

/// Run `f` in an unrelated root process and in an unrelated uid 1000 process.
fn as_unrelated(what: &str, f: impl Fn(&str) -> Result<(), String>) -> CaseResult {
    for (who, uid) in [("an unrelated root process", None), ("an unrelated uid 1000 process", Some(1000))] {
        step("17.4", child(what, 3_000, uid, || f(who)))?;
    }
    Ok(())
}

fn geometry_op(id: u64, x: i32, y: i32, z: u32, flags: u32) -> i64 {
    let mut g = abi::Geometry::ZERO;
    g.id = id;
    g.x = x;
    g.y = y;
    g.z = z;
    g.flags = flags;
    call(op::SET_GEOMETRY, &mut g)
}

fn key_event(keycode: u32) -> abi::WindowEvent {
    let mut e = abi::WindowEvent::ZERO;
    e.kind = abi::event_kind::KEY_PRESS;
    e.keycode = keycode;
    e
}

fn write_events_at(id: u64, ptr: u64, count: u32) -> i64 {
    let mut w = abi::WriteEvents::ZERO;
    w.id = id;
    w.ptr = ptr;
    w.count = count;
    call(op::WRITE_EVENTS, &mut w)
}

fn read_events_at(id: u64, ptr: u64, capacity: u32) -> i64 {
    let mut r = abi::ReadEvents::ZERO;
    r.id = id;
    r.ptr = ptr;
    r.capacity = capacity;
    call(op::READ_EVENTS, &mut r)
}

fn list_at(ptr: u64, capacity: u32) -> i64 {
    let mut l = abi::List::ZERO;
    l.ptr = ptr;
    l.capacity = capacity;
    call(op::LIST, &mut l)
}

fn capture_at(id: u64, dst: u64, dst_len: u64, tree_dst: u64, tree_len: u64) -> i64 {
    let mut c = abi::Capture::ZERO;
    c.id = id;
    c.dst = dst;
    c.dst_len = dst_len;
    c.tree_dst = tree_dst;
    c.tree_len = tree_len;
    call(op::CAPTURE, &mut c)
}

fn claim() -> Result<u64, i64> {
    let mut c = abi::Claim::ZERO;
    let r = call(op::CLAIM_COMPOSITOR, &mut c);
    if r < 0 { Err(r) } else { Ok(c.epoch) }
}

fn release() -> i64 {
    call_at(op::RELEASE_COMPOSITOR, 0, 0)
}

fn composite() -> i64 {
    let mut c = [0u64; 14];
    super::handoff::wire(op::COMPOSITE, &mut c)
}

/// The listing of window `id`, which must be there.
fn info(id: u64) -> Result<abi::WinInfo, libbreenix::suite::CaseError> {
    listed(id).cite("17.1")?.ok_or_else(|| libbreenix::suite::CaseError::Fail(format!(
        "window {id} is no longer listed ({CONTRACT} section 17.2)")))
}

fn isolation_other_register() -> CaseResult {
    interface()?;
    let w = W::new(16, 16, 2)?;
    let id = w.id();
    as_unrelated("REGISTER", |who| {
        let t = b"stolen";
        errno_is(title_op(op::REGISTER, id, t.as_ptr() as u64, t.len() as u32), EPERM, who, &format!("REGISTER of window {id}"))?;
        errno_is(title_op(op::SET_TITLE, id, t.as_ptr() as u64, t.len() as u32), EPERM, who, &format!("SET_TITLE of window {id}"))
    })?;
    ensure(listed(id).cite("17.2")?.is_none(), "17.4", "a refused REGISTER from another process registered the window")
}

fn isolation_other_commit() -> CaseResult {
    interface()?;
    let w = shown(16, 16, 4, b"other-commit")?;
    let id = w.id();
    let before = info(id)?.commit_id;
    as_unrelated("COMMIT", |who| {
        let mut a = abi::Acquire::ZERO;
        a.id = id;
        errno_is(call(op::ACQUIRE, &mut a), EPERM, who, &format!("ACQUIRE of window {id}"))?;
        errno_is(commit(id, 0, &[abi::Rect::new(0, 0, 16, 16)], None).err().unwrap_or(0), EPERM, who, &format!("COMMIT of window {id}"))?;
        let mut n = abi::NoChange::ZERO;
        n.id = id;
        errno_is(call(op::COMMIT_NO_CHANGE, &mut n), EPERM, who, &format!("COMMIT_NO_CHANGE of window {id}"))
    })?;
    let after = info(id)?.commit_id;
    ensure(after == before, "17.4", format!("the window's commit id moved from {before} to {after} after refused commits from other processes"))?;
    let (_, px) = capture(id, 16, 16, None).map_err(|e| format!("CAPTURE {}", outcome(e))).cite("17.3")?;
    same(&px, 16, 16, false, "the owner's frame after the refused commits", "17.4", |x, y| pattern(4, x, y))
}

fn isolation_other_position() -> CaseResult {
    interface()?;
    let w = shown(16, 16, 5, b"other-position")?;
    let id = w.id();
    let vis = abi::geometry_flags::VISIBLE;
    let r = geometry_op(id, 100, 200, 3, vis);
    ensure(r == -(EPERM as i64), "17.4", format!("the owner's own SET_GEOMETRY {}; only the compositor places windows (EPERM)", outcome(r)))?;
    as_unrelated("SET_GEOMETRY", |who| errno_is(geometry_op(id, 100, 200, 3, vis), EPERM, who, &format!("SET_GEOMETRY of window {id}")))?;
    let i = info(id)?;
    ensure((i.x, i.y, i.z, i.flags & abi::win_flags::VISIBLE) == (0, 0, 0, 0), "17.4", format!(
        "refused SET_GEOMETRY calls moved the window to ({}, {}) z {} flags {:#x}", i.x, i.y, i.z, i.flags))
}

fn isolation_other_inject() -> CaseResult {
    interface()?;
    let w = shown(16, 16, 6, b"other-inject")?;
    let id = w.id();
    let e = key_event(30);
    let r = write_events_at(id, &e as *const abi::WindowEvent as u64, 1);
    ensure(r == -(EPERM as i64), "17.4", format!("the owner's own WRITE_EVENTS {}; only the compositor writes events (EPERM)", outcome(r)))?;
    as_unrelated("WRITE_EVENTS", |who| {
        let e = key_event(30);
        errno_is(write_events_at(id, &e as *const abi::WindowEvent as u64, 1), EPERM, who, &format!("WRITE_EVENTS to window {id}"))
    })?;
    let got = read_events(id).map_err(|r| format!("READ_EVENTS {}", outcome(r))).cite("19.2")?;
    ensure(got.is_empty(), "17.4", format!("refused WRITE_EVENTS calls delivered {} event(s), the first of kind {}", got.len(), got.first().map_or(0, |e| e.kind)))
}

fn isolation_other_read() -> CaseResult {
    interface()?;
    let w = shown(24, 12, 7, b"other-read")?;
    let id = w.id();
    step("17.4", child("CAPTURE as uid 1000", 3_000, Some(1000), || {
        let mut px = vec![0u8; 24 * 12 * 4];
        errno_is(capture_at(id, px.as_mut_ptr() as u64, px.len() as u64, 0, 0), EPERM, "an unrelated uid 1000 process", &format!("CAPTURE of window {id}"))?;
        if px.iter().any(|&b| b != 0) {
            return Err(String::from("a refused CAPTURE wrote pixels into the uid 1000 process's buffer (section 17.5)"));
        }
        Ok(())
    }))?;
    step("17.4", child("CAPTURE as root", 3_000, None, || {
        let (c, px) = capture(id, 24, 12, None).map_err(|r| format!("an unrelated root process's CAPTURE of window {id} {}; root may capture", outcome(r)))?;
        if (c.width, c.height) != (24, 12) {
            return Err(format!("root's CAPTURE reports {}x{}; the window is 24x12", c.width, c.height));
        }
        match first_diff(&px, 24, 12, false, |x, y| pattern(7, x, y)) {
            None => Ok(()),
            Some((x, y, got, want)) => Err(format!("root's CAPTURE has {got:02x?} at ({x},{y}); the committed frame has {want:02x?}")),
        }
    }))
}

// gpu-window: §2.2 replaces retired mapping authority with validator ownership of GPU handles (EINVAL).
fn isolation_other_map() -> CaseResult {
    super::handoff::snapshot_lifetime(true)?;
interface()?;
    let w=W::new(16,16,2)?;let target=w.c.buffer[0];
    let mut px=vec![0;1024];let mut record=[0;libbreenix::gpu::abi::record::SIZE];
    want_ok(libbreenix::gpu::readback(target,[0,0,16,16],&mut px,64,&mut record),"17.4","gpu-window: owner's GPU target readback")?;
    as_unrelated("foreign GPU handle",|who| {
        let mut c=bgl::Context::new().map_err(|r|format!("CONTEXT_CREATE {}",outcome(r)))?;
        c.begin(target,16,16);c.clear(0xff112233);
        let result=c.submit();
        if result.err()!=Some(-(EINVAL as i64)){return Err(format!("gpu-window: {who} submitted to another owner's GPU target"));}
        let mut px=vec![0xA5;1024];let mut record=[0;libbreenix::gpu::abi::record::SIZE];
        errno_is(libbreenix::gpu::readback(target,[0,0,16,16],&mut px,64,&mut record),EINVAL,who,"foreign GPU target")?;
        if px.iter().any(|&b|b!=0xA5){return Err("gpu-window: foreign readback returned pixels".into());}Ok(())
    })
}

fn isolation_other_resize() -> CaseResult {
    interface()?;
    let w = shown(16, 16, 9, b"other-resize")?;
    let id = w.id();
    as_unrelated("RESIZE", |who| {
        let mut r = abi::Resize::ZERO;
        r.id = id;
        r.width = 40;
        r.height = 40;
        errno_is(call(op::RESIZE, &mut r), EPERM, who, &format!("RESIZE of window {id}"))
    })?;
    let i = info(id)?;
    ensure((i.width, i.height) == (16, 16), "17.4", format!("a refused RESIZE left the window {}x{}; it is 16x16", i.width, i.height))?;
    let (_, px) = capture(id, 16, 16, None).map_err(|e| format!("CAPTURE {}", outcome(e))).cite("17.3")?;
    same(&px, 16, 16, false, "the frame after the refused resizes", "17.4", |x, y| pattern(9, x, y))
}

fn isolation_other_destroy() -> CaseResult {
    interface()?;
    let w = shown(16, 16, 10, b"other-destroy")?;
    let id = w.id();
    as_unrelated("DESTROY", |who| errno_is(call_id(op::DESTROY, id), EPERM, who, &format!("DESTROY of window {id}")))?;
    info(id)?;
    let (_, px) = capture(id, 16, 16, None).map_err(|e| format!("CAPTURE {}", outcome(e))).cite("17.3")?;
    same(&px, 16, 16, false, "the frame after the refused destroys", "17.4", |x, y| pattern(10, x, y))
}

fn isolation_other_tree() -> CaseResult {
    interface()?;
    let w = shown(16, 16, 11, b"other-tree")?;
    let id = w.id();
    step("17.4", child("MAP_TREE as uid 1000", 3_000, Some(1000), || {
        let mut m = abi::MapTree::ZERO;
        m.id = id;
        errno_is(call(op::MAP_TREE, &mut m), EPERM, "an unrelated uid 1000 process", &format!("MAP_TREE of window {id}"))
    }))?;
    step("17.4", child("MAP_TREE as root", 3_000, None, || {
        let mut m = abi::MapTree::ZERO;
        m.id = id;
        let r = call(op::MAP_TREE, &mut m);
        if r < 0 {
            return Err(format!("an unrelated root process's MAP_TREE of window {id} {}; root may map the tree", outcome(r)));
        }
        if m.addr == 0 || m.len != 2 * 64 * 1024 {
            return Err(format!("root's MAP_TREE gave address {:#x} and {} bytes; two buffers of 64 KiB tree are 131072", m.addr, m.len));
        }
        Ok(())
    }))
}

// gpu-window: §2.2 replaces MAP_COMPOSITOR_TEXTURE with a refused GPU scanout sample; gpu-display supplies the binding.
fn isolation_texture_map() -> CaseResult {
interface()?;
    let (mut k,_)=kiosk().cite("24")?;
    let fixture=k.ask("gpu-private-scanout").cite("24")?;
    let fields:Vec<_>=fixture.split_whitespace().collect();
    ensure(fields.len()==3 && fields[0]=="ok","17.4",format!("gpu-display: private scanout fixture: {fixture}"))?;
    let handle=fields[1].parse::<u64>().map_err(|e|e.to_string())?;
    let device=fields[2].parse::<u64>().map_err(|e|e.to_string())?;
    ensure(handle!=0 && ![0,255].contains(&(handle>>56)),"17.4","gpu-window: fixture did not expose an actual GPU resource handle")?;
    let (ret,modes)=libbreenix::gpu::modes();
    ensure(ret==0 && modes.binding_known==1 && modes.scanout_resource==device && device!=0,"17.4","gpu-display: compositor has no known GPU scanout binding")?;
    let sample=|who:&str|->Result<(),String>{
        let mut c=bgl::Context::new().map_err(|r|format!("context {}",outcome(r)))?;
        let target=libbreenix::gpu::target_create(0,16,16).map_err(|r|format!("target {}",outcome(r)))?;
        c.begin(target,16,16);c.clear(0xff000000);
        c.submit().and_then(|f|f.wait(2_000_000_000)).map_err(|r|format!("positive control {}",outcome(r)))?;
        c.begin(target,16,16);c.copy(handle,&[[0,0,16,16,0,0]]);
        let refused=c.submit().err()==Some(-(EINVAL as i64));
        libbreenix::gpu::resource_destroy(target);
        if !refused {return Err(format!("gpu-window: {who} sampled the compositor's live private scanout handle"));}Ok(())
    };
    sample("root non-compositor").cite("17.4")?;
    as_unrelated("sample compositor scanout",sample)
}

fn isolation_composite_denied() -> CaseResult {
    step("17.4", child("gpu-window: root without owner or claim",3_000,None,|| {
        let context=bgl::Context::new().map_err(|r|format!("gpu-display: context {}",outcome(r)))?;
        let mut snapshot=[0u64;12];snapshot[0]=context.handle();
        errno_is(super::handoff::wire(29,&mut snapshot),EPERM,"root without owner or claim","gpu-window: snapshot")
    }))?;
    interface()?;
    step("17.4", child("B13 claimant without display", 3_000, None, || {
        claim().map_err(|r| format!("gpu-window: claim {}",outcome(r)))?;
        let mut snapshot=[0u64;12];
        let result=super::handoff::wire(29,&mut snapshot);
        let submit=composite();
        release();
        errno_is(result,EPERM,"root claimant without display","gpu-window: COMPOSITE_SNAPSHOT")?;
        errno_is(submit,EPERM,"root claimant without display","gpu-display: COMPOSITE")
    }))?;
    // SAFETY: TAKE has no argument; current-owner TAKE is idempotent.
    want_ok(unsafe { raw::syscall0(431) as i64 },"18","gpu-display: display owner fixture")?;
    let mut snapshot=[0u64;12];
    want_errno(super::handoff::wire(29,&mut snapshot),EPERM,"17.4","gpu-window: display owner without claim snapshot")?;
    let _w = shown(16, 16, 12, b"composite-denied")?;
    let r = composite();
    ensure(r == -(EPERM as i64), "17.4", format!("COMPOSITE by root that is not the compositor {}; the contract gives EPERM", outcome(r)))?;
    step("17.4", child("COMPOSITE as uid 1000", 3_000, Some(1000), || errno_is(composite(), EPERM, "a uid 1000 process", "COMPOSITE")))?;
    let (_k, _) = kiosk().cite("24")?;
    let r = composite();
    ensure(r == -(EPERM as i64), "17.4", format!("COMPOSITE by root while kiosk_test holds the claim {}; the contract gives EPERM", outcome(r)))
}

fn isolation_claim_unprivileged() -> CaseResult {
    interface()?;
    step("17.4", child("CLAIM_COMPOSITOR as uid 1000", 3_000, Some(1000), || match claim() {
        Err(r) => errno_is(r, EPERM, "a uid 1000 process", "CLAIM_COMPOSITOR"),
        Ok(epoch) => Err(format!("a uid 1000 process claimed the compositor with epoch {epoch}; the contract gives EPERM")),
    }))?;
    step("18", child("CLAIM_COMPOSITOR as root", 3_000, None, || match claim() {
        Ok(epoch) if epoch >= 1 => match release() {
            0 => Ok(()),
            r => Err(format!("RELEASE_COMPOSITOR {}", outcome(r))),
        },
        Ok(epoch) => Err(format!("root's claim gave epoch {epoch}; epochs start at 1")),
        Err(r) => Err(format!("root's CLAIM_COMPOSITOR with no live claim {}", outcome(r))),
    }))
}

fn isolation_claim_twice() -> CaseResult {
    interface()?;
    let (mut k, epoch) = kiosk().cite("24")?;
    match claim() {
        Err(r) => want_errno(r, EBUSY, "18", "a second CLAIM_COMPOSITOR while kiosk_test's claim lives")?,
        Ok(e) => return at("18", format!("a second claim while kiosk_test's lives succeeded with epoch {e}; the contract gives EBUSY")),
    }
    let rel = k.ask("release").cite("24")?;
    ensure(rel == "ok", "18", format!("kiosk_test's RELEASE_COMPOSITOR answered `{rel}`"))?;
    let next = claim().map_err(|r| format!("CLAIM_COMPOSITOR after the release {}", outcome(r))).cite("18")?;
    let _ = release();
    ensure(next > epoch, "18", format!("the claim after a release has epoch {next}, not above the first claim's {epoch}"))
}

fn isolation_claim_after_death() -> CaseResult {
    interface()?;
    let (mut first, e1) = kiosk().cite("24")?;
    first.kill_checked().cite("24")?;
    let (_second, e2) = kiosk().map_err(|e| format!("after the first compositor died: {e}")).cite("18")?;
    ensure(e2 > e1, "18", format!("the claim after the holder died has epoch {e2}, not above the dead holder's {e1}"))
}

/// FNV-1a over `bytes`, as kiosk_test reports its captures.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3))
}

fn isolation_compositor_authorized() -> CaseResult {
    interface()?;
    let mine = shown(20, 10, 13, b"authorized-own")?;
    let mut raw = raw_client().cite("24")?;
    let created = raw.ask("create 12 8 2").cite("24")?;
    let theirs: u64 = created.strip_prefix("ok ").and_then(|s| s.split(' ').next()).and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("raw-window_test create answered `{created}`")).cite("24")?;
    let reg = raw.ask(&format!("register {theirs} authorized-raw")).cite("24")?;
    ensure(reg == "ok", "17.1", format!("raw-window_test register answered `{reg}`"))?;
    let filled = raw.ask(&format!("fill {theirs} 14")).cite("24")?;
    ensure(filled.starts_with("ok "), "17.3", format!("raw-window_test fill answered `{filled}`"))?;
    let (mut k, _) = kiosk().cite("24")?;
    let _ = k.ask("auto 0").cite("24")?;
    for (n, (id, w, h, seed)) in [(mine.id(), 20u32, 10u32, 13u32), (theirs, 12, 8, 14)].into_iter().enumerate() {
        let x = 100 + 50 * n as i32;
        let placed = k.ask(&format!("place {id} {x} 40 {n} 1")).cite("24")?;
        ensure(placed == "ok", "17.4", format!("the compositor's SET_GEOMETRY of window {id} answered `{placed}`"))?;
        let i = info(id)?;
        ensure((i.x, i.y, i.z) == (x, 40, n as u32) && i.flags & abi::win_flags::VISIBLE != 0, "17.4", format!(
            "after the compositor placed window {id} at ({x}, 40) z {n} visible, LIST gives ({}, {}) z {} flags {:#x}", i.x, i.y, i.z, i.flags))?;
        let injected = k.ask(&format!("event {id} {}", event_hex(&key_event(30)))).cite("24")?;
        ensure(injected.starts_with("ok 1 "), "17.4", format!("the compositor's WRITE_EVENTS to window {id} answered `{injected}`; one event is queued"))?;
        let (_, px) = capture(id, w, h, None).map_err(|r| format!("root's CAPTURE of window {id} {}", outcome(r))).cite("17.4")?;
        same(&px, w, h, false, &format!("window {id}'s frame"), "17.4", |x, y| pattern(seed, x, y))?;
        let captured = k.ask(&format!("capture {id} {w} {h}")).cite("24")?;
        let want = format!("ok {} {w} {h} {:016x}", i.commit_id, fnv(&px));
        ensure(captured == want, "17.4", format!("the compositor's CAPTURE of window {id} answered `{captured}`; the committed frame gives `{want}`"))?;
    }
    let got = read_events(mine.id()).map_err(|r| format!("READ_EVENTS {}", outcome(r))).cite("19.2")?;
    ensure(got.iter().any(|e| e.kind == abi::event_kind::KEY_PRESS && e.keycode == 30), "17.4", "the event the compositor wrote did not reach the owner")?;
    super::composition::compose(&mut k).map(|_| ())
}

fn isolation_pid_reuse() -> CaseResult {
    interface()?;
    // A child owns a registered window, reports it and its pid, and dies.
    let mut owner = Helper::call(&mut || {
        let mut c = abi::Create::ZERO;
        c.width = 8;
        c.height = 8;
        c.buffers = 2;
        if call(op::CREATE, &mut c) < 0 {
            return 1;
        }
        let t = b"pid-reuse";
        if title_op(op::REGISTER, c.id, t.as_ptr() as u64, t.len() as u32) < 0 {
            return 1;
        }
        let _ = io::write(Fd::from_raw(1), format!("{} {}\n", c.id, getpid()).as_bytes());
        loop {
            let _ = time::sleep_ms(1000);
        }
    }).cite("24")?;
    let line = owner.line(3_000).cite("24")?;
    let mut words = line.split(' ').filter_map(|w| w.parse::<u64>().ok());
    let (id, dead) = match (words.next(), words.next()) {
        (Some(id), Some(pid)) => (id, pid),
        _ => return at("24", format!("the owning child said `{line}`")),
    };
    let i = info(id)?;
    ensure(i.owner as u64 == dead, "17.4", format!("LIST gives owner {} for the child {dead}'s window", i.owner))?;
    owner.kill_checked().cite("24")?;
    // What a process given the dead owner's number could reach: nothing in the registry may
    // still name that owner once it is dead.
    let named: Vec<u64> = list().cite("17.1")?.iter().filter(|w| w.owner as u64 == dead).map(|w| w.id).collect();
    ensure(named.is_empty(), "17.4", format!("after owner {dead} died, LIST still gives it windows {named:?}"))?;
    ensure(listed(id).cite("17.2")?.is_none(), "17.2", format!("window {id} is still listed after its owner {dead} died"))?;
    let live = shown(8, 8, 15, b"pid-reuse-live")?;
    let live_id = live.id();
    // New processes try every op the owner had, on the dead owner's window and on a live one.
    // Breenix allocates numbers from a counter that never repeats, so none of them gets the
    // dead owner's number; a kernel that reuses numbers gives one of them that number here,
    // and it must be refused like the rest.
    for _ in 0..32 {
        step("17.4", child("a new process", 3_000, None, || {
            let me = getpid();
            for target in [id, live_id] {
                let want = if target == id { ENOENT } else { EPERM };
                let t = b"reuse";
                let checks = [
                    ("REGISTER", title_op(op::REGISTER, target, t.as_ptr() as u64, t.len() as u32)),
                    ("DESTROY", call_id(op::DESTROY, target)),
                    ("ACQUIRE", acquire(target, 0).err().unwrap_or(0)),
                ];
                for (what, ret) in checks {
                    if ret != -(want as i64) {
                        return Err(format!("process {me} (the dead owner was {dead}): {what} of window {target} {}; the contract gives {}",
                            outcome(ret), errno_name(want as i64)));
                    }
                }
            }
            Ok(())
        }))?;
    }
    Ok(())
}

fn isolation_efault_create() -> CaseResult {
    interface()?;
    // A valid request in a read-only page: the kernel reads it but cannot write the result.
    // The page and the unmapped address are made before the baseline is taken.
    let bad = unmapped();
    let page = Pages::filled(1, 0, false, |b| {
        let mut c = abi::Create::ZERO;
        c.width = 32;
        c.height = 32;
        c.buffers = 2;
        // SAFETY: Create is plain data and the page holds at least its size.
        unsafe { core::ptr::write_unaligned(b.as_mut_ptr() as *mut abi::Create, c) };
    }).cite("17.5")?;
    let _ = vm_size();
    let allocs = counter("window_buffer_allocs").cite("18")?;
    let vm = vm_size().cite("17.2")?;
    let r = call_at(op::CREATE, bad, core::mem::size_of::<abi::Create>());
    want_errno(r, EFAULT, "17.5", "CREATE with an unmapped argument")?;
    let r = call_at(op::CREATE, page.addr(), core::mem::size_of::<abi::Create>());
    want_errno(r, EFAULT, "17.5", "CREATE whose argument is read-only")?;
    let allocs_after = counter("window_buffer_allocs").cite("18")?;
    ensure(allocs_after == allocs, "17.5", format!("CREATE calls that failed with EFAULT allocated {} buffers", allocs_after - allocs))?;
    let vm_after = vm_size().cite("17.2")?;
    ensure(vm_after == vm, "17.5", format!("CREATE calls that failed with EFAULT changed VmSize from {vm} to {vm_after}"))
}

fn isolation_efault_register() -> CaseResult {
    interface()?;
    let w = W::new(8, 8, 2)?;
    want_errno(title_op(op::REGISTER, w.id(), unmapped(), 8), EFAULT, "17.5", "REGISTER with an unmapped title")?;
    let page = Pages::filled(1, 1, true, |b| b.fill(b'a')).cite("17.5")?;
    want_errno(title_op(op::REGISTER, w.id(), page.end_minus(4), 10), EFAULT, "17.5", "REGISTER with a title running into an unmapped page")?;
    ensure(listed(w.id()).cite("17.2")?.is_none(), "17.5", "a REGISTER that failed with EFAULT registered the window")
}

fn isolation_efault_list() -> CaseResult {
    interface()?;
    let _w = shown(8, 8, 16, b"efault-list")?;
    want_errno(list_at(unmapped(), 4), EFAULT, "17.5", "LIST into an unmapped buffer")
}

fn isolation_efault_capture() -> CaseResult {
    interface()?;
    let w = shown(16, 8, 17, b"efault-capture")?;
    let len = 16 * 8 * 4;
    want_errno(capture_at(w.id(), unmapped(), len, 0, 0), EFAULT, "17.5", "CAPTURE into an unmapped destination")?;
    let mut px = vec![0xA5u8; len as usize];
    let r = capture_at(w.id(), px.as_mut_ptr() as u64, len, unmapped(), 2 * 64 * 1024);
    want_errno(r, EFAULT, "17.5", "CAPTURE with an unmapped tree destination")?;
    ensure(px.iter().all(|&b| b == 0xA5), "17.5", "a CAPTURE that failed with EFAULT wrote into its pixel destination")
}

fn isolation_efault_events() -> CaseResult {
    interface()?;
    let w = shown(8, 8, 18, b"efault-events")?;
    let (mut k, _) = kiosk().cite("24")?;
    let wrote = k.ask(&format!("event {} {}", w.id(), event_hex(&key_event(44)))).cite("24")?;
    ensure(wrote.starts_with("ok 1 "), "19.2", format!("kiosk_test's WRITE_EVENTS answered `{wrote}`"))?;
    // The claim delivered CompositorChanged first; the key press follows it.
    want_errno(read_events_at(w.id(), unmapped(), 4), EFAULT, "17.5", "READ_EVENTS into an unmapped buffer")?;
    let got = read_events(w.id()).map_err(|r| format!("READ_EVENTS {}", outcome(r))).cite("19.2")?;
    ensure(got.iter().any(|e| e.kind == abi::event_kind::KEY_PRESS && e.keycode == 44), "17.5", format!(
        "after a READ_EVENTS that failed with EFAULT, the key press is gone: {} event(s) remain, kinds {:?}", got.len(), got.iter().map(|e| e.kind).collect::<Vec<_>>()))?;
    // A blocking read whose argument structure another thread makes read-only while it waits:
    // the event that wakes it cannot be reported, so the read fails with EFAULT and the event
    // stays queued.
    let page = Pages::new(1, 0, true).cite("17.5")?;
    let mut buf = vec![abi::WindowEvent::ZERO; 4];
    let mut r = abi::ReadEvents::ZERO;
    r.id = w.id();
    r.ptr = buf.as_mut_ptr() as u64;
    r.capacity = 4;
    r.flags = abi::READ_EVENTS_BLOCK;
    r.timeout_ns = 3_000_000_000;
    // SAFETY: the page is mapped writable and holds a ReadEvents.
    unsafe { core::ptr::write_unaligned(page.addr() as *mut abi::ReadEvents, r) };
    let at = page.addr();
    let reader = std::thread::spawn(move || call_at(op::READ_EVENTS, at, core::mem::size_of::<abi::ReadEvents>()));
    let _ = time::sleep_ms(100);
    memory::mprotect(page.base, page.len, memory::PROT_READ).map_err(|e| format!("mprotect: {e}")).cite("17.5")?;
    let wrote = k.ask(&format!("event {} {}", w.id(), event_hex(&key_event(45)))).cite("24")?;
    ensure(wrote.starts_with("ok 1 "), "19.2", format!("kiosk_test's WRITE_EVENTS answered `{wrote}`"))?;
    let blocked = reader.join().map_err(|_| String::from("the reading thread panicked")).cite("17.5")?;
    drop(buf);
    want_errno(blocked, EFAULT, "17.5", "a blocking READ_EVENTS whose argument structure was made read-only while it waited")?;
    let got = read_events(w.id()).map_err(|r| format!("READ_EVENTS {}", outcome(r))).cite("19.2")?;
    ensure(got.iter().any(|e| e.kind == abi::event_kind::KEY_PRESS && e.keycode == 45), "17.5", format!(
        "after a blocking READ_EVENTS failed with EFAULT on its read-only argument structure, the key press that woke it is gone: kinds {:?}",
        got.iter().map(|e| e.kind).collect::<Vec<_>>()))
}

fn isolation_efault_inject() -> CaseResult {
    interface()?;
    let w = shown(8, 8, 19, b"efault-inject")?;
    let (mut k, _) = kiosk().cite("24")?;
    let _ = read_events(w.id());
    let bad = k.ask(&format!("badwrite {}", w.id())).cite("24")?;
    ensure(bad == "err 14", "17.5", format!("the compositor's WRITE_EVENTS from an unmapped pointer answered `{bad}`; the contract gives EFAULT (err 14)"))?;
    let cross = k.ask(&format!("crosswrite {}", w.id())).cite("24")?;
    ensure(cross == "err 14", "17.5", format!("the compositor's WRITE_EVENTS of two events, the second running into an unmapped page, answered `{cross}`; the contract gives EFAULT (err 14)"))?;
    let got = read_events(w.id()).map_err(|r| format!("READ_EVENTS {}", outcome(r))).cite("19.2")?;
    ensure(got.is_empty(), "17.5", format!("WRITE_EVENTS calls that failed with EFAULT delivered {} event(s)", got.len()))
}

fn isolation_efault_composite() -> CaseResult {
    interface()?;
    super::handoff::descriptor_faults()
}

// gpu-display: B7 preserves bad-user-copy protection through GPU CAPS, RESOURCE_CREATE and SUBMIT.
fn isolation_efault_mouse() -> CaseResult {
    step("17.5", child("GPU user-copy EFAULT",5_000,None,|| {
        use libbreenix::gpu::{self,abi as ga};
        let hole=Pages::new(0,1,true)?;let bad=hole.addr();
        for (op,len) in [(ga::op::CAPS,core::mem::size_of::<ga::Caps>()),(ga::op::RESOURCE_CREATE,core::mem::size_of::<ga::ResourceCreate>())] {
            let ret=unsafe{gpu::call(op,bad as *mut u8,len)};
            errno_is(ret,EFAULT,"unmapped user page","gpu-display: GPU descriptor")?;
        }
        let c=bgl::Context::new().map_err(|r|format!("GPU context {}",outcome(r)))?;
        errno_is(gpu::submit_raw(c.handle(),bad,16).0,EFAULT,"unmapped user page","gpu-display: GPU stream")?;
        Ok(())
    }))?;
    // The pointer words (16 bytes) and the key words (8 bytes) alike.
    for len in [16u64, 8] {
        // SAFETY: the kernel checks the pointer.
        let r = unsafe { raw::syscall2(GET_MOUSE_POS, unmapped(), len) as i64 };
        ensure(r == -(EFAULT as i64), "17.5", format!("get_mouse_pos of {len} bytes into an unmapped buffer {}; the contract gives EFAULT", outcome(r)))?;
        let page = Pages::filled(1, 0, false, |b| b.fill(0x5A)).cite("17.5")?;
        // SAFETY: as above.
        let r = unsafe { raw::syscall2(GET_MOUSE_POS, page.addr(), len) as i64 };
        ensure(r == -(EFAULT as i64), "17.5", format!("get_mouse_pos of {len} bytes into a read-only buffer {}; the contract gives EFAULT", outcome(r)))?;
        ensure(page.bytes().iter().all(|&b| b == 0x5A), "17.5", format!("get_mouse_pos of {len} bytes changed a read-only buffer"))?;
    }
    Ok(())
}

fn isolation_efault_commit() -> CaseResult {
    interface()?;
    let w = shown(16, 16, 20, b"efault-commit")?;
    let before = info(w.id())?.commit_id;
    let a = w.acquire(1_000_000_000).map_err(|r| format!("ACQUIRE {}", outcome(r))).cite("17.3")?;
    fill(w.buf(a.buffer), 16, 16, 21);
    let mut c = abi::Commit::ZERO;
    c.id = w.id();
    c.buffer = a.buffer;
    c.damage_count = 1;
    c.damage[0] = abi::Rect::new(0, 0, 16, 16);
    c.meta = unmapped();
    c.meta_len = core::mem::size_of::<abi::Meta>() as u32;
    want_errno(call(op::COMMIT, &mut c), EFAULT, "17.5", "COMMIT with unmapped metadata")?;
    let mid = info(w.id())?.commit_id;
    ensure(mid == before, "17.5", format!("a COMMIT that failed with EFAULT moved the commit id from {before} to {mid}"))?;
    let (_, px) = capture(w.id(), 16, 16, None).map_err(|r| format!("CAPTURE {}", outcome(r))).cite("17.3")?;
    same(&px, 16, 16, false, "the front after a COMMIT that failed with EFAULT", "17.5", |x, y| pattern(20, x, y))?;
    // Metadata with a nonzero reserved field is EINVAL (section 17.1), for both commits.
    for (field, poke) in [("@52", 52usize), ("@124", 124)] {
        let mut bad = abi::Meta::ZERO;
        // SAFETY: the offset is inside the 128-byte Meta, at a reserved u32.
        unsafe { *((&mut bad as *mut abi::Meta as *mut u8).add(poke) as *mut u32) = 1 };
        c.meta = &bad as *const abi::Meta as u64;
        want_errno(call(op::COMMIT, &mut c), EINVAL, "17.1", &format!("COMMIT with Meta's reserved field {field} set"))?;
        let mut n = abi::NoChange::ZERO;
        n.id = w.id();
        n.meta = &bad as *const abi::Meta as u64;
        n.meta_len = core::mem::size_of::<abi::Meta>() as u32;
        want_errno(call(op::COMMIT_NO_CHANGE, &mut n), EINVAL, "17.1", &format!("COMMIT_NO_CHANGE with Meta's reserved field {field} set"))?;
    }
    let refused = info(w.id())?.commit_id;
    ensure(refused == before, "17.1", format!("commits refused for reserved Meta fields moved the commit id from {before} to {refused}"))?;
    let meta = abi::Meta::ZERO;
    let done = commit(w.id(), a.buffer, &[abi::Rect::new(0, 0, 16, 16)], Some(&meta))
        .map_err(|r| format!("committing the same buffer properly afterwards {}; the failed commit must leave it acquired", outcome(r))).cite("17.5")?;
    ensure(done.commit_id == before + 1, "17.5", format!("the commit after the failed one has id {}; it is the next after {before}", done.commit_id))
}

fn isolation_efault_readback() -> CaseResult {
    interface()?;
    let mut record = vec![0u8; 1024];
    let (width,height)=super::composition::screen_size()?;
    let screen = (width*height*4) as u64;
    let mut rb = super::handoff::Readback::ZERO;
    rb.dst = unmapped();
    rb.dst_len = screen;
    rb.record = record.as_mut_ptr() as u64;
    rb.record_len = record.len() as u32;
    want_errno(call(op::SCANOUT_READBACK, &mut rb), EFAULT, "17.5", "SCANOUT_READBACK into an unmapped destination")?;
    let mut dst = vec![0u8; screen as usize];
    let mut rb = super::handoff::Readback::ZERO;
    rb.dst = dst.as_mut_ptr() as u64;
    rb.dst_len = screen;
    rb.record = unmapped();
    rb.record_len = record.len() as u32;
    want_errno(call(op::SCANOUT_READBACK, &mut rb), EFAULT, "17.5", "SCANOUT_READBACK with an unmapped record")?;
    ensure(dst.iter().all(|&b| b == 0), "17.5", "a SCANOUT_READBACK that failed with EFAULT wrote pixels")
}

fn isolation_efault_claim() -> CaseResult {
    interface()?;
    want_errno(call_at(op::CLAIM_COMPOSITOR, unmapped(), core::mem::size_of::<abi::Claim>()), EFAULT, "17.5", "CLAIM_COMPOSITOR with an unmapped argument")?;
    let page = Pages::filled(1, 0, false, |b| b.fill(0)).cite("17.5")?;
    want_errno(call_at(op::CLAIM_COMPOSITOR, page.addr(), core::mem::size_of::<abi::Claim>()), EFAULT, "17.5", "CLAIM_COMPOSITOR with a read-only argument")?;
    // Neither claimed: a compositor can claim now.
    let (_k, _) = kiosk().map_err(|e| format!("after CLAIM_COMPOSITOR calls that failed with EFAULT: {e}")).cite("17.5")?;
    Ok(())
}

fn isolation_efault_cross_page() -> CaseResult {
    interface()?;
    let a = shown(8, 8, 22, b"cross-page-a")?;
    let _b = shown(8, 8, 23, b"cross-page-b")?;
    let entry = core::mem::size_of::<abi::WinInfo>();
    let page = Pages::filled(1, 1, true, |b| b.fill(0xA5)).cite("17.5")?;
    want_errno(list_at(page.end_minus(entry), 2), EFAULT, "17.5", "LIST of two entries starting one entry before an unmapped page")?;
    ensure(page.bytes().iter().all(|&b| b == 0xA5), "17.5", "a LIST that failed with EFAULT wrote its first entry into the mapped page")?;
    drop(page);
    let (mut k, _) = kiosk().cite("24")?;
    let wrote = k.ask(&format!("event {} {}", a.id(), event_hex(&key_event(45)))).cite("24")?;
    ensure(wrote.starts_with("ok 1 "), "19.2", format!("kiosk_test's WRITE_EVENTS answered `{wrote}`"))?;
    let size = core::mem::size_of::<abi::WindowEvent>();
    // Helper startup and command strings allocate through mmap and can reuse an
    // unmapped tail. Establish the same mapped/unmapped boundary after those
    // allocations, immediately before the operation whose EFAULT we require.
    let page = Pages::filled(1, 1, true, |b| b.fill(0xA5)).cite("17.5")?;
    want_errno(read_events_at(a.id(), page.end_minus(size / 2), 1), EFAULT, "17.5", "READ_EVENTS into a buffer running into an unmapped page")?;
    ensure(page.bytes().iter().all(|&b| b == 0xA5), "17.5", "a READ_EVENTS that failed with EFAULT wrote into the mapped page")?;
    let got = read_events(a.id()).map_err(|r| format!("READ_EVENTS {}", outcome(r))).cite("19.2")?;
    ensure(got.iter().any(|e| e.kind == abi::event_kind::KEY_PRESS && e.keycode == 45), "17.5", "a READ_EVENTS that failed with EFAULT consumed the key press")
}

fn isolation_efault_overflow() -> CaseResult {
    interface()?;
    let w = shown(8, 8, 24, b"efault-overflow")?;
    let near_end = u64::MAX - 63;
    want_errno(list_at(near_end, 1), EFAULT, "17.5", "LIST into a range that overflows the address space")?;
    want_errno(capture_at(w.id(), near_end, 8 * 8 * 4, 0, 0), EFAULT, "17.5", "CAPTURE into a range that overflows the address space")?;
    want_errno(title_op(op::SET_TITLE, w.id(), u64::MAX - 3, 8), EFAULT, "17.5", "SET_TITLE from a range that overflows the address space")?;
    want_errno(call_at(op::LIST, near_end, core::mem::size_of::<abi::List>()), EFAULT, "17.5", "LIST whose argument overflows the address space")
}

fn isolation_efault_readonly() -> CaseResult {
    interface()?;
    let w = shown(8, 8, 25, b"efault-readonly")?;
    let page = Pages::filled(2, 0, false, |b| b.fill(0x3C)).cite("17.5")?;
    want_errno(list_at(page.addr(), 2), EFAULT, "17.5", "LIST into a read-only buffer")?;
    want_errno(capture_at(w.id(), page.addr(), 8 * 8 * 4, 0, 0), EFAULT, "17.5", "CAPTURE into a read-only destination")?;
    ensure(page.bytes().iter().all(|&b| b == 0x3C), "17.5", "a call that failed with EFAULT changed the read-only buffer")
}

fn isolation_cow_output() -> CaseResult {
    interface()?;
    let w = shown(8, 8, 26, b"cow-output")?;
    let id = w.id();
    let page = Pages::filled(1, 0, true, |b| b.fill(0x5A)).cite("17.5")?;
    let addr = page.addr();
    // After the fork the page is shared copy-on-write; the child's LIST writes into it.
    step("17.5", child("LIST into a copy-on-write page", 3_000, None, || {
        let n = list_at(addr, 1);
        if n < 1 {
            return Err(format!("the child's LIST into its copy-on-write page {}", outcome(n)));
        }
        // SAFETY: the page is mapped in the child too, and holds one WinInfo.
        let got: abi::WinInfo = unsafe { core::ptr::read_unaligned(addr as *const abi::WinInfo) };
        if got.id == 0x5A5A_5A5A_5A5A_5A5A || got.flags & abi::win_flags::REGISTERED == 0 {
            return Err(format!("the child's LIST into its copy-on-write page left id {:#x} flags {:#x}", got.id, got.flags));
        }
        Ok(())
    }))?;
    ensure(page.bytes().iter().all(|&b| b == 0x5A), "17.5", format!(
        "the child's LIST output reached the parent's copy of the page (window {id} was listed); it must land only in the caller's copy"))
}

fn isolation_destroy_during_commit() -> CaseResult {
    interface()?;
    step("17.5", child("DESTROY racing COMMIT", 8_000, None, || {
        for round in 0..20u64 {
            let mut c = abi::Create::ZERO;
            c.width = 64;
            c.height = 64;
            c.buffers = 2;
            if call(op::CREATE, &mut c) < 0 {
                return Err(format!("round {round}: CREATE failed"));
            }
            let id = c.id;
            let done = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
            let counted = done.clone();
            let committer = std::thread::spawn(move || -> Result<u64, String> {
                loop {
                    let a = match acquire(id, 100_000_000) {
                        Ok(a) => a,
                        Err(r) if r == -(ENOENT as i64) => return Ok(counted.load(std::sync::atomic::Ordering::SeqCst)),
                        Err(r) => return Err(format!("ACQUIRE {}", outcome(r))),
                    };
                    match commit(id, a.buffer, &[abi::Rect::new(0, 0, 64, 64)], None) {
                        Ok(_) => {
                            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        }
                        Err(r) if r == -(ENOENT as i64) => return Ok(counted.load(std::sync::atomic::Ordering::SeqCst)),
                        Err(r) => return Err(format!("COMMIT {}", outcome(r))),
                    }
                }
            });
            // The destroy lands while the committer is in its loop: after its first commit,
            // and, the loop never ending on its own, before its last.
            let start = now_ms();
            while done.load(std::sync::atomic::Ordering::SeqCst) == 0 && since(start) < 2_000 {
                let _ = time::sleep_ms(1);
            }
            let before = done.load(std::sync::atomic::Ordering::SeqCst);
            let _ = time::sleep_ms(round % 5);
            let d = call_id(op::DESTROY, id);
            if d != 0 {
                return Err(format!("round {round}: DESTROY while another thread commits {}", outcome(d)));
            }
            if before == 0 {
                let _ = committer.join();
                return Err(format!("round {round}: the committing thread made no commit within 2 s, so DESTROY raced nothing"));
            }
            match committer.join() {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => return Err(format!("round {round}: the committing thread's {e}; racing a destroy it ends with success or ENOENT")),
                Err(_) => return Err(format!("round {round}: the committing thread panicked")),
            }
            if acquire(id, 0).err() != Some(-(ENOENT as i64)) {
                return Err(format!("round {round}: the destroyed window still answers ACQUIRE"));
            }
        }
        Ok(())
    }))
}

/// Start a helper that prints `blocking`, then blocks in `block`, and SIGKILL it once it is
/// blocked: how long until it is reaped.
fn kill_blocked(what: &str, block: fn() -> i64) -> Result<u64, String> {
    let mut f = move || {
        let _ = io::write(Fd::from_raw(1), b"blocking\n");
        let r = block();
        let _ = io::write(Fd::from_raw(1), format!("returned {r}\n").as_bytes());
        0
    };
    let mut h = Helper::call(&mut f)?;
    let line = h.line(3_000)?;
    if line != "blocking" {
        return Err(format!("the {what} helper said `{line}`"));
    }
    // A call that failed or returned at once has printed its result by now.
    if let Some(after) = h.try_line(100).map_err(|e| format!("the {what} helper: {e}"))? {
        return Err(format!("the {what} helper did not stay blocked: it printed `{after}`"));
    }
    let start = now_ms();
    h.kill_checked().map_err(|e| format!("the {what} helper: {e}"))?;
    Ok(since(start))
}

fn block_in_acquire() -> i64 {
    let mut c = abi::Create::ZERO;
    c.width = 8;
    c.height = 8;
    c.buffers = 2;
    if call(op::CREATE, &mut c) < 0 {
        return -1;
    }
    for _ in 0..2 {
        if acquire(c.id, 0).is_err() {
            return -2;
        }
    }
    acquire(c.id, u64::MAX).err().unwrap_or(0)
}

fn block_in_read_events() -> i64 {
    let mut c = abi::Create::ZERO;
    c.width = 8;
    c.height = 8;
    c.buffers = 2;
    if call(op::CREATE, &mut c) < 0 {
        return -1;
    }
    let mut ev = vec![abi::WindowEvent::ZERO; 4];
    let mut r = abi::ReadEvents::ZERO;
    r.id = c.id;
    r.ptr = ev.as_mut_ptr() as u64;
    r.capacity = 4;
    r.flags = abi::READ_EVENTS_BLOCK;
    r.timeout_ns = u64::MAX;
    call(op::READ_EVENTS, &mut r)
}

fn isolation_killable() -> CaseResult {
    interface()?;
    for (what, block) in [("ACQUIRE", block_in_acquire as fn() -> i64), ("READ_EVENTS", block_in_read_events as fn() -> i64)] {
        let ms = kill_blocked(what, block).cite("17.5")?;
        value(&format!("{}_kill_ms", what.to_lowercase()), ms as i64, "ms", Some((0, 250)));
        ensure(ms <= 250, "17.5", format!("a process blocked in {what} was reaped {ms} ms after SIGKILL; the contract bounds it at 250 ms"))?;
    }
    Ok(())
}

/// This thread's kernel output counter (section 22).
fn kernel_output(tid: u64) -> Result<u64, String> {
    let path = format!("/proc/{}/task/{tid}/kernel_output", getpid());
    let text = std::fs::read_to_string(&path).map_err(|e| format!("read {path}: {e}"))?;
    text.trim().parse().map_err(|_| format!("{path} holds `{}`, not a decimal count", text.trim()))
}

fn gettid() -> u64 {
    process::gettid().map(|t| t.raw()).unwrap_or(0)
}

/// Arm `emit-diagnostic` with writer `path` for thread `tid`, make one window op, and return
/// how much the thread's counter and the unattributed counter moved. Reading
/// `/proc/trace/counters` is itself kernel output on the reading thread (procfs prints its
/// census lines), so the unattributed counter is read only outside the thread's brackets.
fn emit(tid: u64, path: u64, want_thread: u64) -> Result<(u64, u64), String> {
    let f0 = libbreenix::test_control::fired(abi::tc_point::EMIT_DIAGNOSTIC).map_err(|e| format!("FIRED: {e}"))?;
    let u0 = counter("kernel_output_unattributed")?;
    let t0 = kernel_output(tid)?;
    libbreenix::test_control::arm(abi::tc_point::EMIT_DIAGNOSTIC, tid, path).map_err(|e| format!("ARM emit-diagnostic({path}): {e}"))?;
    let _ = window_version();
    let start = now_ms();
    let dt = loop {
        let dt = kernel_output(tid)? - t0;
        if dt == want_thread || since(start) > 200 {
            break dt;
        }
        let _ = time::sleep_ms(5);
    };
    // Once: a second op without arming adds nothing.
    let _ = window_version();
    let again = kernel_output(tid)? - t0;
    let du = counter("kernel_output_unattributed")? - u0;
    let fired = libbreenix::test_control::fired(abi::tc_point::EMIT_DIAGNOSTIC).map_err(|e| format!("FIRED: {e}"))? - f0;
    if fired != 1 {
        return Err(format!("emit-diagnostic({path}) fired {fired} times for one window op"));
    }
    if again != dt {
        return Err(format!("emit-diagnostic({path}) went on counting after its one op: {dt} then {again}"));
    }
    Ok((dt, du))
}

fn isolation_quiet_errors() -> CaseResult {
    interface()?;
    let tid = gettid();
    let un_before = counter("kernel_output_unattributed").cite("22")?;
    let before = kernel_output(tid).cite("22")?;
    let bad = unmapped();
    for i in 0..1_000u64 {
        let r = match i % 4 {
            0 => call_id(op::DESTROY, u64::MAX - i),
            1 => {
                let mut c = abi::Create::ZERO;
                c.buffers = 2;
                call(op::CREATE, &mut c)
            }
            2 => list_at(bad, 4),
            // SAFETY: an unknown op reads nothing.
            _ => unsafe { raw::syscall3(SYS_WINDOW, 0xFFFF, 0, 0) as i64 },
        };
        if r >= 0 {
            return at("17.5", format!("failing window call {i} succeeded ({r})"));
        }
    }
    let after = kernel_output(tid).cite("22")?;
    let un_after = counter("kernel_output_unattributed").cite("22")?;
    // The census lines the counters read printed land after `after` was taken.
    ensure(after == before, "17.5", format!("1,000 failing window calls added {} bytes to this thread's kernel output; window ops produce none", after - before))?;
    ensure(un_after == un_before, "17.5", format!("1,000 failing window calls added {} bytes of unattributed kernel output", un_after - un_before))?;
    use abi::emit_path as p;
    let record = b"[ WARN] test_control: ".len() as u64 + 32 + 1;
    for path in 1..=p::LAST {
        let (want_t, want_u) = match path {
            p::LOG_WARN => (record, 0),
            p::SERIAL_PRINTLN | p::NOTICE => (33, 0),
            p::EMERGENCY_PRINT => (0, 32),
            _ => (32, 0),
        };
        let (dt, du) = emit(tid, path, want_t).cite("22")?;
        ensure((dt, du) == (want_t, want_u), "22", format!(
            "emit-diagnostic({path}) added {dt} bytes to the thread and {du} unattributed; the contract gives {want_t} and {want_u}"))?;
    }
    // A second thread emitting moves only its own counter.
    let mine_before = kernel_output(tid).cite("22")?;
    let other = std::thread::spawn(|| -> Result<(u64, u64), String> {
        let t = gettid();
        emit(t, p::SERIAL_PRINTLN, 33).map(|(dt, _)| (t, dt))
    }).join().map_err(|_| String::from("the second emitting thread panicked")).cite("22")?.cite("22")?;
    let mine_after = kernel_output(tid).cite("22")?;
    ensure(other.1 == 33, "22", format!("the second thread's emit-diagnostic(1) added {} bytes to its own counter; the contract gives 33", other.1))?;
    ensure(mine_after == mine_before, "22", format!("the second thread's emission moved this thread's counter by {}", mine_after - mine_before))
}

fn isolation_controls_latched() -> CaseResult {
    let state = test_control_state();
    ensure(state == 1, "21", format!(
        "test_control(STATE) {} in this suite boot, where PID 1 is /sbin/suite-breengel; the contract latches test controls on here, and STATE returns 1", outcome(state)))?;
    for op in [4u64, 5, 100, u32::MAX as u64] {
        // SAFETY: these ops take no argument and must be refused before reading one.
        let r = unsafe { raw::syscall3(SYS_TEST_CONTROL, op, 0, 0) as i64 };
        ensure(r == -22, "21", format!("test_control(op {op}) {}; no op sets or clears the latch, and every op above 3 fails with EINVAL", outcome(r)))?;
    }
    libbreenix::test_control::disarm_all().map_err(|e| format!("DISARM_ALL: {e}")).cite("21")?;
    let state = test_control_state();
    ensure(state == 1, "21", format!("after DISARM_ALL, test_control(STATE) {}; the latch stays on", outcome(state)))?;
    // A child running `init desktop`, as uid 1000 with RLIMIT_NPROC 1 so it cannot start the
    // desktop's services.
    let mut f = || {
        let limit = [1u64, 1u64];
        // SAFETY: prlimit64 reads two u64s from `limit`.
        let _ = unsafe { raw::syscall4(libbreenix::syscall::nr::PRLIMIT64, 0, 6, limit.as_ptr() as u64, 0) };
        if setuid(1000) != 0 {
            return 125;
        }
        let argv: [*const u8; 3] = [b"init\0".as_ptr(), b"desktop\0".as_ptr(), core::ptr::null()];
        let _ = process::execv(b"/sbin/init\0", argv.as_ptr());
        127
    };
    let mut init = Helper::call(&mut f).cite("21")?;
    let mut lines = Vec::new();
    let start = now_ms();
    while since(start) < 3_000 {
        match init.line(3_000) {
            Ok(l) => {
                let done = l.contains("mode desktop");
                lines.push(l);
                if done {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    init.kill();
    let text = lines.join(" | ");
    ensure(lines.iter().any(|l| l == "[init] not PID 1: test-control check skipped"), "21", format!(
        "a child running /sbin/init desktop printed `{text}`; it must print `[init] not PID 1: test-control check skipped`"))?;
    ensure(!lines.iter().any(|l| l.contains("test controls absent") || l.contains("test controls PRESENT")), "21", format!(
        "a child running /sbin/init desktop made an absence claim: `{text}`"))
}

#[repr(C)]
struct FbDrawCmd {
    op: u32,
    p1: i32,
    p2: i32,
    p3: i32,
    p4: i32,
    color: u32,
}

fn fb_draw(op: u32, p1: i32, p2: i32, p3: i32, p4: i32) -> i64 {
    let cmd = FbDrawCmd { op, p1, p2, p3, p4, color: 0 };
    // SAFETY: the kernel reads one FbDrawCmd from this live stack value.
    unsafe { raw::syscall1(SYS_FB_DRAW, &cmd as *const FbDrawCmd as u64) as i64 }
}

fn isolation_test_hooks_absent() -> CaseResult {
    let mut stats = [0u64; 4];
    let ptr = stats.as_mut_ptr() as u64;
    let mut seen = vec![(27, fb_draw(27, 0, 0, 0, 0)), (29, fb_draw(29, 0, 0, 0, 0)),
        (30, fb_draw(30, ptr as u32 as i32, (ptr >> 32) as i32, 4, 0))];
    // Op 28 waits until woken: it runs in a child, which op 29 wakes if the op exists.
    let (rd, wr) = io::pipe()?;
    let mut body=|| {
        let r=fb_draw(28,0,0,0,0);
        let _=io::write(wr,&r.to_le_bytes());
        0
    };
    let pid=libbreenix::suite::spawn(&[(wr,wr)],libbreenix::suite::Run::Call(&mut body))?;
            let _ = io::close(wr);
            let _ = time::sleep_ms(100);
            let _ = fb_draw(29, 0, 0, 0, 0);
            let mut status = 0;
            let start = now_ms();
            while process::waitpid(pid.raw() as i32, &mut status, WNOHANG).is_ok_and(|p| p.raw() == 0) && now_ms() - start < 1_000 {
                let _ = time::sleep_ms(5);
            }
            let mut buf = [0u8; 8];
            let got = if io::read(rd, &mut buf).is_ok_and(|n| n == 8) { i64::from_le_bytes(buf) } else { i64::MIN };
            let _ = io::close(rd);
            seen.insert(1, (28, got));
    let live: Vec<String> = seen.iter().filter(|(_, r)| *r != -38).map(|(op, r)| {
        if *r == i64::MIN { format!("op {op} blocked") } else { format!("op {op} {}", outcome(*r)) }
    }).collect();
    ensure(live.is_empty(), "17.6", format!("the wait-stress ops do not return ENOSYS in this production kernel: fb_draw {}; the contract requires ENOSYS from each", live.join(", ")))
}

// gpu-apps: §2.2 retires framebuffer syscalls and pixel mappings with their last consumers.
fn isolation_retired_ops() -> CaseResult {
    for number in [SYS_FB_INFO,SYS_FB_DRAW,SYS_FB_MMAP] {
        let mut storage=[0u64;16];
        let ret=unsafe{raw::syscall1(number,storage.as_mut_ptr() as u64) as i64};
        want_errno(ret,38,"17.6",&format!("gpu-apps: retired syscall {number}"))?;
    }
    let mut buffers=abi::MapBuffers::ZERO;
    let mut texture=abi::MapTree::ZERO;
    want_errno(call(op::MAP_COMMITTED,&mut buffers),38,"17.6","gpu-apps: MAP_COMMITTED")?;
    want_errno(call(op::MAP_COMPOSITOR_TEXTURE,&mut texture),38,"17.6","gpu-apps: MAP_COMPOSITOR_TEXTURE")?;
    // The demo ops 0 to 5, 7 and 9 and the version 1 window ops 10 to 26 and 31. Arguments
    // that would do nothing were an op live: empty rectangles, a zero radius, a one-pixel line,
    // null descriptors and pointers, window id 0 and a zero timeout; op 0 clears with black.
    let ops: Vec<u32> = [0u32, 1, 2, 3, 4, 5, 7, 9].into_iter().chain(10..=26).chain([31]).collect();
    let live: Vec<String> = ops.iter().map(|&op| (op, fb_draw(op, 0, 0, 0, 0)))
        .filter(|(_, r)| *r != -38).map(|(op, r)| format!("op {op} {}", outcome(r))).collect();
    ensure(live.is_empty(), "17.6", format!("the retired fb_draw ops do not return ENOSYS: {}; the contract requires ENOSYS from each", live.join(", ")))
}

fn isolation_unknown_op() -> CaseResult {
    match window_version() {
        2 => {
            for unknown in [30, 0xFFFF, u32::MAX as u64] {
                // SAFETY: an unknown op is refused before any argument is read.
                let r = unsafe { raw::syscall3(SYS_WINDOW, unknown, 0, 0) as i64 };
                ensure(r == -22, "17.1", format!("window(op {unknown:#x}) {}; an unknown op fails with EINVAL", outcome(r)))?;
            }
            Ok(())
        }
        r => at("17.1", format!("window interface v2 is absent: window(VERSION) {}, so there is no op table to refuse unknown ops", outcome(r))),
    }
}

