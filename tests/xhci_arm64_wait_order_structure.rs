//! #1920 / #482 -- an xHCI completion that arrives between publishing a request
//! (a TRB queued and its doorbell rung) and the worker waiting for it is not
//! lost on ARM64.
//!
//! The retired driver armed a transient completion IRQ for each synchronous
//! wait and cleared the SPI's pending bit while doing so; clearing after the
//! doorbell discarded a completion that had already fired. The clean-room
//! driver (`kernel/src/drivers/xhci/`) does not re-arm: the SPI stays enabled,
//! the interrupt handler publishes persistent work, and the worker consumes it.
//! The rules below pin that design in the source text, so a later change that
//! brings back a per-wait clear or mask, or a lost-wakeup window, fails here:
//!
//! 1. The SPI's pending state is cleared in exactly one place, the ARM64
//!    `enable_interrupt`, before the function's MSI or MSI-X is programmed, and
//!    `run` routes the interrupt before it sets Run/Stop. A completion cannot be
//!    pending before that point, so the clear cannot discard one.
//! 2. Outside that function the driver does not touch the GIC: the interrupt
//!    handler, the wait and the request paths leave the SPI enabled and its
//!    pending state alone.
//! 3. The interrupt handler publishes work with `WORK.signal()` and does not
//!    consume it; `WORK.take()` is called only by `wait_work`.
//! 4. `wait_work` publishes its thread before it first takes work, and the
//!    closure that blocks it checks `WORK.pending()` before blocking, so a
//!    signal that lands between the take and the block is seen.
//! 5. `Work::signal` sets pending before it reads the waiter, and `Work::take`
//!    clears pending with one atomic swap.
//!
//! Each rule reads code only: comments and string literals are masked out, and
//! `deliberately_broken_copies_redden_the_rules` mutates in-memory copies to
//! show that each rule can fail.

use std::fs;
use std::path::PathBuf;

const MOD: &str = "kernel/src/drivers/xhci/mod.rs";
const PLATFORM: &str = "kernel/src/drivers/xhci/platform.rs";
const SPEC: &str = "kernel/src/drivers/xhci/spec.rs";
const DRIVER_DIR: &str = "kernel/src/drivers/xhci";

fn repo_text(relative: &str) -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    fs::read_to_string(root.join(relative)).unwrap_or_else(|_| panic!("read {relative}"))
}

/// The source with comments, string and char literals replaced by spaces,
/// byte for byte, so offsets still line up.
fn code_only(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = bytes.to_vec();
    let mut i = 0;
    let blank = |out: &mut Vec<u8>, from: usize, to: usize| {
        for byte in &mut out[from..to] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    };
    while i < bytes.len() {
        if bytes[i..].starts_with(b"//") {
            let end = source[i..].find('\n').map_or(bytes.len(), |n| i + n);
            blank(&mut out, i, end);
            i = end;
        } else if bytes[i..].starts_with(b"/*") {
            let mut depth = 0;
            let start = i;
            while i < bytes.len() {
                if bytes[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                } else if bytes[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            blank(&mut out, start, i);
        } else if bytes[i] == b'"' {
            let start = i;
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' {
                i += if bytes[i] == b'\\' { 2 } else { 1 };
            }
            i += 1;
            blank(&mut out, start, i.min(bytes.len()));
        } else if bytes[i] == b'\''
            && i + 2 < bytes.len()
            && (bytes[i + 2] == b'\'' || (bytes[i + 1] == b'\\' && bytes[i + 3] == b'\''))
        {
            let end = if bytes[i + 1] == b'\\' { i + 4 } else { i + 3 };
            blank(&mut out, i, end);
            i = end;
        } else {
            i += 1;
        }
    }
    String::from_utf8(out).expect("masking keeps UTF-8 boundaries")
}

/// The braced body of each `fn name` in `code`, in source order.
fn bodies<'a>(code: &'a str, name: &str) -> Vec<&'a str> {
    let needle = format!("fn {name}");
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = code[from..].find(&needle) {
        let start = from + at;
        from = start + needle.len();
        let after = code[from..].chars().next();
        if !matches!(after, Some('(') | Some('<')) {
            continue;
        }
        let Some(open) = code[start..].find('{').map(|n| start + n) else {
            continue;
        };
        let mut depth = 0;
        for (offset, byte) in code[open..].bytes().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        found.push(&code[open..open + offset + 1]);
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    found
}

