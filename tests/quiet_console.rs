//! Exercise console startup, record admission and census emission.
extern crate alloc;
extern crate self as log;
#[macro_export]
macro_rules! error { ($($arg:tt)*) => { eprintln!($($arg)*) }; }
fn arch_interrupts_enabled() -> bool {
    true
}
fn arch_halt_with_interrupts() {}
mod time {
    pub fn get_ticks() -> u64 {
        0
    }
    pub fn get_monotonic_time_ns() -> (u64, u64) {
        (2, 0)
    }
}
mod serial {
    pub static OUTPUT: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
    pub fn _log_print(args: std::fmt::Arguments<'_>) {
        OUTPUT.lock().unwrap().push_str(&args.to_string());
    }
    pub fn enqueue_log(args: std::fmt::Arguments<'_>) {
        _log_print(args);
    }
    pub fn drain_log_once() -> bool {
        false
    }
}
pub mod thread {
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Thread(pub u64);
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum ThreadState {
        Running,
        Ready,
        Blocked,
        BlockedOnSignal,
        BlockedOnChildExit,
        BlockedOnTimer,
        BlockedOnIO,
        Terminated,
    }
    impl ThreadState {
        pub fn is_blocked(self) -> bool {
            !matches!(self, Self::Running | Self::Ready | Self::Terminated)
        }
    }
}
pub mod scheduler {
    use crate::thread::ThreadState;
    pub static SAMPLE: std::sync::Mutex<Option<(ThreadState, Option<u64>)>> =
        std::sync::Mutex::new(None);
    pub fn thread_wait(_: u64) -> Option<(ThreadState, Option<u64>)> {
        *SAMPLE.lock().unwrap()
    }
    pub fn thread_state(_: u64) -> Option<ThreadState> {
        None
    }
    pub fn current_thread_id() -> Option<u64> {
        None
    }
    pub fn yield_current() {}
    pub static SPAWNED: std::sync::Mutex<
        std::collections::BTreeMap<u64, Box<super::thread::Thread>>,
    > = std::sync::Mutex::new(std::collections::BTreeMap::new());
    pub fn spawn(thread: Box<super::thread::Thread>) {
        assert!(SPAWNED.lock().unwrap().insert(thread.0, thread).is_none());
    }
    pub struct Scheduler;
    impl Scheduler {
        pub fn block_current_for_timer(&mut self, _: u64) {}
    }
    pub fn with_current(f: impl FnOnce(&mut Scheduler)) {
        f(&mut Scheduler);
    }
}
pub mod kthread {
    use std::sync::Mutex;
    pub static FAIL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    pub static WORK: Mutex<Option<Box<dyn FnOnce() + Send>>> = Mutex::new(None);
    pub struct ThreadAllocationFailed;
    pub fn kthread_run(
        f: impl FnOnce() + Send + 'static,
        _: &str,
    ) -> Result<(), ThreadAllocationFailed> {
        if FAIL.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(ThreadAllocationFailed);
        }
        *WORK.lock().unwrap() = Some(Box::new(f));
        Ok(())
    }
}
// The included production modules also expose boot-only APIs outside this harness.
#[allow(dead_code)]
#[path = "../kernel/src/task/dispatch_strand_census.rs"]
mod census;

pub mod task {
    pub use crate::{kthread, scheduler, thread};
}
pub mod drivers {
    pub mod xhci {
        use std::sync::atomic::{AtomicU8, Ordering};
        // Completion outcomes: ready, controller failure, timeout.
        pub static RESULT: AtomicU8 = AtomicU8::new(0);
        pub fn wait_console_ready() -> bool {
            assert!(crate::scheduler::SPAWNED.lock().unwrap().is_empty());
            RESULT.load(Ordering::Relaxed) == 0
        }
    }
}
#[path = "../kernel/src/boot/console_start.rs"]
mod console_start;
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}
impl std::fmt::Display for Level {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Error => "ERROR",
            Self::Warn => "WARN",
            Self::Info => "INFO",
            Self::Debug => "DEBUG",
            Self::Trace => "TRACE",
        })
    }
}
#[allow(dead_code)]
#[path = "../kernel/src/console/grid.rs"]
mod grid;
#[allow(dead_code)]
#[path = "../kernel/src/console/raster.rs"]
mod raster;
#[allow(dead_code)]
#[path = "../kernel/src/console/boxdraw.rs"]
mod boxdraw;
static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static SESSION: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
struct Console {
    vts: [vt::Vt; 1],
    kernel_line_open: bool,
}
mod vt {
    pub struct Vt {
        pub grid: super::grid::Grid,
        pub replies: Replies,
    }
    pub struct Replies;
    impl Replies {
        pub fn push(&mut self, _: &[u8]) -> bool {
            true
        }
    }
    pub fn ensure_history(_: usize) {}
}
static GRID: std::sync::Mutex<Console> = std::sync::Mutex::new(Console {
    vts: [vt::Vt {
        grid: grid::Grid::new(),
        replies: vt::Replies,
    }],
    kernel_line_open: false,
});
fn with_grid<R>(f: impl FnOnce(&mut Console) -> R) -> Option<R> {
    Some(f(&mut GRID.lock().unwrap()))
}
#[allow(dead_code)]
#[path = "../kernel/src/console/records.rs"]
mod records;

