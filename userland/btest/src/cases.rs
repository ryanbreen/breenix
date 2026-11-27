use bu_case::{first_difference, json::Json, Case, Fixture, Goldens, Stdout, MAX_OUTPUT};
use std::fs::{self, DirBuilder, File, Permissions};
use std::io::{self, Read};
use std::os::fd::AsFd;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const WORK: &str = "/var/lib/btest/work";
const RESOURCE_CAPS: &[(bu_sys::Resource, u64)] = &[
    (bu_sys::Resource::CpuSeconds, 2),
    (bu_sys::Resource::FileSize, 16 * 1024 * 1024),
    (bu_sys::Resource::ProcessCount, 16),
    (bu_sys::Resource::AddressSpace, 64 * 1024 * 1024),
];

pub fn supervisor(stage: &Path, id: &str) -> Result<Json, String> {
    let uid = fs::read_to_string("/etc/passwd")
        .map_err(|e| format!("/etc/passwd: {e}"))?
        .lines()
        .filter_map(|line| line.strip_prefix("btest:"))
        .find_map(|line| line.split(':').nth(1)?.parse::<u32>().ok())
        .filter(|&uid| uid != 0)
        .ok_or("no unprivileged btest account")?;
    if bu_sys::user_ids() != (uid, uid) {
        return Err("case supervisor must run as the btest account".into());
    }
    let suite_dir = stage.join("suite");
    let suite = super::load_suite(&suite_dir)?;
    let root = Path::new(WORK).join(id).with_extension(&suite.program);
    DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .map_err(|e| format!("{}: {e}", root.display()))?;
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut failed = 0;
    let mut cases = Vec::new();
    for case in &suite.cases {
        let start = Instant::now();
        let scratch = root.join(&case.id);
        let comparison = run_case(
            &suite_dir,
            &stage.join("bin").join(&suite.binary),
            &scratch,
            deadline,
            case,
        );
        let cleanup = remove_scratch(&scratch);
        let (status, diagnostic) = match (comparison, cleanup) {
            (Ok(None), Ok(())) => ("pass", Json::Null),
            (Ok(Some(diff)), Ok(())) => ("fail", diff.into()),
            (Err(error), _) => ("error", error.into()),
            (_, Err(error)) => ("error", format!("scratch cleanup: {error}").into()),
        };
        failed += usize::from(status != "pass");
        let mut entry = vec![
            ("id".into(), case.id.clone().into()),
            ("status".into(), status.into()),
            ("ms".into(), Json::number(start.elapsed().as_millis())),
        ];
        if status != "pass" {
            let key = if status == "fail" { "diff" } else { "error" };
            entry.push((key.into(), diagnostic));
        }
        cases.push(Json::Object(entry.into_iter().collect()));
    }
    remove_scratch(&root).map_err(|e| format!("{}: {e}", root.display()))?;
    Ok(Json::object([
        ("passed", Json::number(cases.len() - failed)),
        ("failed", Json::number(failed)),
        ("cases", Json::Array(cases)),
    ]))
}

