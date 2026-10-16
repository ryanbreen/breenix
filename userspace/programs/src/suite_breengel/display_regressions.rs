//! DISPLAY regressions exercised on the real ARM64 syscall/backend path.
use crate::*;
use libbreenix::{graphics, window as win};
use win::abi::{self, op, Plain};

pub fn many_windows() -> CaseResult {
    win::claim_compositor()?;
    let (rd, wr) = io::pipe()?;
    let make = || -> Result<(), String> {
        for _ in 0..9 {
            let w = win::create(1, 1, 2, 1).map_err(|e| e.to_string())?;
            win::register(w.id, b"observation-capacity").map_err(|e| e.to_string())?;
        }
        Ok(())
    };
    make()?;
    let mut child = || {
        if make().is_err() {
            return 1;
        }
        let _ = io::write(wr, b"ready");
        let _ = time::sleep_ms(30_000);
        0
    };
    let pid = libbreenix::suite::spawn(&[(wr, wr)], libbreenix::suite::Run::Call(&mut child))?;
    let _ = io::close(wr);
    let mut ready = [0u8; 5];
    ensure(
        io::read(rd, &mut ready)? == 5 && &ready == b"ready",
        "18",
        "second process did not create its nine windows",
    )?;
    let _ = io::close(rd);
    let mut windows = [abi::WinInfo::ZERO; 32];
    let count = win::list(&mut windows)?;
    ensure(
        count == 18,
        "17.2",
        "the per-process window limit was applied globally",
    )?;
    for (i, w) in windows[..count].iter().enumerate() {
        win::call(
            op::SET_GEOMETRY,
            &mut abi::Geometry {
                id: w.id,
                x: i as i32,
                y: 0,
                z: i as u32,
                flags: abi::geometry_flags::VISIBLE,
            },
        )?;
    }
    let mut composite = abi::Composite::ZERO;
    win::call(op::COMPOSITE, &mut composite)?;
    let mut record = vec![0u64; 3 + 4 * 18];
    let mut query = abi::RecordQuery {
        serial: composite.serial,
        ptr: record.as_mut_ptr() as u64,
        len: (record.len() * 8) as u32,
        reserved: 0,
    };
    win::call(op::COMPOSITE_RECORD, &mut query)?;
    let header =
        unsafe { core::ptr::read_unaligned(record.as_ptr().cast::<abi::CompositeRecordHeader>()) };
    ensure(
        header.count == 18 && header.flags == 0,
        "18",
        "composite did not record all 18 visible windows",
    )?;
    // The fixed 512-byte readback ABI cannot carry 18 entries. Refuse without writing.
    let mut pixels = vec![0xA5u8; 1280 * 800 * 4];
    let mut observation = [0xA5u8; 512];
    let mut rb = abi::Readback::ZERO;
    rb.dst = pixels.as_mut_ptr() as u64;
    rb.dst_len = pixels.len() as u64;
    rb.record = observation.as_mut_ptr() as u64;
    rb.record_len = 512;
    let result = unsafe {
        win::raw_call(
            op::SCANOUT_READBACK,
            (&mut rb as *mut abi::Readback).cast(),
            core::mem::size_of_val(&rb),
        )
    };
    ensure(
        result == -5 && pixels.iter().all(|b| *b == 0xA5) && observation.iter().all(|b| *b == 0xA5),
        "20.1",
        "unrepresentable observation was accepted or partly copied",
    )?;
    let _ = signal::kill(pid.raw() as i32, signal::SIGKILL);
    let mut status = 0;
    process::waitpid(pid.raw() as i32, &mut status, 0)?;
    // Destroy every caller window regardless of registry order.
    for w in &windows[..count] {
        let _ = win::destroy(w.id);
    }
    unsafe {
        win::raw_call(op::RELEASE_COMPOSITOR, core::ptr::null_mut(), 0);
    }
    Ok(())
}

