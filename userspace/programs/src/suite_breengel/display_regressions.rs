//! DISPLAY regressions exercised on the real ARM64 syscall/backend path.
use crate::*;
use libbreenix::window as win;
use win::abi::{self, op, Plain};

pub fn many_windows() -> CaseResult {
    let (mut compositor, _) = crate::window_cases::kit::kiosk()?;
    ensure(compositor.ask("auto 0")? == "ok", "24", "gpu-apps: kiosk auto 0 failed")?;
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
    let pid = breenix_suite::suite::spawn(&[(wr, wr)], breenix_suite::suite::Run::Call(&mut child))?;
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
        ensure(compositor.ask(&format!("place {} {} 0 {} 1", w.id, i, i))? == "ok",
            "18", "gpu-window: compositor could not place regression window")?;
    }
    let (serial, _) = crate::window_cases::composition::compose(&mut compositor)?;
    let record = compositor.ask(&format!("record {serial}"))?;
    let fields: Vec<_> = record.split_whitespace().collect();
    ensure(fields.len()==23 && fields[0]=="ok" && fields[1].parse::<u64>().ok()==Some(serial)
        && fields[3]=="18" && fields[4]=="0", "18",
        "gpu-display: composite record lost its serial, 18 entries or zero flags")?;
    // Record v2 still has sixteen entries; eighteen must fail without partial output.
    let modes=bgl::modes().map_err(|r|format!("gpu-display: MODES {}",outcome(r)))?;
    let mode=modes.modes[modes.current as usize];
    let mut pixels = vec![0xA5u8; mode.width as usize * mode.height as usize * 4];
    let mut observation = [0xA5u8; 1024];
    let mut rb = crate::window_cases::handoff::Readback::ZERO;
    rb.dst = pixels.as_mut_ptr() as u64;
    rb.dst_len = pixels.len() as u64;
    rb.record = observation.as_mut_ptr() as u64;
    rb.record_len = 1024;
    let result = unsafe {
        win::raw_call(
            op::SCANOUT_READBACK,
            (&mut rb as *mut crate::window_cases::handoff::Readback).cast(),
            core::mem::size_of_val(&rb),
        )
    };
    ensure(
        result == -5 && rb.reason == 11 && rb.serial == 0 && pixels.iter().all(|b| *b == 0xA5) && observation.iter().all(|b| *b == 0xA5),
        "20.1",
        "gpu-display: unrepresentable observation omitted reason 11 or partly copied outputs",
    )?;
    let _ = signal::kill(pid.raw() as i32, signal::SIGKILL);
    let mut status = 0;
    process::waitpid(pid.raw() as i32, &mut status, 0)?;
    // Destroy every caller window regardless of registry order.
    for w in &windows[..count] {
        let _ = win::destroy(w.id);
    }
    Ok(())
}