fn only_body<'a>(code: &'a str, name: &str) -> Result<&'a str, String> {
    match bodies(code, name).as_slice() {
        [body] => Ok(body),
        other => Err(format!("expected one fn {name}, found {}", other.len())),
    }
}

/// The ARM64 `enable_interrupt`: the definition after `#[cfg(target_arch = "aarch64")]`.
fn arm64_enable_interrupt(platform: &str) -> Result<&str, String> {
    let all = bodies(platform, "enable_interrupt");
    all.into_iter()
        .find(|body| body.contains("gic::"))
        .ok_or_else(|| String::from("no ARM64 enable_interrupt using the GIC"))
}

fn position(body: &str, needle: &str, what: &str) -> Result<usize, String> {
    body.find(needle).ok_or_else(|| format!("{what}: `{needle}` not found"))
}

/// Rule 1: one pending clear, in ARM64 routing setup, before the MSI is
/// programmed; `run` routes before it sets Run/Stop.
fn rule_clear_only_before_routing(module: &str, platform: &str) -> Result<(), String> {
    let enable = arm64_enable_interrupt(platform)?;
    let clear = position(enable, "gic::clear_spi_pending(", "enable_interrupt")?;
    for program in ["configure_msi(", "configure_msix_entry(", "enable_msix("] {
        let at = position(enable, program, "enable_interrupt")?;
        if at < clear {
            return Err(format!("enable_interrupt programs `{program}` before it clears the SPI's pending state"));
        }
    }
    let run = only_body(module, "run")?;
    let routed = position(run, "platform::enable_interrupt(", "run")?;
    let started = position(run, "wr(self.op, USBCMD, RUN | INTE)", "run")?;
    if started < routed {
        return Err(String::from("run sets Run/Stop before it routes the interrupt"));
    }
    Ok(())
}

/// Rule 2: the GIC is touched only by `enable_interrupt`, and the pending clear
/// appears once in the whole driver.
fn rule_gic_only_in_routing(files: &[(String, String)]) -> Result<(), String> {
    let mut clears = 0;
    for (name, code) in files {
        clears += code.matches("clear_spi_pending(").count();
        let mut rest = code.as_str();
        let allowed: Vec<&str> = if name.ends_with("platform.rs") {
            bodies(code, "enable_interrupt")
        } else {
            Vec::new()
        };
        while let Some(at) = rest.find("gic::") {
            let offset = code.len() - rest.len() + at;
            let inside = allowed.iter().any(|body| {
                let start = body.as_ptr() as usize - code.as_ptr() as usize;
                (start..start + body.len()).contains(&offset)
            });
            if !inside {
                let line = code[..offset].matches('\n').count() + 1;
                return Err(format!("{name}:{line} touches the GIC outside the ARM64 enable_interrupt"));
            }
            rest = &rest[at + 5..];
        }
    }
    if clears != 1 {
        return Err(format!("the SPI's pending state is cleared in {clears} places, not 1"));
    }
    Ok(())
}

/// Rule 3: the handler signals and does not take; only `wait_work` takes.
fn rule_handler_publishes_and_only_wait_consumes(module: &str, files: &[(String, String)]) -> Result<(), String> {
    let handler = only_body(module, "handle_interrupt")?;
    position(handler, "WORK.signal()", "handle_interrupt")?;
    if handler.contains(".take()") {
        return Err(String::from("handle_interrupt consumes the work it should publish"));
    }
    let wait = only_body(module, "wait_work")?;
    let mut takes = 0;
    for (_, code) in files {
        takes += code.matches("WORK.take()").count();
    }
    if takes != wait.matches("WORK.take()").count() {
        return Err(String::from("WORK.take() is called outside wait_work"));
    }
    Ok(())
}

