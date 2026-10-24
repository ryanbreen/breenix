use super::kit::*;

pub fn clip_cross() -> CaseResult {
    clip_roundtrip(b"from the first program")
}
pub fn clip_utf8() -> CaseResult {
    clip_roundtrip("héλ🙂\n".as_bytes())
}
pub fn clip_large() -> CaseResult {
    clip_roundtrip(&vec![b'x'; 1024 * 1024])
}
pub fn clip_nul() -> CaseResult {
    clip_roundtrip(b"one\0two\0three")
}
pub fn clip_replace() -> CaseResult {
    let (_d, mut a, mut b) = twin()?;
    let (first, _) = clip_ok(&mut a, true, b"first")?;
    let (second, _) = clip_ok(&mut b, true, b"second")?;
    let (got, text) = clip_ok(&mut a, false, &[])?;
    check(
        second > first && got == second && text == b"second",
        "clipboard replacement did not advance serial and replace exact text",
    )?;
    a.await_event("prior clipboard owner notified of replacement", |e| {
        e.kind == kind::CLIPBOARD_LOST && e.serial == second
    })?;
    Ok(())
}
pub fn clip_owner_exit() -> CaseResult {
    let (_d, mut a, mut b) = twin()?;
    let (serial, _) = clip_ok(&mut a, true, b"retained")?;
    a.send(EXIT, 0, 0)?;
    until(1000, "clipboard owner exited", || {
        Ok(!listed()?.iter().any(|w| w.id == a.id))
    })?;
    let (got, text) = clip_ok(&mut b, false, &[])?;
    check(
        got == serial && text == b"retained",
        "clipboard text vanished or changed on owner exit",
    )
}

pub fn set_get() -> CaseResult {
    let (_d, mut c) = one()?;
    let (set, _) = clip_ok(&mut c, true, b"same-program")?;
    let (got, text) = clip_ok(&mut c, false, &[])?;
    check(
        set == got && text == b"same-program",
        "setter cannot read back its exact text and serial",
    )
}
pub fn empty() -> CaseResult {
    let (_d, mut c) = one()?;
    let (status, _, text) = c.clipboard(false, &[])?;
    check(
        status == 0 && text.is_empty(),
        "new claim clipboard is not empty or GET returned an error",
    )
}
pub fn too_large() -> CaseResult {
    let (_d, mut a, mut b) = twin()?;
    let (serial, _) = clip_ok(&mut a, true, b"kept")?;
    let (code, _, _) = b.clipboard(true, &vec![b'x'; 16 * 1024 * 1024 + 1])?;
    check(
        code == -7,
        "clipboard larger than 16 MiB did not return E2BIG",
    )?;
    let (got, text) = clip_ok(&mut b, false, &[])?;
    check(
        got == serial && text == b"kept",
        "refused oversized set changed clipboard",
    )
}
pub fn change_notice() -> CaseResult {
    let (_d, mut a, mut b) = twin()?;
    b.command(CLIP_SUBSCRIBE, 1, 0)?;
    b.clear()?;
    let (serial, _) = clip_ok(&mut a, true, b"notify")?;
    b.await_event("kernel clipboard change notice", |e| {
        e.kind == kind::CLIPBOARD_CHANGED && e.serial == serial
    })?;
    b.command(CLIP_SUBSCRIBE, 0, 0)?;
    b.clear()?;
    clip_ok(&mut a, true, b"unsubscribed")?;
    b.snapshot()?;
    check(
        !b.events.iter().any(|e| e.kind == kind::CLIPBOARD_CHANGED),
        "unsubscribed window still received change notice",
    )
}
pub fn concurrent_sets() -> CaseResult {
    let (_d, mut a, mut b) = twin()?;
    a.clipboard_begin(b"one")?;
    b.clipboard_begin(b"two")?;
    let (sa, ia, _) = a.clipboard_reply()?;
    let (sb, ib, _) = b.clipboard_reply()?;
    check(
        sa == 0 && sb == 0 && ia != ib,
        "concurrent setters failed or reused a serial",
    )?;
    let expected = if ia > ib { b"one" } else { b"two" };
    let serial = ia.max(ib);
    for c in [&mut a, &mut b] {
        let (got, text) = clip_ok(c, false, &[])?;
        check(
            got == serial && text == expected,
            "concurrent setter did not observe the later serial's text",
        )?;
    }
    Ok(())
}
pub fn mime_text_only() -> CaseResult {
    let (_d, mut c) = one()?;
    let (serial, _) = clip_ok(&mut c, true, b"plain")?;
    let (status, _, _) = c.clipboard_mime(true, b"not-plain", 2)?;
    check(
        status == -22,
        "unsupported clipboard MIME did not return EINVAL",
    )?;
    let (got, text) = clip_ok(&mut c, false, &[])?;
    check(
        got == serial && text == b"plain",
        "unsupported MIME changed clipboard",
    )
}
pub fn window_required() -> CaseResult {
    let (_d, _a) = one()?;
    let mut unregistered = Client::unregistered(99)?;
    let (code, _, _) = unregistered.clipboard(true, b"no desktop window")?;
    check(code == -1, "unregistered client set desktop clipboard")
}
pub fn unprivileged() -> CaseResult {
    let _d = Desktop::start()?;
    child_check(|| {
        if unsafe { libbreenix::syscall::raw::syscall1(libbreenix::syscall::nr::SETUID, 1000) }
            as i64
            != 0
        {
            return 1;
        }
        let result = (|| -> Result<bool, String> {
            let mut c = Client::start(88)?;
            let (a, serial, _) = c.clipboard(true, b"uid1000")?;
            let (b, got, text) = c.clipboard(false, &[])?;
            Ok(a == 0 && b == 0 && serial == got && text == b"uid1000")
        })();
        if result == Ok(true) {
            0
        } else {
            2
        }
    })
}

pub fn textfield_copy() -> CaseResult {
    let (mut d, mut a, mut b) = twin()?;
    clip_ok(&mut b, true, b"old clipboard")?;
    d.focus(&mut a)?;
    a.field("selected text α", true)?;
    a.clear()?;
    key(&mut a, "ctrl-c", 46)?;
    let (_, got) = clip_ok(&mut b, false, &[])?;
    check(
        got == "selected text α".as_bytes(),
        "Ctrl+C in libbui TextField did not copy its exact selection",
    )
}
pub fn textfield_paste() -> CaseResult {
    let (mut d, mut a, mut b) = twin()?;
    clip_ok(&mut b, true, "paste λ".as_bytes())?;
    d.focus(&mut a)?;
    a.field("kept ", false)?;
    a.clear()?;
    key(&mut a, "ctrl-v", 47)?;
    check(
        a.field_text()? == "kept paste λ".as_bytes(),
        "Ctrl+V in libbui TextField did not insert the clipboard text",
    )
}
