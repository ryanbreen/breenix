#[path = "../userspace/programs/src/bwm/clock.rs"]
mod clock;
#[path = "../userspace/programs/src/bwm/timing.rs"]
mod timing;

use clock::Clock;
use std::fs::{self, OpenOptions};
use std::os::unix::fs::FileExt;

#[path = "../libs/libuitest/src/clock.rs"]
mod step_clock;

extern crate alloc;
use libbreenix::window::abi;
// Supply the registry and provider boundaries to the production ACK operation.
// The counter belongs to completed composites, not ACKs.
struct Counter(std::sync::atomic::AtomicU64);
impl Counter {
    const fn new() -> Self { Self(std::sync::atomic::AtomicU64::new(0)) }
    fn aggregate(&self) -> u64 { self.0.load(std::sync::atomic::Ordering::Relaxed) }
}
mod tracing { pub mod providers { pub mod window {
    pub(crate) static WINDOW_COMPOSITES: crate::Counter = crate::Counter::new();
}}}
struct Caller { tgid: u64 }
impl Caller { fn is_root(&self) -> bool { true } }
#[derive(Clone, Copy)]
struct Claim { holder: u64, epoch: u64, ack: abi::CompositorAck, first_serial: u64, composites_base: u64 }
struct Window { owner: u64, registered: bool }
struct Record { serial: u64, flags: u32 }
struct Presents { records: Vec<Record> }
struct Registry {
    claim: Option<Claim>, windows: std::collections::BTreeMap<u64, Window>,
    present: Presents, scene_version: u64,
}
struct RegistryLock(std::sync::Mutex<Registry>);
impl RegistryLock {
    fn lock(&self) -> std::sync::MutexGuard<'_, Registry> { self.0.lock().unwrap() }
}
static REGISTRY: RegistryLock = RegistryLock(std::sync::Mutex::new(Registry {
    claim: None, windows: std::collections::BTreeMap::new(),
    present: Presents { records: Vec::new() }, scene_version: 0,
}));
mod status_pages {
    pub fn format_status(status: &libbreenix::window::abi::CompositorStatus) -> String {
        format!("composites={}", status.composites)
    }
}
mod pacing { pub fn text() -> String { String::new() } }
#[path = "../kernel/src/window/status.rs"]
mod status;