/// Rule 4: publish before the first take; the blocking closure checks pending first.
fn rule_wait_publishes_then_rechecks(module: &str) -> Result<(), String> {
    let wait = only_body(module, "wait_work")?;
    let publish = position(wait, "WORK.publish(", "wait_work")?;
    let take = position(wait, "WORK.take()", "wait_work")?;
    if take < publish {
        return Err(String::from("wait_work takes work before it publishes its thread"));
    }
    let block = position(wait, "block_current_for_io_with_timeout(", "wait_work")?;
    let closure = wait[..block]
        .rfind("with_current(")
        .ok_or("wait_work blocks outside a scheduler closure")?;
    let window = &wait[closure..block];
    if !window.contains("WORK.pending()") {
        return Err(String::from("wait_work blocks without checking WORK.pending() in the same closure"));
    }
    Ok(())
}

/// Rule 5: signal sets pending then reads the waiter; take is one swap.
fn rule_work_primitive(spec: &str) -> Result<(), String> {
    let signal = only_body(spec, "signal")?;
    let store = position(signal, "self.pending.store(true", "Work::signal")?;
    let read = position(signal, "self.waiter.load(", "Work::signal")?;
    if read < store {
        return Err(String::from("Work::signal reads the waiter before it sets pending"));
    }
    let take = only_body(spec, "take")?;
    position(take, "self.pending.swap(false", "Work::take")?;
    Ok(())
}

fn driver_files() -> Vec<(String, String)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(DRIVER_DIR);
    let mut names: Vec<String> = fs::read_dir(&root)
        .expect("read the xHCI driver directory")
        .map(|entry| entry.expect("directory entry").file_name().into_string().expect("UTF-8 name"))
        .filter(|name| name.ends_with(".rs"))
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|name| {
            let path = format!("{DRIVER_DIR}/{name}");
            let code = code_only(&repo_text(&path));
            (path, code)
        })
        .collect()
}

#[test]
fn the_pending_clear_precedes_msi_programming_and_run() {
    rule_clear_only_before_routing(&code_only(&repo_text(MOD)), &code_only(&repo_text(PLATFORM))).unwrap();
}

#[test]
fn only_routing_setup_touches_the_gic() {
    rule_gic_only_in_routing(&driver_files()).unwrap();
}

#[test]
fn the_handler_publishes_and_only_the_wait_consumes() {
    rule_handler_publishes_and_only_wait_consumes(&code_only(&repo_text(MOD)), &driver_files()).unwrap();
}

#[test]
fn the_wait_publishes_its_thread_and_rechecks_before_blocking() {
    rule_wait_publishes_then_rechecks(&code_only(&repo_text(MOD))).unwrap();
}

#[test]
fn the_work_flag_is_set_before_the_waiter_is_read() {
    rule_work_primitive(&code_only(&repo_text(SPEC))).unwrap();
}

fn replace_once(source: &str, from: &str, to: &str) -> String {
    assert_eq!(source.matches(from).count(), 1, "mutation anchor `{from}` must occur once");
    source.replacen(from, to, 1)
}

