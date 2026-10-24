use super::kit::*;

pub fn keyboard_completion() -> CaseResult {
    let mut stream = Stream::open()?;
    let (_d, mut c) = one()?;
    replay(1, 0, 0)?;
    check(
        stream
            .read(2)?
            .iter()
            .any(|e| e.kind == 1 && e.code == 30 && e.value == 1),
        "bound keyboard completion did not report A",
    )?;
    c.await_event("bound keyboard completion wakes bwm and delivers A", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 30
    })?;
    Ok(())
}
pub fn pointer_completion() -> CaseResult {
    let mut stream = Stream::open()?;
    let (_d, mut c) = one()?;
    {
        let w = c.info()?;
        replay(2, w.x + 40, w.y + 50)?;
    };
    check(
        stream.read(4)?.iter().any(|e| e.kind == 3),
        "bound pointer completion did not report absolute axes",
    )?;
    c.await_event("bound pointer completion wakes bwm", |e| {
        e.kind == kind::POINTER_MOVE
    })?;
    Ok(())
}
pub fn replay_faults() -> CaseResult {
    let mut stream = Stream::open()?;
    let (_d, mut c) = one()?;
    replay(1, 0, 0)?;
    c.await_event("positive keyboard completion before planted fault", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 30
    })?;
    stream.read(2)?;
    c.clear()?;
    replay(0x101, 0, 0)?; // same production completion, suppress its report once
    check(
        stream.read(1).is_err(),
        "report-suppression fault still published input",
    )?;
    c.snapshot()?;
    check(
        c.events.iter().all(|e| e.kind != kind::KEY_PRESS),
        "report-suppression fault still reached a client",
    )
}
