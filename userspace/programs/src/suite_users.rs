use libbreenix::suite::{case, category, check, suite, CaseResult, Suite};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::OnceLock;

static STARTUP: OnceLock<Result<Vec<String>, String>> = OnceLock::new();
fn startup() -> CaseResult {
    match STARTUP.get() {
        Some(Ok(_)) => Ok(()),
        Some(Err(why)) => Err(why.clone().into()),
        None => Err("startup did not run".into()),
    }
}

static SUITE: Suite = suite("users", "Users & permissions", &[
    category("layout", "The standard filesystem layout with its owners and modes", &[
        case("fhs-paths", "Standard layout paths have their specified owners and modes", fhs_paths),
        case("shadow-0600", "The shadow database is a root-owned regular file at mode 0600", shadow),
        case("sudo-policy-0440", "The admin policy is a root-owned regular file at mode 0440", sudo_policy),
        case("setuid-set", "Exactly su, passwd and sudo are installed set-user-ID root", setuid_set),
        case("run-cleared", "Startup removes stale runtime and temporary state", run_cleared),
    ]),
]);

fn attrs(path: &Path, mode: u32, uid: u32, gid: u32, directory: bool) -> CaseResult {
    startup()?;
    let meta = fs::symlink_metadata(path).map_err(|e| format!("{}: {}", path.display(), e))?;
    check(if directory { meta.is_dir() } else { meta.is_file() }, &format!("{}: wrong kind", path.display()))?;
    check(meta.uid() == uid && meta.gid() == gid && meta.mode() & 0o7777 == mode,
        &format!("{}: owner {}:{} mode {:o}, expected {}:{} {:o}", path.display(), meta.uid(), meta.gid(), meta.mode() & 0o7777, uid, gid, mode))
}
fn fhs_paths() -> CaseResult {
    for line in libbreenix::layout::TABLE.lines() {
        let row: Vec<_> = line.split_whitespace().collect();
        if row.len() != 5 || !matches!(row[0], "dir" | "file") { continue; }
        let mode = u32::from_str_radix(row[2], 8).map_err(|e| e.to_string())?;
        attrs(&Path::new("/").join(row[1]), mode, row[3].parse().map_err(|_| "bad uid")?, row[4].parse().map_err(|_| "bad gid")?, row[0] == "dir")?;
    }
    for entry in fs::read_dir("/home").map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.file_name().and_then(|n| n.to_str()) == Some("lost+found") { continue; }
        let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if meta.is_dir() { attrs(&path, 0o700, meta.uid(), meta.uid(), true)?; }
    }
    check(!Path::new("/test").exists() && !Path::new("/deep").exists() && !Path::new("/usr/local/test").exists(), "test fixtures remain in administrator paths")
}
fn shadow() -> CaseResult { attrs(Path::new("/etc/shadow"), 0o600, 0, 0, false) }
fn sudo_policy() -> CaseResult { attrs(Path::new("/etc/sudo.policy"), 0o440, 0, 0, false) }
fn setuid_set() -> CaseResult {
    startup()?;
    fn walk(path: &Path, found: &mut Vec<String>) -> CaseResult {
        for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            if meta.is_dir() { walk(&path, found)?; }
            else if meta.mode() & 0o6000 != 0 {
                check(meta.is_file() && meta.uid() == 0 && meta.gid() == 0 && meta.mode() & 0o7777 == 0o4755, "unexpected set-ID ownership or mode")?;
                found.push(path.to_string_lossy().into_owned());
            }
        }
        Ok(())
    }
    let mut found = Vec::new();
    for path in ["/bin", "/sbin", "/usr"] { walk(Path::new(path), &mut found)?; }
    found.sort();
    check(found == ["/bin/su", "/usr/bin/passwd", "/usr/bin/sudo"], &format!("set-ID programs: {:?}", found))
}
fn run_cleared() -> CaseResult {
    startup()?;
    check(!Path::new("/run/layout-stale").exists() && !Path::new("/tmp/layout-stale").exists(), "startup left stale state")
}
fn main() {
    let _ = STARTUP.set(libbreenix::etc_reconcile::prepare(Path::new("/")));
    SUITE.run()
}
