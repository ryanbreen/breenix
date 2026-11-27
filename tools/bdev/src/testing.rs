use super::{checked, git, sync, validate_path, BoxPaths, SyncManifest, SSH, USAGE};
use bsync::tree::Excludes;
use bsync::Result;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    host: String,
    jump: Option<String>,
    ssh_config: Option<PathBuf>,
    source: String,
}

fn identifier(value: &str) -> bool {
    value
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
}

struct Connection<'a> {
    config: &'a Config,
    directory: tempfile::TempDir,
    options: Vec<String>,
}

impl<'a> Connection<'a> {
    fn open(config: &'a Config) -> Result<(Self, String)> {
        let mut options = vec![
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            "ConnectTimeout=10".into(),
        ];
        if let Some(path) = &config.ssh_config {
            options.extend([
                "-F".into(),
                path.to_str().ok_or("SSH config path must be UTF-8")?.into(),
            ]);
        }
        if let Some(jump) = &config.jump {
            options.extend(["-J".into(), jump.clone()]);
        }
        let settings = checked(
            Command::new(SSH)
                .args(&options)
                .args(["-G", "--", &config.host])
                .output()?,
            "resolve SSH alias",
        )?;
        let user = std::str::from_utf8(&settings)?
            .lines()
            .find_map(|line| line.strip_prefix("user "))
            .filter(|user| identifier(user))
            .ok_or("ssh -G did not return a usable user")?;
        let peer = format!("{user}@{}", config.host);
        // macOS limits UNIX socket path length. Give each master a short, private path.
        let directory = tempfile::tempdir_in("/tmp")?;
        options.extend([
            "-S".into(),
            directory.path().join("sock").display().to_string(),
        ]);
        let status = Command::new(SSH)
            .args(&options)
            .args([
                "-MNf",
                "-o",
                "ControlPersist=no",
                "-o",
                "ClearAllForwardings=yes",
                "--",
                &config.host,
            ])
            .stdin(Stdio::null())
            .status()?;
        if !status.success() {
            return Err(format!("open SSH ControlMaster for {} ({status})", config.host).into());
        }
        // A client may only use this master. A lost socket must fail rather
        // than silently open a fresh connection midway through a sync.
        let options = vec![
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            "ControlMaster=no".into(),
            "-o".into(),
            "ProxyCommand=false".into(),
            "-S".into(),
            directory.path().join("sock").display().to_string(),
        ];
        Ok((
            Self {
                config,
                directory,
                options,
            },
            peer,
        ))
    }
}