#[test]
fn idle_epoch_is_required_and_taskbar_uses_wall_time_on_scene_redraw() {
    // Isolate the process-wide clock environment from the existing clock case.
    const CHILD: &str = "BWM_EPOCH_CHECK_CHILD";
    if std::env::var_os(CHILD).is_none() {
        assert!(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "idle_epoch_is_required_and_taskbar_uses_wall_time_on_scene_redraw"])
            .env(CHILD, "1").status().unwrap().success());
        return;
    }
    use libbreenix::{ui_clock, window::abi};
    let path = std::env::temp_dir().join(format!("bwm-epoch-{}", std::process::id()));
    ui_clock::ClockPage::create(&path, 1_704_067_200_000_000_000).unwrap();
    std::env::set_var("BREENIX_UI_CLOCK", &path);
    let reads = ui_clock::real_clock_reads();
    let mut clock = Clock::new(true);
    clock.participate().unwrap();
    let driver = step_clock::Clock::open(&path).unwrap();
    let mut idle = driver.begin(0, 0, |_| false).unwrap();
    assert_eq!(idle.waiting(), &[ui_clock::COMPOSITOR_ID]);
    clock.begin_epoch();
    assert_eq!(clock.update_clock(), None);
    assert_eq!(clock.scene_clock(), None, "idle epoch must not schedule a composite");
    let ack = clock.acknowledgment(9);
    assert_eq!((ack.epoch, ack.presented, ack.flags), (1, 0, abi::compositor_ack_flags::NONE_NEEDED));
    let caller = Caller { tgid: 7 };
    REGISTRY.lock().claim = Some(Claim { holder: 7, epoch: 9, ack: Default::default(), first_serial: 0, composites_base: 0 });
    tracing::providers::window::WINDOW_COMPOSITES.0.store(17, std::sync::atomic::Ordering::Relaxed);
    let mut status = status::read(&caller).unwrap();
    idle.observe_compositor(&status);
    assert!(!idle.complete());
    status.epoch = ack.epoch;
    idle.observe_compositor(&status);
    assert!(!idle.complete(), "an absent receipt without none-needed is not an ack");
    status::ack(&caller, &ack).unwrap();
    status = status::read(&caller).unwrap();
    idle.observe_compositor(&status);
    assert!(idle.complete());
    assert_eq!(status.composites, 17, "none-needed acknowledgment must not composite");
    assert_eq!(status::text(&caller).unwrap(), "composites=17");
    clock.acknowledged();

    let mut next = driver.begin(2000, 1, |_| false).unwrap();
    clock.begin_epoch();
    assert_eq!(clock.update_clock(), None, "stepped time alone must not paint chrome");
    // A separately required scene composite absorbs the current taskbar text.
    let sec = clock.scene_clock().unwrap();
    assert_eq!(sec as u64 * 1_000_000_000, ui_clock::wall_ns());
    let mut text = [0; 11];
    clock::format_clock(sec, &mut text);
    assert_eq!(&text, b"19:00:02 ET");
    clock.presented(42);
    let ack = clock.acknowledgment(9);
    assert_eq!((ack.epoch, ack.presented, ack.flags), (2, 42, 0));
    REGISTRY.lock().present.records.push(Record { serial: 42, flags: 0 });
    tracing::providers::window::WINDOW_COMPOSITES.0.store(18, std::sync::atomic::Ordering::Relaxed);
    status::ack(&caller, &ack).unwrap();
    status = status::read(&caller).unwrap();
    next.observe_compositor(&status);
    assert!(next.complete());
    assert!(next.check(1).is_ok());
    assert_eq!(next.stamp().now_ns, 2_000_000_000);
    clock.acknowledged();
    assert_eq!(clock.scene_clock(), None);
    assert_eq!(ui_clock::real_clock_reads(), reads);
    drop(clock);
    assert!(ui_clock::ClockPage::open(&path).unwrap().participants().is_empty());
    std::env::remove_var("BREENIX_UI_CLOCK");
    fs::remove_file(path).unwrap();
}