#[repr(C)]
struct Flush {
    op: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    color: u32,
}
fn flush(x: i32, y: i32, w: i32, h: i32) -> i64 {
    let cmd = Flush {
        op: 6,
        x,
        y,
        w,
        h,
        color: 0,
    };
    unsafe { raw::syscall1(SYS_FB_DRAW, &cmd as *const _ as u64) as i64 }
}
fn nested_take(depth: u32) -> i32 {
    let ret = unsafe { raw::syscall0(SYS_TAKE_OVER_DISPLAY) as i64 };
    if depth == 4 {
        return if ret == -16 { 0 } else { 1 };
    }
    if ret != 0 {
        return 2;
    }
    let mut child = || nested_take(depth + 1);
    let Ok(pid) = libbreenix::suite::spawn(&[], libbreenix::suite::Run::Call(&mut child)) else {
        return 3;
    };
    let mut status = 0;
    if process::waitpid(pid.raw() as i32, &mut status, 0).is_err() {
        return 4;
    }
    if !process::wifexited(status) || process::wexitstatus(status) != 0 {
        return 5;
    }
    if unsafe { raw::syscall0(SYS_GIVE_BACK_DISPLAY) as i64 } != 0 {
        return 6;
    }
    0
}
pub fn ownership_and_flush() -> CaseResult {
    // The suite case already occupies one stack entry; three more fit, the next is EBUSY.
    let mut child = || nested_take(1);
    let pid = libbreenix::suite::spawn(&[], libbreenix::suite::Run::Call(&mut child))?;
    let mut status = 0;
    process::waitpid(pid.raw() as i32, &mut status, 0)?;
    ensure(
        process::wifexited(status) && process::wexitstatus(status) == 0,
        "18",
        "display ownership nesting did not stop at its bounded capacity",
    )?;
    let mut child = || {
        if unsafe { raw::syscall1(146, 1000) as i64 } != 0 {
            return 1;
        }
        if unsafe { raw::syscall0(SYS_TAKE_OVER_DISPLAY) as i64 } != -1 {
            return 2;
        }
        0
    };
    let pid = libbreenix::suite::spawn(&[], libbreenix::suite::Run::Call(&mut child))?;
    process::waitpid(pid.raw() as i32, &mut status, 0)?;
    ensure(
        process::wifexited(status) && process::wexitstatus(status) == 0,
        "18",
        "unprivileged takeover was not refused",
    )?;
    let mut pane_child = || -> i32 {
        let result = (|| -> CaseResult {
            let (ptr, width) = graphics::fb_mmap_pane()?;
            ensure(
                width == 640,
                "18",
                "nonowner did not receive the half-screen pane",
            )?;
            let before = crate::window_cases::composition::screen()?.0;
            unsafe {
                core::ptr::write_bytes(ptr, 0x67, width as usize * 800 * 4);
            }
            ensure(
                flush(630, 0, 100, 2) == 0,
                "18",
                "nonowner pane FLUSH was refused",
            )?;
            let after = crate::window_cases::composition::screen()?.0;
            for y in 0..800 {
                for x in 0..1280 {
                    let i = (y * 1280 + x) * 4;
                    if y < 2 && (630..640).contains(&x) {
                        ensure(
                            after[i..i + 4] == [0x67, 0x67, 0x67, 255],
                            "18",
                            "clipped pane pixels were not flushed",
                        )?;
                    } else {
                        ensure(
                            after[i..i + 4] == before[i..i + 4],
                            "18",
                            "FLUSH escaped the nonowner pane or rectangle",
                        )?;
                    }
                }
            }
            Ok(())
        })();
        i32::from(result.is_err())
    };
    let pid = libbreenix::suite::spawn(&[], libbreenix::suite::Run::Call(&mut pane_child))?;
    process::waitpid(pid.raw() as i32, &mut status, 0)?;
    ensure(
        process::wifexited(status) && process::wexitstatus(status) == 0,
        "18",
        "nonowner FLUSH failed its clipped pane regression",
    )?;
    // Make the second row inaccessible: failure must not even change the first row.
    let (ptr, width) = graphics::fb_mmap_pane()?;
    let before = crate::window_cases::composition::screen()?.0;
    unsafe {
        core::ptr::write_bytes(ptr, 0x39, 5120);
    }
    libbreenix::memory::mprotect(
        unsafe { ptr.add(8192) },
        4096,
        libbreenix::memory::PROT_NONE,
    )?;
    let failed = flush(0, 0, width as i32, 3);
    libbreenix::memory::mprotect(
        unsafe { ptr.add(8192) },
        4096,
        libbreenix::memory::PROT_READ | libbreenix::memory::PROT_WRITE,
    )?;
    ensure(
        failed == -14,
        "17.5",
        "FLUSH across an unreadable page did not return EFAULT",
    )?;
    let after = crate::window_cases::composition::screen()?.0;
    ensure(before == after, "17.5", "EFAULT FLUSH changed scanout")?;
    // A forced cursor publish reads the retained backing without replacing those pixels.
    let fired = libbreenix::test_control::fired(abi::tc_point::MOVE_POINTER)?;
    libbreenix::test_control::arm(abi::tc_point::MOVE_POINTER, 0, 0)?;
    let started = now_ms();
    while libbreenix::test_control::fired(abi::tc_point::MOVE_POINTER)? == fired {
        ensure(
            now_ms() - started < 500,
            "21",
            "cursor did not publish the retained backing",
        )?;
        time::sleep_ms(5)?;
    }
    time::sleep_ms(20)?;
    let after = crate::window_cases::composition::screen()?.0;
    ensure(
        before[17 * 4..18 * 4] == after[17 * 4..18 * 4],
        "17.5",
        "EFAULT left a partial backing change beneath the cursor",
    )?;
    // A successful disjoint flush also restores the overlay.
    ensure(
        flush(0, 4, 1, 1) == 0,
        "18",
        "valid FLUSH after EFAULT failed",
    )?;
    let after = crate::window_cases::composition::screen()?.0;
    ensure(
        before[..5120 * 3] == after[..5120 * 3],
        "17.5",
        "failed FLUSH partly changed the backing",
    )?;
    Ok(())
}
