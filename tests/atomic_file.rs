use libbreenix::atomic_file::{
    remove_stale, save, save_with, Operations, SaveResult, StdOperations,
};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "breenix-atomic-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        fs::write(p.join("config"), b"old bytes").unwrap();
        Self(p)
    }
    fn dest(&self) -> PathBuf {
        self.0.join("config")
    }
    fn assert_contents(&self, bytes: &[u8]) {
        assert_eq!(fs::read(self.dest()).unwrap(), bytes);
        assert_eq!(fs::read_dir(&self.0).unwrap().count(), 1);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

struct Inject {
    fail: Option<usize>,
    stop: Option<usize>,
    short: bool,
    writes: usize,
    inner: StdOperations,
}
impl Inject {
    fn new(fail: Option<usize>, stop: Option<usize>, short: bool) -> Self {
        Self {
            fail,
            stop,
            short,
            writes: 0,
            inner: StdOperations,
        }
    }
    fn before(&self, step: usize) -> io::Result<()> {
        if self.fail == Some(step) {
            Err(io::Error::other("injected"))
        } else {
            Ok(())
        }
    }
    fn after(&self, step: usize) {
        if self.stop == Some(step) {
            panic!("interrupted");
        }
    }
}
impl Operations for Inject {
    type File = File;
    fn create_temp(&mut self, dest: &Path) -> io::Result<(PathBuf, File)> {
        self.inner.create_temp(dest)
    }
    fn write(&mut self, file: &mut File, bytes: &[u8]) -> io::Result<usize> {
        self.before(1)?;
        if self.fail == Some(5) {
            return Ok(0);
        }
        if self.fail == Some(6) && self.writes > 0 {
            return Err(io::Error::other("injected after short write"));
        }
        self.writes += 1;
        let n = file.write(if self.short {
            &bytes[..bytes.len().min(2)]
        } else {
            bytes
        })?;
        self.after(1);
        Ok(n)
    }
    fn sync_file(&mut self, file: &File) -> io::Result<()> {
        self.before(2)?;
        file.sync_all()?;
        self.after(2);
        Ok(())
    }
    fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        self.before(3)?;
        fs::rename(from, to)?;
        self.after(3);
        Ok(())
    }
    fn sync_dir(&mut self, dir: &Path) -> io::Result<()> {
        self.before(4)?;
        File::open(dir)?.sync_all()?;
        self.after(4);
        Ok(())
    }
    fn remove_temp(&mut self, path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }
}

#[test]
fn failures_preserve_the_correct_version() {
    for step in 1..=4 {
        let f = Fixture::new();
        let mut ops = Inject::new(Some(step), None, false);
        let result = save_with(&mut ops, &f.dest(), b"new bytes");
        if step == 4 {
            match result {
                SaveResult::Unconfirmed(e) => assert_eq!(e.kind(), io::ErrorKind::Other),
                other => panic!("unexpected result: {other:?}"),
            };
            f.assert_contents(b"new bytes");
        } else {
            match result {
                SaveResult::Failed(e) => assert_eq!(e.kind(), io::ErrorKind::Other),
                other => panic!("unexpected result: {other:?}"),
            };
            f.assert_contents(b"old bytes");
        }
    }
}

#[test]
fn short_writes_are_retried_and_zero_writes_fail_cleanly() {
    let f = Fixture::new();
    assert!(matches!(save(&f.dest(), b"normal save"), SaveResult::Saved));
    let mut ops = Inject::new(None, None, true);
    assert!(matches!(
        save_with(&mut ops, &f.dest(), b"new bytes"),
        SaveResult::Saved
    ));
    assert!(ops.writes > 1);
    f.assert_contents(b"new bytes");
    for failure in [5, 6] {
        let f = Fixture::new();
        let mut ops = Inject::new(Some(failure), None, true);
        assert!(matches!(
            save_with(&mut ops, &f.dest(), b"new bytes"),
            SaveResult::Failed(_)
        ));
        f.assert_contents(b"old bytes");
    }
}

#[test]
fn interruption_at_each_boundary_recovers_without_mixed_data() {
    for step in 1..=4 {
        let f = Fixture::new();
        let mut ops = Inject::new(None, Some(step), false);
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| save_with(
                &mut ops,
                &f.dest(),
                b"new bytes"
            )))
            .is_err()
        );
        remove_stale(&f.dest()).unwrap();
        f.assert_contents(if step < 3 { b"old bytes" } else { b"new bytes" });
    }
}

#[test]
fn cleanup_keeps_other_destinations_temporaries() {
    let f = Fixture::new();
    let other = f.0.join(".config.atomic-other.atomic-1-2");
    fs::write(&other, b"other").unwrap();
    remove_stale(&f.dest()).unwrap();
    assert_eq!(fs::read(&other).unwrap(), b"other");
}
