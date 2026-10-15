//! isolation: window authority and EFAULT safety (contract sections 17.4 to 17.6, 21, 22).

use crate::*;

pub(crate) const ISOLATION: &[Case] = &[
    case("other-register", "An unrelated process registering another's window fails with EPERM", isolation_other_register),
    case("other-commit", "An unrelated process committing another's window fails with EPERM", isolation_other_commit),
    case("other-position", "A non-compositor setting a window's position fails with EPERM", isolation_other_position),
    case("other-inject", "A non-compositor writing events into a window fails with EPERM", isolation_other_inject),
    case("other-read", "An unrelated non-root process capturing another's window fails with EPERM, and root succeeds", isolation_other_read),
    case("other-map", "An unrelated process mapping another's buffers fails with EPERM", isolation_other_map),
    case("other-resize", "An unrelated process resizing another's window fails with EPERM", isolation_other_resize),
    case("other-destroy", "An unrelated process destroying another's window fails with EPERM", isolation_other_destroy),
    case("other-tree", "An unrelated non-root process mapping another's tree page fails with EPERM", isolation_other_tree),
    case("texture-map", "A non-compositor mapping the compositor texture fails with EPERM", isolation_texture_map),
    case("composite-denied", "A non-compositor compositing fails with EPERM", isolation_composite_denied),
    case("claim-unprivileged", "A uid 1000 process claiming the compositor fails with EPERM, and root's claim succeeds", isolation_claim_unprivileged),
    case("claim-twice", "A second compositor claim fails with EBUSY while the first lives", isolation_claim_twice),
    case("claim-after-death", "The compositor role can be claimed again once its holder dies, with a higher epoch", isolation_claim_after_death),
    case("compositor-authorized", "The compositor's position, inject, capture and composite calls succeed on every window", isolation_compositor_authorized),
    case("pid-reuse", "A process that reuses a dead owner's PID number gains no authority over any window", isolation_pid_reuse),
    case("efault-create", "An unmapped output pointer for create fails with EFAULT and creates nothing", isolation_efault_create),
    case("efault-register", "An unmapped title pointer fails with EFAULT", isolation_efault_register),
    case("efault-list", "An unmapped list buffer fails with EFAULT", isolation_efault_list),
    case("efault-capture", "An unmapped capture destination fails with EFAULT", isolation_efault_capture),
    case("efault-events", "An unmapped event-read buffer fails with EFAULT and consumes no event", isolation_efault_events),
    case("efault-inject", "An unmapped event pointer given to the compositor's write fails with EFAULT", isolation_efault_inject),
    case("efault-composite", "An unmapped composite descriptor fails with EFAULT", isolation_efault_composite),
    case("efault-mouse", "An unmapped pointer-position buffer fails with EFAULT", isolation_efault_mouse),
    case("efault-commit", "An unmapped commit metadata pointer fails with EFAULT and commits nothing", isolation_efault_commit),
    case("efault-readback", "An unmapped readback destination fails with EFAULT", isolation_efault_readback),
    case("efault-claim", "An unmapped claim argument fails with EFAULT and claims nothing", isolation_efault_claim),
    case("efault-cross-page", "A buffer that runs from a mapped page into an unmapped one fails with EFAULT and has no side effect", isolation_efault_cross_page),
    case("efault-overflow", "A pointer whose range overflows the address space fails with EFAULT", isolation_efault_overflow),
    case("efault-readonly", "A read-only output buffer fails with EFAULT and is left unchanged", isolation_efault_readonly),
    case("cow-output", "Output written into a copy-on-write page after fork lands only in the caller's copy", isolation_cow_output),
    case("destroy-during-commit", "Destroying a window while another thread commits it ends with one outcome and no fault", isolation_destroy_during_commit),
    case("test-hooks-absent", "The wait-stress ops fail with ENOSYS in the production kernel", isolation_test_hooks_absent),
    case("retired-ops", "The retired demo ops fail with ENOSYS", isolation_retired_ops),
    case("unknown-op", "An unknown op fails with EINVAL", isolation_unknown_op),
    case("killable", "SIGKILL ends a process blocked in acquire or wait_event within 250 ms", isolation_killable),
    case("quiet-errors", "1,000 failing window calls on one thread add nothing to that thread's kernel-output counter or to the unattributed counter; with `emit-diagnostic(path)` armed, each writer path of section 12.1 (direct writers, nested wrappers, the logger, a notice written out by another thread, and `emergency_print` into the unattributed counter) adds exactly its bytes, once, to the right counter, and a second emitting thread moves only its own", isolation_quiet_errors),
    case("controls-latched", "In this suite boot the test controls are enabled because PID 1 is a suite binary, no op can clear or set the latch, and a child running `/sbin/init desktop` reports it is not PID 1 and makes no absence claim", isolation_controls_latched),
];

