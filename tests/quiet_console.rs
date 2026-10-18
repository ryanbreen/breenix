//! Startup ordering and sink boundaries for the local console.
use std::{fs, path::Path};

fn source(path: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(path)).unwrap()
}

fn body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source.find(signature).unwrap();
    let body = &source[start..];
    &body[..body.find("\n}\n").unwrap()]
}

#[test]
fn init_is_scheduled_only_after_keyboard_preparation_completes() {
    let main = source("kernel/src/main.rs");
    let launch = body(&main, "fn launch_x86_production_init(");
    let gate = launch.find("if kernel::drivers::xhci::wait_console_ready()").unwrap();
    assert!(launch[gate..].starts_with(
        "if kernel::drivers::xhci::wait_console_ready() {\n                task::scheduler::spawn(thread);"));
    let worker = source("kernel/src/drivers/xhci/keyboard.rs");
    let request = &worker[worker.find("if CONSOLE_START.swap").unwrap()..];
    let replug = request.find("self.replug_exercise();").unwrap();
    let arm = request.find("self.console.arm();").unwrap();
    let ready = request.find("CONSOLE_READY.complete(1);").unwrap();
    assert!(replug < arm && arm < ready);
    assert!(request[..replug].contains("if REPLUG.swap(false, Ordering::Relaxed)"));
    let xhci = source("kernel/src/drivers/xhci/mod.rs");
    let wait = body(&xhci, "pub fn wait_console_ready()");
    assert!(wait.contains("matches!(completed, Ok(true)) && !FAILED.load(Ordering::Acquire)"));
    assert!(wait.contains("CONSOLE_READY.wait_timeout_uninterruptible"));
}

#[test]
fn console_session_filters_at_record_admission_without_waiting_for_render() {
    let console = source("kernel/src/console/mod.rs");
    let log = body(&console, "pub fn log_record(");
    assert!(log.contains("SESSION.load(Ordering::Acquire) && level > log::Level::Warn"));
    assert!(!log.contains("QUIET.load"));
    assert!(log.find("SESSION.load").unwrap() < log.find("with_grid(").unwrap());
    let ownership = body(&console, "pub fn console_session(");
    assert!(ownership.contains("SESSION.store(held, Ordering::Release)"));
    let usb = source("kernel/src/drivers/xhci/keyboard.rs");
    assert!(usb.split_whitespace().collect::<String>().contains("log::info!(\"[usb]bootkeyboardattached:"));
    // Periodic diagnostics retain the scored kernel serial record and do not
    // enter either the screen sink or the TTY output/echo path.
    let census = source("kernel/src/task/dispatch_strand_census.rs");
    let snapshot = body(&census, "fn snapshot(deferred: bool)");
    assert!(snapshot.contains("crate::serial::enqueue_log(args)"));
    assert!(snapshot.contains("crate::serial::_log_print(args)"));
    for sink in ["log::", "console::", "tty::", "serial::_print(", "serial_print!"] {
        assert!(!snapshot.contains(sink), "census enters console sink {sink}");
    }
}

#[test]
fn disk_root_intent_is_consumed_before_registering_a_ram_block_device() {
    let ramdisk = source("kernel/src/block/ramdisk.rs");
    let init = body(&ramdisk, "pub unsafe fn init(");
    assert!(init.contains("if crate::boot::console_intent::capture(bytes) {\n        return Ok(());"));
    assert!(init.find("console_intent::capture(bytes)").unwrap() < init.find("RamDisk::new(bytes)").unwrap());
    let main = source("kernel/src/main.rs");
    let mode = body(&main, "fn x86_console_mode()");
    assert!(mode.contains("kernel::boot::console_intent::replug_or(replug)"));
}
