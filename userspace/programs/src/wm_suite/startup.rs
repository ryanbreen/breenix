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

pub fn empty_desktop() -> CaseResult {
    let _clock = measure::Clock::new()?;
    let _config = FileOverride::new(
        "/etc/bwm.conf",
        b"font=/usr/share/fonts/DejaVuSansMono.ttf\nclock=frozen\nfps=off\n",
    )?;
    let d = Desktop::start()?;
    measure::acknowledged(0)?;
    d.pointer(d.width as i32 - 1, d.height as i32 - 1, 0, 0)?;
    time::sleep_ms(150)?;
    super::scene::empty_desktop(&Screen::read()?, &d)
}
pub fn config_test() -> CaseResult {
    let _clock = measure::Clock::new()?;
    let _config = FileOverride::new(
        "/etc/bwm.conf",
        b"font=/usr/share/fonts/DejaVuSansMono.ttf\nclock=frozen\nfps=off\n",
    )?;
    let mut d = Desktop::start()?;
    let state = status()?;
    check(
        state.flags & 1 != 0,
        "bwm did not activate its test configuration",
    )?;
    check(
        d.output().contains("DejaVuSansMono.ttf") && !d.output().contains("[bwm-fps]"),
        "pinned font or fps=off setting was ignored",
    )?;
    let s = Screen::read()?;
    super::scene::clock(&s, &d, "00:00:00 UTC")?;
    let mut c = Client::start(11)?;
    c.title(b"Font")?;
    content(&mut d, &c)?;
    check(focused(&c)?, "configured font client is not focused")?;
    let s = d.shown(&c)?;
    let w = c.info()?;
    super::scene::ttf_text(
        &s,
        b"Font",
        w.x + 6 * d.scale,
        w.y - 26 * d.scale,
        0xffffffff,
        0xff2864dc,
        d.scale,
        "/usr/share/fonts/DejaVuSansMono.ttf",
    )
}
pub fn config_invalid() -> CaseResult {
    let _clock = measure::Clock::new()?;
    let _config = FileOverride::new("/etc/bwm.conf", b"font=\nclock=invalid\nfps=invalid\n")?;
    let mut d = Desktop::start()?;
    check(
        status()?.flags & 1 == 0,
        "malformed configuration was accepted",
    )?;
    let out = d.output();
    check(
        out.contains("bwm.conf") && out.to_ascii_lowercase().contains("error"),
        "malformed configuration was not reported",
    )?;
    // The scanned rectangle excludes the rightmost 80 pixels, including this cursor.
    parked(&d)?;
    let s = Screen::read()?;
    let mut visible = 0;
    for y in 28 * d.scale..d.height as i32 - 36 * d.scale {
        for x in 0..d.width as i32 - 80 {
            visible += usize::from(s.pixel(x, y)? != background(x, y, d.height as i32, d.scale));
        }
    }
    check(
        visible > 0,
        "malformed configuration error is absent from the desktop",
    )?;
    let mut c = Client::start(11)?;
    content(&mut d, &c)?;
    let w = c.info()?;
    check(
        w.x < 372 * d.scale && w.y < 80 * d.scale,
        "regression client does not overlap the error banner",
    )?;
    c.child.stop()?;
    until(1500, "error banner restored after overlapping client closes", || {
        if listed()?.iter().any(|w| w.id == c.id) {
            return Ok(false);
        }
        let after = Screen::read()?;
        for y in 40 * d.scale..80 * d.scale {
            for x in 12 * d.scale..372 * d.scale {
                if after.pixel(x, y)? != s.pixel(x, y)? {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    })?;
    Ok(())
}
pub fn fonts_loaded() -> CaseResult {
    let _font = FileOverride::new(
        "/etc/bwm.conf",
        b"font=/usr/share/fonts/DejaVuSansMono.ttf\nclock=frozen\nfps=off\n",
    )?;
    let _clock = measure::Clock::new()?;
    let (mut d, mut c) = one()?;
    c.title(b"Font")?;
    content(&mut d, &c)?;
    parked(&d)?;
    let s = d.shown(&c)?;
    let w = c.info()?;
    super::scene::ttf_text(
        &s,
        b"Font",
        w.x + 6 * d.scale,
        w.y - 26 * d.scale,
        0xffffffff,
        0xff2864dc,
        d.scale,
        "/usr/share/fonts/DejaVuSansMono.ttf",
    )
}
pub fn hotkeys_loaded() -> CaseResult {
    super::focus::hotkeys_config()
}
pub fn composited_marker() -> CaseResult {
    let mut d = Desktop::start()?;
    check(
        !d.output().contains("[bwm] composited client frame:"),
        "bwm announced a client composite on an empty desktop",
    )?;
    let mut c = Client::unregistered(11)?;
    check(
        !d.output().contains("[bwm] composited client frame:"),
        "bwm announced an unregistered client's frame",
    )?;
    use libbreenix::test_control as tc;
    let point = abi::tc_point::COMPOSITE_STALL;
    let before = tc::fired(point)?;
    tc::arm(point, c.id, 0)?;
    c.command(REGISTER, 0, 0)?;
    until(
        1000,
        "first client composite paused before backend submission",
        || {
            if d.output().contains("[bwm] composited client frame:") {
                return Err("client marker appeared before backend composite".into());
            }
            Ok(tc::fired(point).map_err(|e| e.to_string())? == before + 1)
        },
    )?;
    check(
        !Screen::read()?.contains(c.id, c.frame_commit),
        "paused first composite already appeared in completed scanout",
    )?;
    check(
        !d.output().contains("[bwm] composited client frame:"),
        "client marker appeared while its first composite was paused",
    )?;
    tc::arm(abi::tc_point::RELEASE_STALL, 0, 0)?;
    d.client_ready(&c)?;
    d.shown(&c)?.client(&c)
}
