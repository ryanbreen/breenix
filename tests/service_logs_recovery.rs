use libbreenix::service_logs::{KmsgFollower, LIMIT};
use std::{fs, path::PathBuf, sync::atomic::{AtomicUsize, Ordering}};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("blogd-recovery-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn follower(&self) -> KmsgFollower { KmsgFollower::restore(&self.0, &self.0.join("cursor"), "boot").unwrap() }
    fn content(&self) -> String { fs::read_to_string(self.0.join("kernel.log")).unwrap() }
}
impl Drop for Fixture { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
fn snapshot(start: u64, bytes: &[u8]) -> String {
    format!("BREENIX-KMSG 1 start={} end={}\n{}", start, start + bytes.len() as u64, bytes.iter().map(|b| format!("{b:02x}")).collect::<String>())
}
#[test]
fn recovers_unpublished_append_and_keeps_stopped_records() {
    use std::io::Write;
    let f = Fixture::new();
    f.follower().apply(&snapshot(0, b"one\n")).unwrap();
    let committed = f.content();
    fs::OpenOptions::new().append(true).open(f.0.join("kernel.log")).unwrap().write_all(b"[kernel] two\n").unwrap();
    let mut follower = f.follower();
    assert_eq!(f.content(), committed);
    follower.apply(&snapshot(0, b"one\ntwo\nthree\n")).unwrap();
    let log = f.content();
    for line in ["one", "two", "three"] { assert_eq!(log.matches(&format!("[kernel] {line}\n")).count(), 1); }
}
#[test]
fn truncates_torn_line() {
    use std::io::Write;
    let f = Fixture::new();
    f.follower().apply(&snapshot(0, b"one\n")).unwrap();
    let committed = f.content();
    fs::OpenOptions::new().append(true).open(f.0.join("kernel.log")).unwrap().write_all(b"[kernel] tor").unwrap();
    drop(f.follower());
    assert_eq!(f.content(), committed);
}
#[test]
fn recovers_interrupted_rotation() {
    let f = Fixture::new();
    let data = format!("{}\n", "a".repeat(1000)).repeat((LIMIT as usize / 1000) - 1);
    f.follower().apply(&snapshot(0, data.as_bytes())).unwrap();
    let committed = f.content();
    fs::rename(f.0.join("kernel.log"), f.0.join("kernel.log.1")).unwrap();
    fs::write(f.0.join("kernel.log"), b"uncommitted\n").unwrap();
    let mut follower = f.follower();
    assert_eq!(f.content(), committed);
    let all = format!("{}next\n", data);
    follower.apply(&snapshot(0, all.as_bytes())).unwrap();
    let combined = format!("{}{}", fs::read_to_string(f.0.join("kernel.log.1")).unwrap(), f.content());
    assert_eq!(combined.matches("[kernel] next\n").count(), 1);
}
#[test]
fn ring_overrun_is_reported() {
    let f = Fixture::new();
    f.follower().apply(&snapshot(10, b"new\n")).unwrap();
    let log = f.content();
    assert!(log.contains("reason=ring-overrun from=0 to=10"));
    assert!(log.contains("[kernel] new\n"));
}

#[test]
fn failed_cursor_publish_rolls_back_real_rotation_and_restores_partial_input() {
    let f = Fixture::new();
    let data = format!("{}\n", "a".repeat(1000)).repeat((LIMIT as usize / 1010) - 1);
    let mut follower = f.follower();
    follower.apply(&snapshot(0, data.as_bytes())).unwrap();
    let committed = f.content();
    let marker = format!("next-{}", "b".repeat(3000));
    let all = format!("{data}{marker}\npar");
    // Fail the real cursor open after the real append, without a timing race.
    fs::create_dir(f.0.join("cursor.new")).unwrap();
    assert!(follower.apply(&snapshot(0, all.as_bytes())).is_err());
    assert_eq!(fs::read_to_string(f.0.join("kernel.log.1")).unwrap(), committed);
    drop(follower);
    fs::remove_dir(f.0.join("cursor.new")).unwrap();
    let mut follower = f.follower();
    assert_eq!(f.content(), committed);
    follower.apply(&snapshot(0, all.as_bytes())).unwrap();
    assert!(!f.content().contains("[kernel] par"));
    drop(follower);
    f.follower().apply(&snapshot(0, format!("{all}tial\n").as_bytes())).unwrap();
    let combined = format!("{}{}", fs::read_to_string(f.0.join("kernel.log.1")).unwrap(), f.content());
    assert_eq!(combined.matches(&format!("[kernel] {marker}\n")).count(), 1);
    assert_eq!(combined.matches("[kernel] partial\n").count(), 1);
    assert_eq!(combined.matches(&format!("[kernel] {}\n", "a".repeat(1000))).count(),
        (LIMIT as usize / 1010) - 1);
}
