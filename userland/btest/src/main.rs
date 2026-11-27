mod cases;

use bu_case::{identifier, json::Json, Suite};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{self, BufRead, BufReader, Write};
use std::os::fd::AsFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{self, Command, ExitCode, Stdio};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const USAGE: &str = "Usage: btest run <prog>...";
const STATE: &str = "/var/lib/btest";
const CURRENT: &str = "/opt/breenix/rust/CURRENT";

fn main() -> ExitCode {
    let args: Vec<_> = env::args().skip(1).collect();
    match args.as_slice() {
        [help] if help == "--help" => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        [action, args @ ..] if action == "run" && !args.is_empty() => {
            let json = args.last().is_some_and(|arg| arg == "--json");
            report(&args[..args.len() - usize::from(json)], json)
        }
        [action, stage, id, options @ ..]
            if action == "__case-supervisor"
                && identifier(id)
                && (options.is_empty() || options == ["--json"]) =>
        {
            match cases::supervisor(Path::new(stage), id, !options.is_empty()) {
                Ok(result) => {
                    println!("{result}");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("btest: {error}");
                    ExitCode::from(2)
                }
            }
        }
        _ => {
            eprintln!("btest: {USAGE}");
            ExitCode::from(2)
        }
    }
}

fn report(packages: &[String], json: bool) -> ExitCode {
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch");
    let wall = Instant::now();
    let id = format!(
        "{}-{:09}-{}",
        started.as_secs(),
        started.subsec_nanos(),
        process::id()
    );
    let path = Path::new(STATE).join("runs").join(format!("{id}.json"));
    let mut fields = BTreeMap::from([
        ("kind".into(), "summary".into()),
        ("startedAtUnixMs".into(), Json::number(started.as_millis())),
        ("path".into(), path.to_string_lossy().into_owned().into()),
    ]);
    let lock = File::options()
        .read(true)
        .write(true)
        .open(Path::new(STATE).join("run.lock"))
        .map_err(|e| format!("open btest run lock: {e}"))
        .and_then(|file| {
            bu_sys::lock_exclusive(file.as_fd())
                .map_err(|e| format!("acquire btest run lock: {e}"))?;
            Ok(file)
        });
    let mut code = match lock
        .as_ref()
        .map_err(Clone::clone)
        .and_then(|_| run(packages, &id, json))
    {
        Ok(run) => {
            fields.insert("suites".into(), Json::Array(run.suites));
            fields.insert("toolchain".into(), run.toolchain.into());
            fields.insert("tree".into(), run.tree);
            if run.failed {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(error) => {
            fields.insert("error".into(), error.into());
            ExitCode::from(2)
        }
    };
    fields.insert("wallMs".into(), Json::number(wall.elapsed().as_millis()));
    let mut result = Json::Object(fields);
    let mut line = format!("{result}\n");
    if let Err(error) = save(&path, line.as_bytes()) {
        // Preserve the run outcome and attach a publication error to it.
        if let Json::Object(fields) = &mut result {
            fields.insert("reportError".into(), error.to_string().into());
        }
        line = format!("{result}\n");
        code = ExitCode::from(2);
    }
    if let Err(error) = io::stdout().lock().write_all(line.as_bytes()) {
        eprintln!("btest: write summary: {error}");
        return ExitCode::from(2);
    }
    code
}

fn save(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = path.with_extension("tmp");
    let mut out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)?;
    let result = out.write_all(bytes).and_then(|()| out.sync_all());
    drop(out);
    if let Err(error) = result.and_then(|()| fs::rename(&temp, path)) {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    // The file sync covers its contents. Commit the rename in the parent directory.
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("run report has no parent"))?;
    File::open(parent)?.sync_all()
}

struct Run {
    suites: Vec<Json>,
    toolchain: String,
    tree: Json,
    failed: bool,
}

fn run(packages: &[String], id: &str, json: bool) -> Result<Run, String> {
    if packages.is_empty() {
        return Err(USAGE.into());
    }
    let mut seen = BTreeSet::new();
    for package in packages {
        if !identifier(package) || !seen.insert(package) {
            return Err(format!("invalid or repeated package {package:?}"));
        }
    }
    let home = PathBuf::from(env::var_os("HOME").ok_or("HOME is unset")?);
    let workspace = env::var_os("BTEST_WORKSPACE")
        .map_or_else(|| home.join("src/breenix/userland"), PathBuf::from);
    let workspace = workspace
        .canonicalize()
        .map_err(|e| format!("{}: {e}", workspace.display()))?;
    let stamp = workspace
        .parent()
        .ok_or("workspace must have a parent")?
        .join(".btest-tree.json");
    let tree =
        Json::parse(&fs::read_to_string(&stamp).map_err(|e| format!("{}: {e}", stamp.display()))?)?;
    if !matches!(tree, Json::Object(_)) {
        return Err("tree stamp must be a JSON object".into());
    }
    let toolchain = fs::read_to_string(CURRENT)
        .map_err(|e| format!("{CURRENT}: {e}"))?
        .trim()
        .to_owned();
    let cache = home.join(".cache/btest/target");
    let suites = packages
        .iter()
        .map(|package| {
            let dir = workspace.join(package).join("suite");
            let suite = load_suite(&dir)?;
            if suite.program != *package || suite.binary != *package {
                return Err(format!(
                    "{}: suite program and binary must match package {package}",
                    dir.display()
                ));
            }
            Ok((dir, suite))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut builds = Command::new("/bin/nice");
    let installed = Path::new("/usr/bin/bcargo");
    let bcargo = if installed.is_file() {
        installed.into()
    } else {
        home.join("bin/bcargo")
    };
    builds
        .current_dir(&workspace)
        .arg("-n")
        .arg("10")
        .arg(bcargo)
        .arg("build")
        .args(["--release", "--offline", "--frozen", "--target-dir"])
        .arg(&cache);
    for package in packages {
        builds.arg("-p").arg(package);
    }
    let build_start = Instant::now();
    let output = builds.output();
    let build_ms = build_start.elapsed().as_millis();
    let build_error = match output {
        Ok(output) if output.status.success() => None,
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            eprint!("{stderr}{stdout}");
            let lines = || stderr.lines().chain(stdout.lines());
            let detail = lines()
                .find(|line| line.trim_start().starts_with("error"))
                .or_else(|| lines().find(|line| !line.trim().is_empty()));
            Some(format!(
                "bcargo {}: {}",
                output.status,
                detail.unwrap_or("no diagnostic")
            ))
        }
        Err(error) => Some(format!("launch bcargo: {error}")),
    };
    let bin_dir = cache
        .join(format!("{}-breenix", env::consts::ARCH))
        .join("release");
    let mut results = Vec::new();
    let mut failed = false;
    for (dir, suite) in suites {
        let mut build = BTreeMap::from([("ms".into(), Json::number(build_ms))]);
        let binary = bin_dir.join(&suite.binary);
        let digest = match &build_error {
            Some(error) => Err(error.clone()),
            None => sha256(&binary),
        };
        let mut result = BTreeMap::from([("pkg".into(), suite.program.clone().into())]);
        match digest {
            Err(error) => {
                build.insert("status".into(), "fail".into());
                build.insert("error".into(), error.into());
                failed = true;
            }
            Ok(digest) => {
                build.insert("status".into(), "ok".into());
                build.insert("sha256".into(), digest.into());
                match run_suite(&dir, &suite, &binary, id, json) {
                    Ok(cases) => {
                        failed |= !matches!(cases.get("failed"), Some(Json::Number(n)) if n == "0");
                        result.extend(cases);
                    }
                    Err(error) => {
                        result.insert("error".into(), error.into());
                        failed = true;
                    }
                }
            }
        }
        result.insert("build".into(), Json::Object(build));
        results.push(Json::Object(result));
    }
    Ok(Run {
        suites: results,
        toolchain,
        tree,
        failed,
    })
}

fn sha256(path: &Path) -> Result<String, String> {
    let output = Command::new("/bin/sha256sum")
        .arg(path)
        .output()
        .map_err(|e| format!("hash {}: {e}", path.display()))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let digest = text
        .split_whitespace()
        .next()
        .filter(|digest| {
            output.status.success()
                && digest.len() == 64
                && digest.bytes().all(|b| b.is_ascii_hexdigit())
        })
        .ok_or_else(|| {
            format!(
                "cannot hash {}: {}",
                path.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            )
        })?;
    Ok(digest.into())
}

pub(crate) fn load_suite(dir: &Path) -> Result<Suite, String> {
    let path = dir.join("manifest.json");
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    Suite::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

struct Stage(PathBuf);

impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run_suite(
    dir: &Path,
    suite: &Suite,
    binary: &Path,
    id: &str,
    json: bool,
) -> Result<BTreeMap<String, Json>, String> {
    let parent = Path::new(STATE)
        .join("stage")
        .join(format!("{id}-{}", suite.program));
    fs::create_dir(&parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    let stage = Stage(parent);
    fs::set_permissions(&stage.0, Permissions::from_mode(0o755))
        .map_err(|e| format!("stage permissions: {e}"))?;
    let staged_bin = stage.0.join("bin");
    stage_dir(&staged_bin)?;
    let staged_suite = stage.0.join("suite");
    for name in ["", "fixtures", "golden", "inputs"] {
        stage_dir(&staged_suite.join(name))?;
    }
    copy(
        &dir.join("manifest.json"),
        &staged_suite.join("manifest.json"),
        0o444,
    )?;
    for case in &suite.cases {
        for ext in ["out", "err", "status"] {
            let path = PathBuf::from("golden").join(format!("{}.{ext}", case.id));
            copy(&dir.join(&path), &staged_suite.join(&path), 0o444)?;
        }
        if let Some(fixture) = &case.fixture {
            let path = PathBuf::from("fixtures").join(format!("{fixture}.json"));
            copy(&dir.join(&path), &staged_suite.join(&path), 0o444)?;
        }
        let path = PathBuf::from("inputs").join(format!("{}.in", case.id));
        if dir.join(&path).exists() {
            copy(&dir.join(&path), &staged_suite.join(&path), 0o444)?;
        }
    }
    let supervisor = stage.0.join("supervisor");
    let current = env::current_exe().map_err(|e| format!("current executable: {e}"))?;
    copy(&current, &supervisor, 0o555)?;
    copy(binary, &staged_bin.join(&suite.binary), 0o555)?;
    // Stock wheel policy permits root. The second sudo uses that policy's
    // root path to drop every credential before executing the supervisor.
    let mut command = Command::new("/usr/bin/sudo");
    command
        .current_dir("/")
        .args(["-n", "/usr/bin/sudo", "-n", "-u", "btest"])
        .arg(supervisor)
        .arg("__case-supervisor")
        .arg(&stage.0)
        .arg(id);
    if json {
        command.arg("--json");
    }
    let mut child = command
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("launch btest account: {e}"))?;
    let result = read_cases(BufReader::new(child.stdout.take().unwrap()));
    let status = child
        .wait()
        .map_err(|e| format!("wait for case supervisor: {e}"))?;
    if !status.success() {
        return Err(format!("case supervisor {status}"));
    }
    result
}

fn read_cases(stdout: impl BufRead) -> Result<BTreeMap<String, Json>, String> {
    let mut result = None;
    let mut stream_error = None;
    for line in stdout.lines() {
        let line = line.map_err(|e| format!("read case result: {e}"))?;
        match Json::parse(&line) {
            Ok(Json::Object(fields)) if matches!(fields.get("kind"), Some(Json::String(kind)) if kind == "case") => {
                if let Err(error) = write_case_line(&line) {
                    stream_error.get_or_insert(error);
                }
            }
            Ok(Json::Object(fields)) if result.is_none() => result = Some(fields),
            _ => {
                stream_error.get_or_insert_with(|| "invalid case supervisor result".into());
            }
        }
    }
    if let Some(error) = stream_error {
        return Err(error);
    }
    result.ok_or_else(|| "missing case supervisor result".into())
}

fn write_case_line(line: &str) -> Result<(), String> {
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "{line}")
        .and_then(|()| stdout.flush())
        .map_err(|e| format!("write case line: {e}"))
}

fn stage_dir(path: &Path) -> Result<(), String> {
    fs::create_dir(path)
        .and_then(|()| fs::set_permissions(path, Permissions::from_mode(0o755)))
        .map_err(|e| format!("stage {}: {e}", path.display()))
}

fn copy(source: &Path, dest: &Path, mode: u32) -> Result<(), String> {
    let context = |e| format!("stage {}: {e}", source.display());
    fs::copy(source, dest).map_err(context)?;
    fs::set_permissions(dest, Permissions::from_mode(mode)).map_err(context)
}