#[test]
fn stepped_clock_acknowledges_without_clock_composites_or_fps() {
    let path = std::env::temp_dir().join(format!("bwm-clock-{}", std::process::id()));
    let page = OpenOptions::new().read(true).write(true).create_new(true).open(&path).unwrap();
    let mut bytes = [0; 64];
    bytes[..8].copy_from_slice(b"BGCLK001");
    bytes[8..16].copy_from_slice(&1u64.to_le_bytes());
    bytes[16..24].copy_from_slice(&250_000_000u64.to_le_bytes());
    bytes[24..32].copy_from_slice(&1_767_225_600_000_000_000u64.to_le_bytes());
    page.write_all_at(&bytes, 0).unwrap();
    std::env::set_var("BREENIX_UI_CLOCK", &path);

    let reads = libbreenix::ui_clock::real_clock_reads();
    let mut clock = Clock::new(true);
    assert_eq!(clock.wall_sec(), 1_767_225_600);
    assert_eq!(clock.update_clock(), None);
    let ack = clock.acknowledgment(9);
    assert_eq!((ack.claim_epoch, ack.epoch, ack.presented, ack.flags), (9, 1, 0, 1));
    assert_eq!(ack.next_deadline_ns, 0, "a test clock schedules no timed repaint");

    clock.presented(42);
    clock.begin_epoch();
    assert_eq!(clock.acknowledgment(9).presented, 42);
    page.write_all_at(&2_250_000_000u64.to_le_bytes(), 16).unwrap();
    page.write_all_at(&2u64.to_le_bytes(), 8).unwrap();
    clock.begin_epoch();
    assert_eq!(clock.wall_sec(), 1_767_225_602);
    assert_eq!(clock.update_clock(), None, "stepping wall time must not composite chrome");
    let ack = clock.acknowledgment(9);
    assert_eq!((ack.epoch, ack.presented, ack.next_deadline_ns), (2, 0, 0));
    clock.presented(43);
    assert_eq!(clock.acknowledgment(9).presented, 43);
    assert_eq!(libbreenix::ui_clock::real_clock_reads(), reads);

    // The production loop has no FPS emitter to wake an otherwise idle desktop.
    assert!(!include_str!("../userspace/programs/src/bwm.rs").contains("[bwm-fps]"));
    let mut real = Clock::new(false);
    assert_eq!(real.update_clock(), None);
    let ack = real.acknowledgment(9);
    // Lane 2A schedules the next wall-second boundary in monotonic time.
    assert_eq!(ack.next_deadline_ns, 3_000_000_000);
    assert_eq!((ack.epoch, ack.flags), (2, 0));
    page.write_all_at(&3_000_000_000u64.to_le_bytes(), 16).unwrap();
    page.write_all_at(&3u64.to_le_bytes(), 8).unwrap();
    real.begin_epoch();
    assert_eq!(real.update_clock(), Some(1_767_225_603));
    assert_eq!(real.update_clock(), None);
    assert_eq!(real.acknowledgment(9).next_deadline_ns, 4_000_000_000);

    // The repeat scheduler consumes the mapped clock, including a frozen page.
    use libbreenix::{input::{Event, key, kind}, ui_clock};
    let source = Event { serial: 42, device: 7, kind: kind::KEY, code: key::A,
        value: 1, ..Event::default() };
    let mut repeats = timing::Timing::new();
    assert!(repeats.key(source, Some(9), false, ui_clock::now_ns()));
    assert_eq!(repeats.deadline(), Some(3_500_000_000));
    for _ in 0..100 {
        assert!(repeats.due(ui_clock::now_ns(), Some(9)).is_empty());
    }
    page.write_all_at(&3_499_999_999u64.to_le_bytes(), 16).unwrap();
    assert!(repeats.due(ui_clock::now_ns(), Some(9)).is_empty());
    page.write_all_at(&3_500_000_000u64.to_le_bytes(), 16).unwrap();
    let first = repeats.due(ui_clock::now_ns(), Some(9));
    assert_eq!(first.len(), 1);
    assert_eq!((first[0].source.serial, first[0].target, first[0].time, first[0].derived),
        (42, 9, 3_500_000_000, 2));
    assert!(!repeats.key(Event { value: 2, ..source }, Some(9), false, ui_clock::now_ns()));
    page.write_all_at(&3_600_000_000u64.to_le_bytes(), 16).unwrap();
    assert_eq!(repeats.due(ui_clock::now_ns(), Some(9)).iter().map(|r| r.time).collect::<Vec<_>>(),
        [3_550_000_000, 3_600_000_000]);
    repeats.key(Event { value: 0, ..source }, Some(9), false, ui_clock::now_ns());
    page.write_all_at(&4_000_000_000u64.to_le_bytes(), 16).unwrap();
    assert!(repeats.due(ui_clock::now_ns(), Some(9)).is_empty());
    let mut clicks = timing::Clicks::new();
    assert_eq!(clicks.press(7, 272, 9, (2, 3), ui_clock::now_ns()), 1);
    assert_eq!(clicks.release(7, 272), 1);
    clicks.retain_devices(|device| device == 7);
    page.write_all_at(&4_400_000_000u64.to_le_bytes(), 16).unwrap();
    assert_eq!(clicks.press(7, 272, 9, (2, 3), ui_clock::now_ns()), 2);
    assert!(timing::within(4_000_000_000, ui_clock::now_ns()));
    assert_eq!(ui_clock::real_clock_reads(), reads);
    std::env::remove_var("BREENIX_UI_CLOCK");
    fs::remove_file(path).unwrap();
}