fn remove_scratch(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => {
            // A case can make its directories read-only or remove traversal.
            fs::set_permissions(path, Permissions::from_mode(0o700))?;
            for child in fs::read_dir(path)? {
                remove_scratch(&child?.path())?;
            }
            fs::remove_dir(path)
        }
        Ok(_) => fs::remove_file(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn run_case(
    suite: &Path,
    binary: &Path,
    scratch: &Path,
    deadline: Instant,
    case: &Case,
) -> Result<Option<String>, String> {
    let deadline = deadline.min(Instant::now() + case.timeout);
    let goldens = Goldens::load(suite, &case.id)?;
    fs::create_dir(scratch).map_err(|e| format!("{}: {e}", scratch.display()))?;
    if let Some(fixture) = &case.fixture {
        let path = suite.join("fixtures").join(format!("{fixture}.json"));
        let spec = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Fixture::parse(&spec)?
            .materialize(scratch)
            .map_err(|e| format!("materialize {fixture}: {e}"))?;
    }
    let input = suite.join("inputs").join(format!("{}.in", case.id));
    let stdin = match File::open(&input) {
        Ok(file) => Stdio::from(file),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Stdio::null(),
        Err(e) => return Err(format!("{}: {e}", input.display())),
    };
    let stdout = match case.stdout {
        // Rust may reopen missing stdio. A pipe's read end stays open but
        // Breenix rejects writes to it with EBADF.
        Stdout::Closed => {
            let (reader, _) = io::pipe().map_err(|e| format!("create closed-stdout pipe: {e}"))?;
            Stdio::from(reader)
        }
        _ => Stdio::piped(),
    };
    let mut command = Command::new(binary);
    command
        .current_dir(scratch)
        .process_group(0)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", scratch)
        .env("TZ", "UTC")
        .env("LC_ALL", "C.UTF-8")
        .envs(&case.env)
        .args(&case.argv)
        .stdin(stdin)
        .stdout(stdout)
        .stderr(Stdio::piped());
    bu_sys::cap_child_resources(&mut command, RESOURCE_CAPS);
    let (status, out, err) = execute(&mut command, deadline, case.stdout)?;
    let err = normalize_stderr(&err, binary);
    let status = match status.code() {
        Some(code) => code.to_string(),
        None => format!("signal:{}", status.signal().ok_or("unknown exit status")?),
    };
    Ok(first_difference(
        &goldens.out,
        &out,
        &goldens.err,
        &err,
        &goldens.status,
        &status,
    ))
}

fn normalize_stderr(bytes: &[u8], binary: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    let path = binary.as_os_str().as_bytes();
    let name = binary
        .file_name()
        .expect("binary has a filename")
        .as_bytes();
    let mut normalized = Vec::with_capacity(bytes.len());
    for line in bytes.split_inclusive(|&b| b == b'\n') {
        if let Some(rest) = [path, name].into_iter().find_map(|prefix| {
            line.strip_prefix(prefix)
                .filter(|rest| rest.starts_with(b":"))
        }) {
            normalized.extend_from_slice(b"{prog}");
            normalized.extend_from_slice(rest);
        } else {
            normalized.extend_from_slice(line);
        }
    }
    normalized
}

struct ChildGroup {
    child: Child,
    closed: bool,
}

impl ChildGroup {
    fn terminate(&mut self) -> io::Result<()> {
        if self.closed {
            return Ok(());
        }
        let group = bu_sys::kill_process_group(self.child.id());
        // Always reap the child, even if group signaling failed.
        if self.child.try_wait()?.is_none() {
            self.child.kill()?;
        }
        self.child.wait()?;
        self.closed = true;
        group
    }
}

impl Drop for ChildGroup {
    fn drop(&mut self) {
        let _ = self.terminate();
    }
}

struct Stream<R> {
    reader: Option<R>,
    bytes: Vec<u8>,
    prefix: Option<usize>,
}

impl<R: Read + AsFd> Stream<R> {
    fn new(reader: Option<R>, prefix: Option<usize>) -> io::Result<Self> {
        if let Some(reader) = &reader {
            bu_sys::nonblocking(reader.as_fd())?;
        }
        Ok(Self {
            reader,
            bytes: Vec::new(),
            prefix,
        })
    }

    fn drain(&mut self) -> io::Result<()> {
        let mut buffer = [0; 8192];
        // Limit the work per poll so an infinite writer cannot starve the deadline.
        for _ in 0..8 {
            let Some(reader) = &mut self.reader else {
                break;
            };
            let limit = self.prefix.unwrap_or(MAX_OUTPUT + 1) - self.bytes.len();
            let count = limit.min(buffer.len());
            match reader.read(&mut buffer[..count]) {
                Ok(0) => self.reader = None,
                Ok(n) => {
                    self.bytes.extend_from_slice(&buffer[..n]);
                    if self.prefix == Some(self.bytes.len()) {
                        self.reader = None;
                    } else if self.bytes.len() > MAX_OUTPUT {
                        return Err(io::Error::other("captured output exceeds 1 MiB"));
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

fn execute(
    command: &mut Command,
    deadline: Instant,
    stdout: Stdout,
) -> Result<(ExitStatus, Vec<u8>, Vec<u8>), String> {
    if Instant::now() >= deadline {
        return Err("case or suite timeout before spawn".into());
    }
    let mut group = ChildGroup {
        child: command.spawn().map_err(|e| format!("spawn case: {e}"))?,
        closed: false,
    };
    let prefix = if let Stdout::Prefix(bytes) = stdout {
        Some(bytes)
    } else {
        None
    };
    let mut out =
        Stream::new(group.child.stdout.take(), prefix).map_err(|e| format!("stdout: {e}"))?;
    let mut err =
        Stream::new(group.child.stderr.take(), None).map_err(|e| format!("stderr: {e}"))?;
    let mut status = None;
    loop {
        if Instant::now() >= deadline {
            group
                .terminate()
                .map_err(|e| format!("case timed out, group cleanup failed: {e}"))?;
            return Err("case timed out, process group terminated".into());
        }
        out.drain().map_err(|e| format!("stdout: {e}"))?;
        err.drain().map_err(|e| format!("stderr: {e}"))?;
        if status.is_none() {
            status = group
                .child
                .try_wait()
                .map_err(|e| format!("wait for case: {e}"))?;
            if status.is_some() {
                group
                    .terminate()
                    .map_err(|e| format!("clean up case process group: {e}"))?;
            }
        }
        if let Some(status) = status {
            if out.reader.is_none() && err.reader.is_none() {
                return Ok((status, out.bytes, err.bytes));
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
}
