//! DISPLAY regressions exercised on the real ARM64 syscall/backend path.
use crate::*;
use libbreenix::window as win;
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
    // Record v2 still has sixteen entries; eighteen must fail without partial output.
    let modes=bgl::modes().map_err(|r|format!("gpu-display: MODES {}",outcome(r)))?;
    let mode=modes.modes[modes.current as usize];
    let mut pixels = vec![0xA5u8; mode.width as usize * mode.height as usize * 4];
    let mut observation = [0xA5u8; 1024];
    let mut rb = abi::Readback::ZERO;
    rb.dst = pixels.as_mut_ptr() as u64;
    rb.dst_len = pixels.len() as u64;
    rb.record = observation.as_mut_ptr() as u64;
    rb.record_len = 1024;
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