#[test]
fn deliberately_broken_copies_redden_the_rules() {
    let module = repo_text(MOD);
    let platform = repo_text(PLATFORM);
    let spec = repo_text(SPEC);
    let files = driver_files();
    let with = |path: &str, text: &str| -> Vec<(String, String)> {
        files
            .iter()
            .map(|(name, code)| {
                if name == path {
                    (name.clone(), code_only(text))
                } else {
                    (name.clone(), code.clone())
                }
            })
            .collect()
    };

    // 1: the pending clear moved after MSI programming.
    let late_clear = replace_once(&platform, "    gic::clear_spi_pending(irq);\n", "");
    let late_clear = replace_once(
        &late_clear,
        "        device.configure_msi(cap, destination as u32, irq as u16);\n",
        "        device.configure_msi(cap, destination as u32, irq as u16);\n        gic::clear_spi_pending(irq);\n",
    );
    assert!(rule_clear_only_before_routing(&code_only(&module), &code_only(&late_clear)).is_err());

    // 1: Run/Stop set before the interrupt is routed.
    let early_run = replace_once(
        &module,
        "        let vector = platform::enable_interrupt(&self.pci, handle_interrupt)?;\n",
        "        wr(self.op, USBCMD, RUN | INTE);\n        let vector = platform::enable_interrupt(&self.pci, handle_interrupt)?;\n",
    );
    assert!(rule_clear_only_before_routing(&code_only(&early_run), &code_only(&platform)).is_err());

    // 2: a per-wait clear in the command path, the old driver's defect.
    let rearm = replace_once(
        &module,
        "        self.ring_doorbell(0, 0);\n        self.event(&[pointer], 33)\n",
        "        self.ring_doorbell(0, 0);\n        crate::arch_impl::aarch64::gic::clear_spi_pending(platform::get_irq().unwrap_or(0));\n        self.event(&[pointer], 33)\n",
    );
    assert!(rule_gic_only_in_routing(&with(MOD, &rearm)).is_err());

    // 2: the handler masks the SPI.
    let masked = replace_once(
        &module,
        "    let tid = WORK.signal();\n",
        "    crate::arch_impl::aarch64::gic::disable_spi(platform::get_irq().unwrap_or(0));\n    let tid = WORK.signal();\n",
    );
    assert!(rule_gic_only_in_routing(&with(MOD, &masked)).is_err());

    // 3: the handler consumes the work it publishes.
    let consumed = replace_once(&module, "    let tid = WORK.signal();\n", "    let tid = WORK.signal();\n    let _ = WORK.take();\n");
    assert!(rule_handler_publishes_and_only_wait_consumes(&code_only(&consumed), &with(MOD, &consumed)).is_err());

    // 3: the handler stops publishing.
    let silent = replace_once(&module, "    let tid = WORK.signal();\n", "    let tid = WORK.waiter();\n");
    assert!(rule_handler_publishes_and_only_wait_consumes(&code_only(&silent), &with(MOD, &silent)).is_err());

    // 4: the wait blocks without rechecking pending work.
    let unchecked = replace_once(&module, "                if WORK.pending() || requested() {\n", "                if requested() {\n");
    assert!(rule_wait_publishes_then_rechecks(&code_only(&unchecked)).is_err());

    // 4: a comment claiming the recheck does not count.
    let commented = replace_once(&module, "                if WORK.pending() || requested() {\n", "                // WORK.pending() checked\n                if requested() {\n");
    assert!(rule_wait_publishes_then_rechecks(&code_only(&commented)).is_err());

    // 5: signal reads the waiter before setting pending.
    let reordered = replace_once(
        &spec,
        "        self.pending.store(true, Ordering::SeqCst);\n        self.waiter.load(Ordering::SeqCst)\n",
        "        let waiter = self.waiter.load(Ordering::SeqCst);\n        self.pending.store(true, Ordering::SeqCst);\n        waiter\n",
    );
    assert!(rule_work_primitive(&code_only(&reordered)).is_err());
}

#[test]
fn masking_hides_comments_and_strings_only() {
    let code = code_only("let a = 1; // gic::x\nlet b = \"gic::y\"; /* gic::z */ let c = 'g';\nlet d = gic::w;");
    assert!(!code.contains("gic::x") && !code.contains("gic::y") && !code.contains("gic::z"));
    assert!(code.contains("let d = gic::w;") && code.contains("let a = 1;"));
}
