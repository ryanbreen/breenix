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
        e.kind == kind::CLIPBOARD_LOST
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
    let _d = Desktop::start()?;
    let mut c = breengel::Window::new(b"clipboard API", 32, 32)?;
    c.clipboard_subscribe(true)?;
    let set = c.clipboard_set("same-program\0λ")?;
    let got = c.clipboard_get()?;
    check(
        set == got.serial && got.text == "same-program\0λ",
        "setter cannot read back its exact text and serial",
    )?;
    let mut notified = false;
    c.dispatch(|e| {
        if matches!(e.event, breengel::Event::ClipboardChanged) {
            notified = e.serial != 0;
        }
        breengel::Handled::NoChange
    })?;
    check(notified, "Breengel did not decode the kernel clipboard notice")?;
    let replacement = c.clipboard_set("replacement")?;
    let mut dst = [0xa5; 32];
    let mut stale = abi::ClipboardGet::ZERO;
    stale.id = c.id();
    stale.dst = dst.as_mut_ptr() as u64;
    stale.capacity = dst.len() as u64;
    stale.serial = set;
    stale.mime = abi::CLIPBOARD_TEXT_UTF8;
    let code = unsafe {
        window::raw_call(abi::op::CLIPBOARD_GET, (&mut stale as *mut abi::ClipboardGet).cast(),
            core::mem::size_of_val(&stale))
    };
    check(code == -11 && dst == [0xa5; 32], "stale clipboard copy did not return EAGAIN without a partial copy")?;
    let invalid = [0xff];
    let mut bad = abi::ClipboardSet::ZERO;
    bad.id = c.id();
    bad.ptr = invalid.as_ptr() as u64;
    bad.len = 1;
    bad.mime = abi::CLIPBOARD_TEXT_UTF8;
    let code = unsafe {
        window::raw_call(abi::op::CLIPBOARD_SET, (&mut bad as *mut abi::ClipboardSet).cast(),
            core::mem::size_of_val(&bad))
    };
    check(code == -22, "invalid clipboard UTF-8 did not return EINVAL")?;
    let got = c.clipboard_get()?;
    check(got.serial == replacement && got.text == "replacement", "invalid UTF-8 changed clipboard")?;
    let mut original = [0u64; 2];
    let resource_limit = |new: u64, old: u64| unsafe {
        libbreenix::syscall::raw::syscall4(libbreenix::syscall::nr::PRLIMIT64, 0, 9, new, old) as i64
    };
    check(resource_limit(0, original.as_mut_ptr() as u64) == 0, "read clipboard setter RLIMIT_AS")?;
    let restricted = [0, original[1]];
    check(resource_limit(restricted.as_ptr() as u64, 0) == 0, "restrict clipboard setter RLIMIT_AS")?;
    let denied = c.clipboard_set("must be charged");
    let restored = resource_limit(original.as_ptr() as u64, 0);
    check(restored == 0, "restore clipboard setter RLIMIT_AS")?;
    check(matches!(denied, Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::ENOMEM))),
        "clipboard storage bypassed setter RLIMIT_AS")?;
    let got = c.clipboard_get()?;
    check(got.serial == replacement && got.text == "replacement", "ENOMEM changed clipboard")?;
    // VmSize reports whole KiB. Keep the baseline clipboard charge KiB-aligned
    // so AddressLimit measures it exactly, with less than a page released by SET.
    c.clipboard_set(&"x".repeat(1024))?;
    let payload = [b'x'; 8192];
    let limit = AddressLimit::new(libbreenix::process::getpid()?.raw() as i32, payload.len() as u64)?;
    c.clipboard_set(core::str::from_utf8(&payload).unwrap())?;
    let page = || libbreenix::memory::mmap(core::ptr::null_mut(), 4096,
        libbreenix::memory::PROT_READ | libbreenix::memory::PROT_WRITE,
        libbreenix::memory::MAP_PRIVATE | libbreenix::memory::MAP_ANONYMOUS, -1, 0);
    let denied = page();
    if let Ok(ptr) = denied {
        libbreenix::memory::munmap(ptr, 4096)?;
        return Err("retained clipboard charge did not exclude a page mmap".into());
    }
    check(matches!(denied, Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::ENOMEM))),
        "retained clipboard charge did not return ENOMEM for mmap")?;
    c.clipboard_set("x")?;
    let ptr = page()?;
    libbreenix::memory::munmap(ptr, 4096)?;
    drop(limit);
    let got = c.clipboard_get()?;
    check(got.text == "x", "short replacement was not retained")?;
    c.clipboard_subscribe(false)?;
    Ok(())
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
        e.kind == kind::CLIPBOARD_CHANGED
    })?;
    let (notified_serial, notified_text) = clip_ok(&mut b, false, &[])?;
    check(
        notified_serial == serial && notified_text == b"notify",
        "clipboard notice did not identify the changed text through GET",
    )?;
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
    // Six simultaneous maximum-size SETs would put 96 MiB of payload on
    // QEMU's 64 MiB kernel heap. Synchronize entry after registering windows.
    let mut payload = vec![b'x'; abi::CLIPBOARD_MAX_BYTES as usize];
    payload[4095..4099].copy_from_slice("🙂".as_bytes());
    let payload = std::sync::Arc::new(String::from_utf8(payload).unwrap());
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(6));
    let mut setters = Vec::new();
    for _ in 0..6 {
        let payload = payload.clone();
        let barrier = barrier.clone();
        setters.push(std::thread::spawn(move || -> Result<u64, String> {
            let window = breengel::Window::new_without_fonts(b"maximum clipboard", 1, 1);
            barrier.wait();
            window.map_err(|e| e.to_string())?.clipboard_set(&payload).map_err(|e| e.to_string())
        }));
    }
    let results: Vec<_> = setters.into_iter().map(|t| t.join()).collect();
    let mut serials = Vec::new();
    for result in results {
        serials.push(result.map_err(|_| "maximum clipboard setter panicked")??);
    }
    serials.sort_unstable();
    check(serials.windows(2).all(|s| s[0] < s[1]),
        "maximum-size concurrent SETs reused a serial")?;
    // Read the maximum payload directly; the client's pipe protocol otherwise
    // asks read(2) for a contiguous 16 MiB kernel bounce buffer.
    let reader = breengel::Window::new_without_fonts(b"maximum clipboard reader", 1, 1)?;
    let got = reader.clipboard_get()?;
    check(got.serial == serials[5] && got.text == *payload,
        "maximum-size concurrent SET lost exact text or final serial")?;
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
    // W15: an unregistered window is ENOENT and cannot change the clipboard.
    let (_d, mut a) = one()?;
    let (serial, _) = clip_ok(&mut a, true, b"registered owner's text")?;
    let mut unregistered = Client::unregistered(99)?;
    let (code, _, _) = unregistered.clipboard(true, b"no desktop window")?;
    check(code == -2, "unregistered clipboard SET did not return ENOENT")?;
    let (got, text) = clip_ok(&mut a, false, &[])?;
    check(
        got == serial && text == b"registered owner's text",
        "unregistered clipboard SET changed text or serial",
    )
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
