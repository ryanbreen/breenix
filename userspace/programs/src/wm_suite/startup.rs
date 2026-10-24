use super::kit::*;

pub fn claims_display() -> CaseResult {
    let (mut d, c) = one()?;
    d.shown(&c)?;
    let result = window::claim_compositor();
    check(
        matches!(
            result,
            Err(libbreenix::error::Error::Os(
                libbreenix::errno::Errno::EBUSY
            ))
        ),
        &format!("bwm did not retain the compositor claim: {result:?}"),
    )
}
pub fn desktop_control() -> CaseResult {
    let d = Desktop::start()?;
    let s = Screen::read()?;
    check(
        s.serial() != 0 && s.pixel(d.width as i32 / 2, 1)? == 0xff14161e,
        "completed desktop control lacks the literal taskbar pixel",
    )
}

pub fn first_client() -> CaseResult {
    let (mut d, c) = one()?;
    content(&mut d, &c)
}
pub fn existing_windows() -> CaseResult {
    let c = Client::start(11)?;
    c.child.freeze()?;
    let mut d = Desktop::start()?;
    d.shown(&c)?.client(&c)
}
pub fn independent_clients() -> CaseResult {
    let (mut d, a, b) = twin()?;
    check(
        a.child.pid != b.child.pid
            && a.info()?.owner as i32 == a.child.pid
            && b.info()?.owner as i32 == b.child.pid,
        "clients are not independent window owners",
    )?;
    content(&mut d, &b)
}
pub fn supervisor_starts() -> CaseResult {
    let mut d = Desktop::launch(true)?;
    let c = Client::start(11)?;
    content(&mut d, &c)?;
    check(
        !children(d.wm.pid, "bwm")?.is_empty(),
        "init desktop did not retain a real bwm child",
    )
}

pub fn no_gpu_required() -> CaseResult {
    let (mut d, c) = one()?;
    content(&mut d, &c)?;
    check(
        d.wm.status()?.is_none(),
        "bwm exited despite a working GL device",
    )
}
pub fn epoch_ack() -> CaseResult {
    let mut clock = measure::Clock::new()?;
    let (_d, mut c) = one()?;
    for epoch in 1..=4 {
        clock.step(epoch, epoch * 16_000_000)?;
        c.command(STEP, epoch as u32, 0)?;
        measure::acknowledged(epoch)?;
        let s = status()?;
        check(
            s.epoch == epoch && s.claim_epoch != 0,
            "status does not identify acknowledged claim and epoch",
        )?;
    }
    let before = status()?.composites;
    clock.step(5, 80_000_000)?;
    c.command(STEP, 5, 0)?;
    measure::acknowledged(5)?;
    let after = status()?;
    check(
        after.presented == 0 && after.composites == before,
        "idle acknowledged epoch composed instead of none-needed",
    )
}
