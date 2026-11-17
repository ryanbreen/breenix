//! Exercise corruption reporting against the host-built disposable fixtures.
use breenix_suite::suite::{check, CaseResult};
use libbreenix::{fs as sysfs, syscall::{nr, raw}};
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const MANIFEST: &str = "/usr/local/filesystem-suite/corrupt/manifest";
fn io<T>(r: std::io::Result<T>) -> Result<T, String> { r.map_err(|e| e.to_string()) }

struct Fixture { dir: PathBuf, disk: u8, mounted: bool }
impl Fixture {
    fn mount(kind: &str) -> Result<Self, String> {
        let dir = PathBuf::from(format!("/tmp/filesystem-corrupt-{}", std::process::id()));
        io(fs::create_dir(&dir))?;
        let result = (|| {
            let mut disks = Vec::new();
            for entry in io(fs::read_dir("/dev"))? {
                let name = io(entry)?.file_name();
                if let Some(n) = name.to_str().and_then(|s| s.strip_prefix("disk")).and_then(|s| s.parse::<u8>().ok()) { disks.push(n); }
            }
            disks.sort_unstable();
            for disk in disks {
                let target = dir.to_str().ok_or("mount path encoding")?;
                if unsafe { sysfs::mount(&format!("/dev/disk{disk}"), target, "breenfs", 1, std::ptr::null()) }.is_err() { continue; }
                let matched = fs::read(dir.join("corrupt-marker")).is_ok_and(|b| b == kind.as_bytes());
                sysfs::umount2(target, 0).map_err(|e| e.to_string())?;
                if matched {
                    unsafe { sysfs::mount(&format!("/dev/disk{disk}"), target, "breenfs", 0, std::ptr::null()) }.map_err(|e| e.to_string())?;
                    return Ok(Self { dir: dir.clone(), disk, mounted: true });
                }
            }
            Err(format!("{kind} corruption fixture not found"))
        })();
        if result.is_err() { let _ = fs::remove_dir(&dir); }
        result
    }
    fn unmount(&mut self) -> Result<(), String> {
        sysfs::umount2(self.dir.to_str().ok_or("mount path encoding")?, 0).map_err(|e| e.to_string())?;
        self.mounted = false;
        Ok(())
    }
    fn remount(&mut self) -> Result<(), String> {
        unsafe { sysfs::mount(&format!("/dev/disk{}", self.disk), self.dir.to_str().ok_or("mount path encoding")?, "breenfs", 0, std::ptr::null()) }.map_err(|e| e.to_string())?;
        self.mounted = true;
        Ok(())
    }
    fn finish(mut self) -> CaseResult { self.unmount()?; io(fs::remove_dir(&self.dir))?; Ok(()) }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if self.mounted && self.unmount().is_err() { return; }
        let _ = fs::remove_dir(&self.dir);
    }
}
fn block(key: &str) -> Result<u64, String> {
    let text = io(fs::read_to_string(MANIFEST))?;
    let rows: Vec<_> = text.lines().filter_map(|line| {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() == 3 && fields[0] == key { Some(fields[1]) } else { None }
    }).collect();
    if rows.len() != 1 { return Err(format!("missing or duplicate {key} in manifest")); }
    rows[0].parse::<u64>().map_err(|e| e.to_string())
}
fn mode(dir: &Path, expected: &str) -> CaseResult {
    let mounts = io(fs::read_to_string("/proc/mounts"))?;
    let path = dir.to_str().ok_or("mount path encoding")?;
    let rows: Vec<_> = mounts.lines().map(|l| l.split_whitespace().collect::<Vec<_>>())
        .filter(|r| r.len() >= 4 && r[1] == path).collect();
    check(rows.len() == 1 && rows[0][3].split(',').any(|v| v == expected), &format!("mount is not {expected}: {rows:?}"))
}
fn errno<T>(result: std::io::Result<T>, expected: i32, operation: &str) -> CaseResult {
    match result { Err(e) if e.raw_os_error() == Some(expected) => Ok(()),
        Err(e) => Err(format!("{operation}: expected errno {expected}, got {e}").into()),
        Ok(_) => Err(format!("{operation}: expected errno {expected}, succeeded").into()) }
}
fn marker(f: &Fixture, expected: &str) -> CaseResult {
    check(io(fs::read(f.dir.join("corrupt-marker")))? == expected.as_bytes(), "marker does not read back")
}
pub fn data_flip_eio() -> CaseResult {
    let f = Fixture::mount("data")?;
    let _ = block("data-block")?;
    let mut file = io(File::open(f.dir.join("rot/target")))?;
    errno(file.read(&mut [0u8; 4096]), 5, "read corrupt data")?;
    drop(file);
    marker(&f, "data")?;
    mode(&f.dir, "rw")?;
    f.finish()
}
pub fn node_flip_eio_readonly() -> CaseResult {
    let f = Fixture::mount("node")?;
    let _ = block("node-block")?;
    let path = f.dir.join("rot-dir/f0001");
    errno(fs::metadata(&path), 5, "stat corrupt node")?;
    errno(File::open(&path), 5, "open corrupt node")?;
    mode(&f.dir, "ro")?;
    errno(OpenOptions::new().write(true).create_new(true).open(f.dir.join("new-file")), 30, "create on read-only mount")?;
    marker(&f, "node")?;
    f.finish()
}
fn command(action: &str, dir: &Path) -> Result<(bool, String), String> {
    let mut child = Command::new("/usr/bin/breenfs").arg("scrub").arg(action).arg(dir)
        .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    let start = Instant::now();
    loop {
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            let output = child.wait_with_output().map_err(|e| e.to_string())?;
            return Ok((output.status.success(), String::from_utf8(output.stdout).map_err(|e| e.to_string())?));
        }
        if start.elapsed() >= Duration::from_secs(15) {
            let _ = child.kill(); let _ = child.wait();
            return Err(format!("scrub {action} timed out"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
fn field<'a>(text: &'a str, key: &str) -> Result<&'a str, String> {
    text.split_whitespace().find_map(|word| word.strip_prefix(key).and_then(|s| s.strip_prefix('=')))
        .ok_or_else(|| format!("missing {key} in {text:?}"))
}
fn scrub(f: &Fixture) -> CaseResult {
    let (ok, _) = command("start", &f.dir)?;
    check(ok, "scrub start failed")?;
    let (ok, out) = command("wait", &f.dir)?;
    check(!ok && field(&out, "state")? == "done" && field(&out, "errors")? == "1", &format!("unexpected scrub result: {out}"))?;
    let expected = block("data-block")?;
    check(out.lines().any(|line| field(line, "bad_block").ok().and_then(|s| s.parse::<u64>().ok()) == Some(expected) && field(line, "kind").ok() == Some("data")), &format!("missing data block {expected}: {out}"))
}
pub fn scrub_names_block() -> CaseResult {
    let f = Fixture::mount("data")?;
    scrub(&f)?;
    f.finish()
}
pub fn scrub_recorded() -> CaseResult {
    let mut f = Fixture::mount("data")?;
    let (ok, before) = command("status", &f.dir)?;
    check(ok, "initial scrub status failed")?;
    let previous = field(&before, "last_scrub_generation")?.parse::<u64>().map_err(|e| e.to_string())?;
    scrub(&f)?;
    unsafe { raw::syscall0(nr::SYNC) };
    f.unmount()?;
    f.remount()?;
    let (ok, out) = command("status", &f.dir)?;
    check(ok && field(&out, "last_scrub_errors")? == "1" && field(&out, "last_scrub_generation")?.parse::<u64>().map_err(|e| e.to_string())? > previous, &format!("scrub report not persisted: {out}"))?;
    f.finish()
}