impl Drop for Connection<'_> {
    fn drop(&mut self) {
        let _ = Command::new(SSH)
            .arg("-S")
            .arg(self.directory.path().join("sock"))
            .args(["-O", "exit", "--", &self.config.host])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

pub(super) fn changed_paths(checkout: &Path) -> Result<BTreeSet<String>> {
    let mut paths = BTreeSet::new();
    for args in [
        &[
            "diff",
            "--name-only",
            "--no-renames",
            "-z",
            "origin/main...HEAD",
            "--",
        ][..],
        &["diff", "--name-only", "--no-renames", "-z", "HEAD", "--"],
        &["ls-files", "--others", "--exclude-standard", "-z", "--"],
    ] {
        let names = String::from_utf8(git(checkout, args)?)?;
        paths.extend(names.split_terminator('\0').map(str::to_owned));
    }
    Ok(paths)
}

pub(super) fn select_suites(
    paths: &BTreeSet<String>,
    roots: &BTreeSet<String>,
    suites: &BTreeMap<String, BTreeSet<String>>,
) -> Result<Vec<String>> {
    let excludes = Excludes::new(&SyncManifest::load()?.excludes)?;
    let inputs: Vec<_> = paths
        .iter()
        .map(Path::new)
        .filter(|path| {
            roots.iter().any(|root| {
                path.strip_prefix(root)
                    .ok()
                    .and_then(Path::to_str)
                    .is_some_and(|relative| !excludes.matches(relative))
            })
        })
        .collect();
    let workspace_change = inputs.iter().any(|path| {
        **path == *Path::new("userland/Cargo.toml")
            || **path == *Path::new("userland/Cargo.lock")
            || path.starts_with("userland/crates/bu-sys")
    });
    Ok(suites
        .iter()
        .filter(|(_, dependencies)| {
            workspace_change
                || inputs
                    .iter()
                    .any(|path| dependencies.iter().any(|dep| path.starts_with(dep)))
        })
        .map(|(name, _)| name.clone())
        .collect())
}

pub(super) fn test(config_path: Option<&Path>, selectors: &[String]) -> Result<ExitCode> {
    let changed = selectors == ["--changed"];
    let valid_program = |name: &String| {
        name.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
    };
    if !changed && (selectors.is_empty() || !selectors.iter().all(valid_program)) {
        return Err(USAGE.into());
    }

    let home = PathBuf::from(env::var_os("HOME").ok_or("HOME is unset")?);
    let default_config = home.join(".config/breenix-devbox/bdev.json");
    let config_path = config_path.unwrap_or(&default_config);
    let bytes =
        fs::read(config_path).map_err(|error| format!("{}: {error}", config_path.display()))?;
    let config: Config = serde_json::from_slice(&bytes)?;
    if !identifier(&config.host) || config.jump.as_deref().is_some_and(|jump| !identifier(jump)) {
        return Err("host and jump must be SSH aliases".into());
    }
    validate_path(&config.source)?;

    let checkout = String::from_utf8(git(Path::new("."), &["rev-parse", "--show-toplevel"])?)?;
    let checkout = Path::new(checkout.trim_end_matches('\n'));
    let state = super::git_state(checkout)?;
    let manifest = SyncManifest::load()?;
    let layout = manifest.layout(checkout)?;
    let programs = if changed {
        select_suites(&changed_paths(checkout)?, &layout.roots, &layout.suites)?
    } else {
        selectors.to_vec()
    };
    if programs.is_empty() {
        println!("No changed btest suites");
        return Ok(ExitCode::SUCCESS);
    }
    eprintln!("btest suites: {}", programs.join(", "));

    let cache = home.join("Library/Caches/breenix-devbox");
    let runs = cache.join("runs");
    fs::create_dir_all(&runs)?;
    let lock = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(cache.join("workspace.lock"))?;
    lock.lock()?;
    let (connection, peer) = Connection::open(&config)?;
    let remote = BoxPaths {
        host: &peer,
        source: &config.source,
        cache: "",
        ssh_program: Path::new(SSH),
        ssh_args: &connection.options,
    };
    sync(checkout, &remote, true, &manifest, &layout.roots, state)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?;
    let receipt = runs.join(format!("{}-{:09}.json", now.as_secs(), now.subsec_nanos()));
    run_btest(&remote, &programs, &receipt, &mut io::stdout().lock())
}

pub(super) fn run_btest(
    remote: &BoxPaths<'_>,
    programs: &[String],
    receipt: &Path,
    output: &mut impl Write,
) -> Result<ExitCode> {
    let workspace = format!("BTEST_WORKSPACE={}/userland", remote.source);
    let mut words = vec!["/bin/env", &workspace, "btest", "run"];
    words.extend(programs.iter().map(String::as_str));
    words.push("--json");
    let mut child = remote.ssh(&words).stdout(Stdio::piped()).spawn()?;
    let stdout = BufReader::new(child.stdout.take().unwrap());
    let report = collect_summary(stdout, output);
    let status = child.wait()?;
    let code = ExitCode::from(
        status
            .code()
            .and_then(|code| u8::try_from(code).ok())
            .unwrap_or(1),
    );
    let prepared = report.and_then(|bytes| {
        let mut file =
            tempfile::NamedTempFile::new_in(receipt.parent().ok_or("missing run directory")?)?;
        file.write_all(&bytes)?;
        file.persist_noclobber(receipt)?;
        Ok(())
    });
    match prepared {
        Ok(()) => eprintln!("Saved {}", receipt.display()),
        Err(error) if status.success() => return Err(error),
        Err(error) => eprintln!("bdev: could not save the btest summary: {error}"),
    }
    Ok(code)
}

fn collect_summary(mut stdout: impl BufRead, output: &mut impl Write) -> Result<Vec<u8>> {
    let mut summary = None;
    let mut stream_error = None;
    let mut line = Vec::new();
    while stdout.read_until(b'\n', &mut line)? != 0 {
        if let Err(error) = output.write_all(&line).and_then(|()| output.flush()) {
            stream_error.get_or_insert(error);
        }
        if serde_json::from_slice::<Value>(&line)
            .is_ok_and(|record| record.get("kind").and_then(Value::as_str) == Some("summary"))
        {
            summary = Some(std::mem::take(&mut line));
        }
        line.clear();
    }
    if let Some(error) = stream_error {
        eprintln!("bdev: could not stream btest output: {error}");
    }
    summary.ok_or_else(|| "btest did not return a JSON summary".into())
}