#[test]
fn input_preparation_and_thread_allocation_failures_still_schedule_init_once() {
    use std::sync::atomic::Ordering::Relaxed;
    for outcome in 0..3 {
        drivers::xhci::RESULT.store(outcome, Relaxed);
        scheduler::SPAWNED.lock().unwrap().clear();
        console_start::schedule(Box::new(thread::Thread(1)));
        assert!(scheduler::SPAWNED.lock().unwrap().is_empty());
        let worker = kthread::WORK.lock().unwrap().take().unwrap();
        worker();
        assert_eq!(
            scheduler::SPAWNED
                .lock()
                .unwrap()
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            [1]
        );
    }
    scheduler::SPAWNED.lock().unwrap().clear();
    kthread::FAIL.store(true, Relaxed);
    console_start::schedule(Box::new(thread::Thread(2)));
    assert!(kthread::WORK.lock().unwrap().is_none());
    assert_eq!(
        scheduler::SPAWNED
            .lock()
            .unwrap()
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        [2]
    );
}

#[test]
fn login_prompt_survives_usb_records_and_periodic_and_immediate_census() {
    with_grid(|console| {
        let g = &mut console.vts[0].grid;
        g.set_storage(vec![grid::BLANK; 800].leak(), vec![grid::BLANK; 800].leak());
        g.set_size(100, 8)
    });
    records::log_record(Level::Info, &format_args!("boot console"));
    records::console_session(true);
    records::write_tty(b"Breenix login: ", |_| {});
    let before = with_grid(|console| {
        let g = &mut console.vts[0].grid;
        (g.generation(), g.cursor())
    })
    .unwrap();
    for message in [
        "[usb] boot keyboard attached: port=2",
        "[usb] boot keyboard detached: port=2",
    ] {
        records::log_record(Level::Info, &format_args!("{message}"));
    }
    census::note_save(7);
    *scheduler::SAMPLE.lock().unwrap() = Some((thread::ThreadState::BlockedOnIO, None));
    serial::OUTPUT.lock().unwrap().clear();
    census::report_snapshot();
    let immediate = serial::OUTPUT.lock().unwrap().clone();
    assert!(immediate.contains("DISPATCH_STRAND_CENSUS:seq=1:"));
    assert!(immediate.contains(":saved=1:stranded=0:"));
    assert!(immediate.contains(":waiting=1]"));
    *scheduler::SAMPLE.lock().unwrap() = Some((thread::ThreadState::BlockedOnTimer, Some(1)));
    serial::OUTPUT.lock().unwrap().clear();
    census::report_heartbeat_if_due();
    let periodic = serial::OUTPUT.lock().unwrap().clone();
    assert!(periodic.contains("DISPATCH_STRAND_CENSUS:seq=2:"));
    assert!(periodic.contains(":saved=1:stranded=1:tids=7:"));
    assert!(periodic.contains(":waiting=0]"));
    assert_eq!(
        with_grid(|console| {
            let g = &mut console.vts[0].grid;
            (g.generation(), g.cursor())
        })
        .unwrap(),
        before
    );
    let screen = with_grid(|console| {
        let g = &mut console.vts[0].grid;
        (0..g.rows())
            .map(|r| {
                g.row(r)
                    .iter()
                    .map(|cell| cell.character())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    })
    .unwrap();
    assert!(screen.contains("Breenix login:"));
    assert!(!screen.contains("[usb]"));
    assert!(!screen.contains("CENSUS"));
    assert!(serial::OUTPUT
        .lock()
        .unwrap()
        .contains("DISPATCH_STRAND_CENSUS"));
    records::log_record(Level::Warn, &format_args!("input unavailable"));
    records::write_tty(b"root", |_| {});
    assert!(
        with_grid(|console| {
            let g = &mut console.vts[0].grid;
            g.generation()
        })
        .unwrap()
            > before.0
    );
    records::console_session(false);
    let before = with_grid(|console| {
        let g = &mut console.vts[0].grid;
        g.generation()
    })
    .unwrap();
    records::log_record(Level::Info, &format_args!("session ended"));
    assert!(
        with_grid(|console| {
            let g = &mut console.vts[0].grid;
            g.generation()
        })
        .unwrap()
            > before
    );
}

// Source guards tie the host doubles to hardware-only producers and sinks.
fn source(path: &str) -> String {
    let test = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(file!());
    std::fs::read_to_string(test.parent().unwrap().parent().unwrap().join(path)).unwrap()
}

fn body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source.find(signature).unwrap();
    let body = &source[start..];
    &body[..body.find("\n}\n").unwrap()]
}

#[test]
fn disk_root_intent_is_consumed_before_registering_a_ram_block_device() {
    let ramdisk = source("kernel/src/block/ramdisk.rs");
    let init = body(&ramdisk, "pub unsafe fn init(");
    assert!(
        init.contains("if crate::boot::console_intent::capture(bytes) {\n        return Ok(());")
    );
    assert!(
        init.find("console_intent::capture(bytes)").unwrap()
            < init.find("RamDisk::new(bytes)").unwrap()
    );
    let main = source("kernel/src/main.rs");
    let mode = body(&main, "fn x86_console_mode()");
    assert!(mode.contains("kernel::boot::console_intent::replug_or(replug)"));
}

#[test]
fn keyboard_preparation_starts_only_after_init_is_published() {
    let main = source("kernel/src/main.rs");
    let mode = body(&main, "fn x86_console_mode()");
    assert!(!mode.contains("console_session_starting") && !mode.contains("input_clock::start"));
    let launch = body(&main, "fn launch_x86_production_init(");
    let published = launch.find("manager.publish_init(publication)").unwrap();
    for start in [
        "kernel::tty::input_clock::start(minutes)",
        "kernel::drivers::xhci::console_session_starting(replug)",
    ] {
        assert!(
            launch.find(start).unwrap() > published,
            "{start} precedes init publication"
        );
    }
}

#[test]
fn readiness_producer_orders_preparation_before_completion_and_wait_rejects_timeout() {
    let keyboard = source("kernel/src/drivers/xhci/keyboard.rs");
    let serve = &keyboard[keyboard.find("pub(super) fn serve(").unwrap()..];
    let request = &serve[serve.find("if CONSOLE_START.swap").unwrap()..];
    let replug = request.find("self.replug_exercise();").unwrap();
    let arm = request.find("self.console.arm();").unwrap();
    let ready = request.find("CONSOLE_READY.complete(1);").unwrap();
    assert!(replug < arm && arm < ready);
    assert!(request[..replug].contains("if REPLUG.swap(false, Ordering::Relaxed)"));
    let xhci = source("kernel/src/drivers/xhci/mod.rs");
    let wait = body(&xhci, "pub fn wait_console_ready()");
    assert!(wait.contains("CONSOLE_READY.wait_timeout_uninterruptible(1, 60 * SECOND)"));
    assert!(wait.contains("matches!(completed, Ok(true)) && !FAILED.load(Ordering::Acquire)"));
}

#[test]
fn real_usb_levels_and_census_sinks_match_host_inputs() {
    let keyboard = source("kernel/src/drivers/xhci/keyboard.rs");
    let compact = keyboard.split_whitespace().collect::<String>();
    for event in ["attached", "detached"] {
        assert!(compact.contains(&format!("log::info!(\"[usb]bootkeyboard{event}:")));
    }
    let census = source("kernel/src/task/dispatch_strand_census.rs");
    let snapshot = body(&census, "fn snapshot(deferred: bool)");
    assert!(snapshot.contains("crate::serial::enqueue_log(args)"));
    assert!(snapshot.contains("crate::serial::_log_print(args)"));
    for sink in [
        "log::",
        "console::",
        "tty::",
        "serial::_print(",
        "serial_print!",
    ] {
        assert!(
            !snapshot.contains(sink),
            "census enters console sink {sink}"
        );
    }
}