// ---------------------------------------------------------------------------------------
// isolation: window authority & EFAULT safety (contract sections 17.4 to 17.6, 21, 22)
// ---------------------------------------------------------------------------------------

fn isolation_other_register() -> CaseResult { needs_window("17.4", "EPERM for an unrelated REGISTER") }
fn isolation_other_commit() -> CaseResult { needs_window("17.4", "EPERM for an unrelated COMMIT") }
fn isolation_other_position() -> CaseResult { needs_window("17.4", "EPERM for a non-compositor SET_GEOMETRY") }
fn isolation_other_inject() -> CaseResult { needs_window("17.4", "EPERM for a non-compositor WRITE_EVENTS") }
fn isolation_other_read() -> CaseResult { needs_window("17.4", "EPERM for an unrelated non-root CAPTURE, and root's success") }
fn isolation_other_map() -> CaseResult { needs_window("17.4", "EPERM for an unrelated MAP_COMMITTED") }
fn isolation_other_resize() -> CaseResult { needs_window("17.4", "EPERM for an unrelated RESIZE") }
fn isolation_other_destroy() -> CaseResult { needs_window("17.4", "EPERM for an unrelated DESTROY") }
fn isolation_other_tree() -> CaseResult { needs_window("17.4", "EPERM for an unrelated non-root MAP_TREE") }
fn isolation_texture_map() -> CaseResult { needs_window("17.4", "EPERM for a non-compositor MAP_COMPOSITOR_TEXTURE") }
fn isolation_composite_denied() -> CaseResult { needs_window("17.4", "EPERM for a non-compositor COMPOSITE") }
fn isolation_claim_unprivileged() -> CaseResult { needs_window("17.4, 18", "EPERM for uid 1000's CLAIM_COMPOSITOR, and root's success") }
fn isolation_claim_twice() -> CaseResult { needs_window("17.4, 18", "EBUSY for a second live claim") }
fn isolation_claim_after_death() -> CaseResult { needs_window("18", "a new claim with a higher epoch after the holder dies") }
fn isolation_compositor_authorized() -> CaseResult { needs_window("17.4", "the compositor's SET_GEOMETRY, WRITE_EVENTS, CAPTURE and COMPOSITE") }
fn isolation_pid_reuse() -> CaseResult { needs_window("17.4", "no authority for a process reusing a dead owner's PID") }
fn isolation_efault_create() -> CaseResult { needs_window("17.5", "EFAULT for CREATE's output, creating nothing") }
fn isolation_efault_register() -> CaseResult { needs_window("17.5", "EFAULT for an unmapped title") }
fn isolation_efault_list() -> CaseResult { needs_window("17.5", "EFAULT for an unmapped LIST buffer") }
fn isolation_efault_capture() -> CaseResult { needs_window("17.5", "EFAULT for an unmapped CAPTURE destination") }
fn isolation_efault_events() -> CaseResult { needs_window("17.5", "EFAULT for an unmapped READ_EVENTS buffer, consuming no event") }
fn isolation_efault_inject() -> CaseResult { needs_window("17.5", "EFAULT for an unmapped WRITE_EVENTS pointer") }
fn isolation_efault_composite() -> CaseResult { needs_window("17.5", "EFAULT for an unmapped COMPOSITE argument") }
fn isolation_efault_mouse() -> CaseResult { needs_window("17.5", "EFAULT for an unmapped pointer-position buffer") }
fn isolation_efault_commit() -> CaseResult { needs_window("17.5", "EFAULT for unmapped commit metadata, committing nothing") }
fn isolation_efault_readback() -> CaseResult { needs_window("17.5", "EFAULT for an unmapped SCANOUT_READBACK destination") }
fn isolation_efault_claim() -> CaseResult { needs_window("17.5", "EFAULT for an unmapped CLAIM_COMPOSITOR argument, claiming nothing") }
fn isolation_efault_cross_page() -> CaseResult { needs_window("17.5", "EFAULT with no side effect for a buffer running into an unmapped page") }
fn isolation_efault_overflow() -> CaseResult { needs_window("17.5", "EFAULT for a range that overflows the address space") }
fn isolation_efault_readonly() -> CaseResult { needs_window("17.5", "EFAULT for a read-only output buffer, left unchanged") }
fn isolation_cow_output() -> CaseResult { needs_window("17.5", "output landing only in the caller's copy-on-write page") }
fn isolation_destroy_during_commit() -> CaseResult { needs_window("17.5", "one outcome and no fault for DESTROY racing COMMIT") }

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
    match process::fork()? {
        ForkResult::Child => {
            let _ = io::close(rd);
            let r = fb_draw(28, 0, 0, 0, 0);
            let _ = io::write(wr, &r.to_le_bytes());
            process::exit(0);
        }
        ForkResult::Parent(pid) => {
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
        }
    }
    let live: Vec<String> = seen.iter().filter(|(_, r)| *r != -38).map(|(op, r)| {
        if *r == i64::MIN { format!("op {op} blocked") } else { format!("op {op} {}", outcome(*r)) }
    }).collect();
    ensure(live.is_empty(), "17.6", format!("the wait-stress ops do not return ENOSYS in this production kernel: fb_draw {}; the contract requires ENOSYS from each", live.join(", ")))
}

fn isolation_retired_ops() -> CaseResult {
    // Arguments that draw nothing: empty rectangles, a zero radius, a one-pixel line, a null
    // descriptor for the VirGL ops; op 0 clears with black.
    let calls: [(u32, i32, i32, i32, i32); 8] = [(0, 0, 0, 0, 0), (1, 0, 0, 0, 0), (2, 0, 0, 0, 0), (3, 0, 0, 0, 0),
        (4, 0, 0, 0, 0), (5, 0, 0, 0, 0), (7, 0, 0, 0, 0), (9, 0, 0, 0, 0)];
    let live: Vec<String> = calls.iter().map(|&(op, a, b, c, d)| (op, fb_draw(op, a, b, c, d)))
        .filter(|(_, r)| *r != -38).map(|(op, r)| format!("op {op} {}", outcome(r))).collect();
    ensure(live.is_empty(), "17.6", format!("the retired fb_draw demo ops do not return ENOSYS: {}; the contract requires ENOSYS from each", live.join(", ")))
}

fn isolation_unknown_op() -> CaseResult {
    match window_version() {
        2 => {
            // SAFETY: an unknown op is refused before any argument is read.
            let r = unsafe { raw::syscall3(SYS_WINDOW, 0xFFFF, 0, 0) as i64 };
            ensure(r == -22, "17.1", format!("window(op 0xFFFF) {}; an unknown op fails with EINVAL", outcome(r)))
        }
        r => at("17.1", format!("window interface v2 is absent: window(VERSION) {}, so there is no op table to refuse unknown ops", outcome(r))),
    }
}

fn isolation_killable() -> CaseResult { needs_window("17.5", "SIGKILL ending a blocked ACQUIRE or READ_EVENTS within 250 ms") }

fn isolation_quiet_errors() -> CaseResult {
    let tid = process::gettid().map(|t| t.raw()).unwrap_or(0);
    let pid = process::getpid().map(|p| p.raw()).unwrap_or(0);
    let path = format!("/proc/{pid}/task/{tid}/kernel_output");
    if std::fs::metadata(&path).is_err() {
        return missing(&format!("per-thread kernel output accounting ({path})"), "22");
    }
    needs_window("17.5, 22", "no kernel output from 1,000 failing window calls, and the emit-diagnostic twins")
}

fn isolation_controls_latched() -> CaseResult {
    let state = test_control_state();
    ensure(state == 1, "21", format!(
        "test_control(STATE) {} in this suite boot, where PID 1 is /sbin/suite-breengel; the contract latches test controls on here, and STATE returns 1", outcome(state)))?;
    // SAFETY: SET_LATCH (op 4) takes no argument and must be refused before reading one.
    let set = unsafe { raw::syscall3(SYS_TEST_CONTROL, 4, 0, 0) as i64 };
    ensure(set == -22, "21", format!("test_control(SET_LATCH) {}; the contract has no op that sets or clears the latch, so it fails with EINVAL", outcome(set)))?;
    missing("init's child mode (/sbin/init desktop as a child reporting it is not PID 1)", "21")
}
