#![forbid(unsafe_code)]

use bsync::remote::Destination;
use bsync::tree::{plan, scan, Action, Entry, Excludes, Tree};
use bsync::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::{Command, ExitCode, Output, Stdio};
use std::time::{Duration, Instant};

#[cfg(test)]
mod tests;

const STAMP: &str = ".btest-tree.json";
const TOOLCHAIN: &str = "/opt/breenix/rust";
const USAGE: &str =
    "usage: bdev --host USER@HOST --source /BOX/SOURCE --cache /BOX/CACHE sync | build PACKAGE";

#[derive(Deserialize)]
struct SyncManifest {
    roots: Vec<String>,
    excludes: Vec<String>,
}

impl SyncManifest {
    fn load() -> Result<Self> {
        Ok(serde_json::from_str(include_str!("../sync.json"))?)
    }

    fn roots(&self, checkout: &Path) -> Result<BTreeSet<String>> {
        let userland = checkout.join("userland/Cargo.toml");
        let mut roots = self
            .roots
            .iter()
            .filter(|root| *root != "userland" || userland.is_file())
            .cloned()
            .collect::<BTreeSet<_>>();
        if userland.is_file() {
            #[derive(Deserialize)]
            struct Package {
                manifest_path: String,
                source: Option<String>,
            }
            #[derive(Deserialize)]
            struct Metadata {
                packages: Vec<Package>,
            }
            let output = checked(
                Command::new("cargo")
                    .current_dir(checkout)
                    .args(["metadata", "--format-version", "1", "--offline", "--locked"])
                    .args(["--all-features", "--manifest-path"])
                    .arg(userland)
                    .output()?,
                "cargo metadata",
            )?;
            let metadata: Metadata = serde_json::from_slice(&output)?;
            for package in metadata
                .packages
                .into_iter()
                .filter(|pkg| pkg.source.is_none())
            {
                let directory = Path::new(&package.manifest_path)
                    .parent()
                    .ok_or("package manifest has no parent directory")?
                    .strip_prefix(checkout)?
                    .to_str()
                    .ok_or("package path is not UTF-8")?;
                if directory.starts_with("libs/") {
                    roots.insert(directory.into());
                } else if !directory.starts_with("userland/") && directory != "userland" {
                    return Err(
                        format!("path dependency outside the sync roots: {directory}").into(),
                    );
                }
            }
        }
        let included = roots.clone();
        roots.retain(|root| {
            !Path::new(root).ancestors().skip(1).any(|ancestor| {
                ancestor
                    .to_str()
                    .is_some_and(|path| included.contains(path))
            })
        });
        Ok(roots)
    }
}

#[derive(Debug, Deserialize, PartialEq, Eq, Serialize)]
struct GitState {
    commit: String,
    origin_main: String,
    ancestor_of_main: bool,
    dirty: bool,
}

#[derive(Deserialize, Serialize)]
struct TreeStamp {
    #[serde(flatten)]
    git: GitState,
    roots: BTreeMap<String, String>,
}

fn git(checkout: &Path, args: &[&str]) -> Result<Vec<u8>> {
    checked(
        Command::new("git")
            .current_dir(checkout)
            .args(args)
            .output()?,
        &format!("git {}", args[0]),
    )
}

fn git_state(checkout: &Path) -> Result<GitState> {
    let commit = String::from_utf8(git(checkout, &["rev-parse", "HEAD"])?)?
        .trim_end()
        .to_owned();
    let origin_main = String::from_utf8(git(checkout, &["rev-parse", "origin/main"])?)?
        .trim_end()
        .to_owned();
    let status = Command::new("git")
        .current_dir(checkout)
        .args(["merge-base", "--is-ancestor", &commit, &origin_main])
        .status()?;
    let ancestor_of_main = match status.code() {
        Some(0) => true,
        Some(1) => false,
        _ => return Err(format!("git merge-base failed ({status})").into()),
    };
    let dirty = !git(
        checkout,
        &["status", "--porcelain=v1", "-z", "--untracked-files=normal"],
    )?
    .is_empty();
    Ok(GitState {
        commit,
        origin_main,
        ancestor_of_main,
        dirty,
    })
}

fn stage(checkout: &Path, snapshot: &Path, roots: &BTreeSet<String>) -> Result<()> {
    for root in roots {
        fs::create_dir_all(snapshot.join(root))?;
    }
    let mut args = vec![
        "ls-files",
        "--cached",
        "--others",
        "--exclude-standard",
        "-z",
        "--",
    ];
    args.extend(roots.iter().map(String::as_str));
    let files = String::from_utf8(git(checkout, &args)?)?;
    for name in files.split_terminator('\0').collect::<BTreeSet<_>>() {
        let relative = Path::new(name);
        for parent in relative.ancestors().skip(1) {
            match fs::symlink_metadata(checkout.join(parent)) {
                Ok(metadata) if metadata.is_dir() => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                _ => return Err(format!("source parent is not a directory: {parent:?}").into()),
            }
        }
        let source = checkout.join(relative);
        let metadata = match fs::symlink_metadata(&source) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let destination = snapshot.join(relative);
        fs::create_dir_all(destination.parent().ok_or("source file has no parent")?)?;
        if metadata.is_file() {
            fs::copy(&source, &destination)?;
        } else if metadata.is_symlink() {
            symlink(fs::read_link(source)?, destination)?;
        } else {
            return Err(format!("unsupported source type: {relative:?}").into());
        }
    }
    Ok(())
}

fn source_tree(source: &Path, excludes: &Excludes) -> Result<Tree> {
    let mut tree = scan(source, excludes)?;
    // A directory created while staging an excluded file has no payload.
    let mut directories = BTreeSet::from(["".to_owned()]);
    for (path, entry) in &tree {
        if matches!(entry, Entry::File { .. }) {
            for ancestor in Path::new(path).ancestors().skip(1) {
                directories.insert(ancestor.to_str().expect("valid bsync path").to_owned());
            }
        }
    }
    tree.retain(|path, entry| !matches!(entry, Entry::Dir(_)) || directories.contains(path));
    Ok(tree)
}

fn source_plan(local: &Tree, remote: &Tree, excludes: &Excludes) -> Result<Vec<Action>> {
    let actions = plan(local, remote, excludes, true)?;
    if let Some(action) = actions
        .iter()
        .find(|action| matches!(action, Action::RemoveFile(_) | Action::RemoveDir(_)))
    {
        return Err(format!("sync would remove unexpected box path {:?}", action.path()).into());
    }
    Ok(actions)
}

fn root_digest(tree: &Tree) -> String {
    let mut hash = Sha256::new();
    // Paths are sorted by bsync and contain no control characters.
    for (path, entry) in tree {
        match entry {
            Entry::Dir(mode) => hash.update(format!("d\t{mode:o}\t{path}\n")),
            Entry::File { size, mode, digest } => {
                hash.update(format!("f\t{mode:o}\t{size}\t{digest}\t{path}\n"));
            }
            Entry::Other => unreachable!("local bsync scan only returns files and directories"),
        }
    }
    format!("{:x}", hash.finalize())
}

struct BoxPaths<'a> {
    host: &'a str,
    source: &'a str,
    cache: &'a str,
}

impl BoxPaths<'_> {
    fn destination(&self, path: &str) -> Result<Destination> {
        Destination::parse(&format!("{}:{path}", self.host))
    }

    fn ssh(&self, words: &[&str]) -> Command {
        // OpenSSH sends one exec string rather than an argv array. Quote
        // each argument without relying on pipelines, expansion or redirects.
        let request = words
            .iter()
            .map(|word| format!("'{}'", word.replace('\'', "'\\''")))
            .collect::<Vec<_>>()
            .join(" ");
        let mut command = Command::new("/usr/bin/ssh");
        command.args(["-T", "-o", "BatchMode=yes", "--", self.host, &request]);
        command.stdin(Stdio::null());
        command
    }

    fn run(&self, words: &[&str]) -> Result<Vec<u8>> {
        checked(self.ssh(words).output()?, words[0])
    }

    fn put(&self, path: &str, bytes: &[u8]) -> Result<()> {
        let size = bytes.len().to_string();
        let digest = format!("{:x}", Sha256::digest(bytes));
        let mut child = self
            .ssh(&["/bin/bput", path, &size, &digest, "644"])
            .stdin(Stdio::piped())
            .spawn()?;
        let written = child.stdin.take().unwrap().write_all(bytes);
        let status = child.wait()?;
        written?;
        if !status.success() {
            return Err(format!("publish tree stamp failed ({status})").into());
        }
        Ok(())
    }
}

fn checked(output: Output, step: &str) -> Result<Vec<u8>> {
    if !output.status.success() {
        return Err(format!(
            "{step} failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim_end()
        )
        .into());
    }
    Ok(output.stdout)
}

fn sync(checkout: &Path, remote: &BoxPaths<'_>) -> Result<()> {
    let git = git_state(checkout)?;
    let manifest = SyncManifest::load()?;
    let roots = manifest.roots(checkout)?;
    let excludes = Excludes::new(&manifest.excludes)?;
    let snapshot = tempfile::tempdir()?;
    stage(checkout, snapshot.path(), &roots)?;
    if git_state(checkout)? != git {
        return Err("Git state changed while preparing the source snapshot".into());
    }

    let mut stamp = TreeStamp {
        git,
        roots: BTreeMap::new(),
    };
    remote.run(&["/bin/mkdir", "-p", remote.source])?;
    let mut work = Vec::new();
    for root in roots {
        let path = format!("{}/{root}", remote.source);
        let (parent, _) = path.rsplit_once('/').ok_or("source root has no parent")?;
        remote.run(&["/bin/mkdir", "-p", parent])?;
        let source = snapshot.path().join(&root);
        let tree = source_tree(&source, &excludes)?;
        let destination = remote.destination(&path)?;
        let listing = destination.inventory(false, &excludes)?;
        let actions = source_plan(&tree, &listing.entries, &excludes)?;
        work.push((root, source, tree, destination, listing, actions));
    }
    for (root, source, tree, destination, listing, actions) in work {
        eprintln!("bsync {root} ({} changes)", actions.len());
        for action in &actions {
            eprintln!("{} {:?}", action.label(), action.path());
        }
        destination.apply(&listing, &source, &tree, &actions, false)?;
        stamp.roots.insert(root, root_digest(&tree));
    }

    let mut bytes = serde_json::to_vec_pretty(&stamp)?;
    bytes.push(b'\n');
    remote.put(&format!("{}/{STAMP}", remote.source), &bytes)?;
    io::stdout().write_all(&bytes)?;
    Ok(())
}

fn toolchain_id(contents: &str) -> Result<&str> {
    let id = contents.strip_suffix('\n').unwrap_or(contents);
    let valid = id
        .strip_prefix("x86_64-")
        .and_then(|suffix| suffix.split_once('-'))
        .is_some_and(|(commit, hash)| {
            [commit, hash]
                .iter()
                .all(|part| part.len() == 8 && part.bytes().all(|byte| byte.is_ascii_hexdigit()))
        });
    if !valid {
        return Err("invalid x86-64 toolchain ID in CURRENT".into());
    }
    Ok(id)
}

fn cargo_words(source: &str, cache: &str, id: &str, package: &str) -> Vec<String> {
    let fixture = format!("{TOOLCHAIN}/{id}");
    // The self-host invocation in userspace/programs/src/suite_support/d1.rs.
    let flags = [
        format!("--sysroot={fixture}/sysroot"),
        format!("-Clinker={fixture}/bin/bld"),
        format!("-Clink-arg=-T{fixture}/lib/linker.ld"),
        format!("-Lnative={fixture}/lib"),
        "-Clink-arg=--allow-multiple-definition".into(),
        "-Cdefault-linker-libraries=no".into(),
        format!("--remap-path-prefix={source}=/breenix-src"),
    ];
    vec![
        "/bin/time".into(),
        "-p".into(),
        "/bin/env".into(),
        "-i".into(),
        "PATH=/bin".into(),
        format!("HOME={cache}/cargo-home"),
        format!("CARGO_HOME={cache}/cargo-home"),
        "CARGO_BUILD_JOBS=4".into(),
        "CARGO_TERM_COLOR=never".into(),
        format!("RUSTC={fixture}/bin/rustc"),
        format!("RUST_TARGET_PATH={fixture}/targets"),
        format!("CARGO_ENCODED_RUSTFLAGS={}", flags.join("\u{1f}")),
        format!("TMPDIR={cache}/tmp"),
        format!("{fixture}/bin/cargo"),
        // env on the box has no chdir option. Cargo's -C gives it the
        // suite's working directory before resolving config or the manifest.
        "-Z".into(),
        "unstable-options".into(),
        "-C".into(),
        format!("{source}/toolchain"),
        "build".into(),
        "--offline".into(),
        "--frozen".into(),
        "--profile".into(),
        "selfhost".into(),
        "--target".into(),
        format!("{fixture}/targets/x86_64-breenix.json"),
        "--manifest-path".into(),
        format!("{source}/toolchain/Cargo.toml"),
        "-p".into(),
        package.into(),
        "--target-dir".into(),
        format!("{cache}/target"),
    ]
}

fn elapsed_ms(log: &str) -> Option<u64> {
    let seconds = log
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("real "))?;
    let duration = Duration::try_from_secs_f64(seconds.trim().parse().ok()?).ok()?;
    duration.as_millis().try_into().ok()
}

#[derive(Deserialize)]
struct Pin {
    sha256: String,
}

fn build(remote: &BoxPaths<'_>, package: &str) -> Result<ExitCode> {
    if package.is_empty()
        || package.starts_with('-')
        || !package
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
    {
        return Err("invalid Cargo package name".into());
    }
    let stamp: TreeStamp =
        serde_json::from_slice(&remote.run(&["/bin/cat", &format!("{}/{STAMP}", remote.source)])?)?;
    if !stamp.roots.contains_key("toolchain") {
        return Err("the source stamp has no toolchain root".into());
    }

    let current = String::from_utf8(remote.run(&["/bin/cat", &format!("{TOOLCHAIN}/CURRENT")])?)?;
    let id = toolchain_id(&current)?;
    remote.run(&[
        "/bin/mkdir",
        "-p",
        &format!("{}/cargo-home/registry", remote.cache),
        &format!("{}/tmp", remote.cache),
        &format!("{}/target", remote.cache),
    ])?;
    let words = cargo_words(remote.source, remote.cache, id, package);
    let start = Instant::now();
    let output = remote
        .ssh(&words.iter().map(String::as_str).collect::<Vec<_>>())
        .output()?;
    let ssh_wall_ms = start.elapsed().as_millis();
    let stderr = String::from_utf8_lossy(&output.stderr);
    io::stderr().write_all(&output.stdout)?;
    io::stderr().write_all(&output.stderr)?;

    let artifact = if package == "bobj" {
        "libbobj.rlib"
    } else {
        package
    };
    #[derive(Deserialize)]
    struct Receipt {
        outputs: BTreeMap<String, Pin>,
    }
    let pins: Receipt = serde_json::from_str(include_str!(
        "../../../toolchain/rust-host/selfhost/x86_64-mac.json"
    ))?;
    let mut sha256 = None;
    let mut digest_error = None;
    let pin = pins.outputs.get(artifact);
    if let Some(pin) = pin.filter(|_| output.status.success()) {
        let path = format!("{}/target/x86_64-breenix/selfhost/{artifact}", remote.cache);
        let result = remote.run(&["/bin/sha256sum", &path]).and_then(|bytes| {
            let text = String::from_utf8(bytes)?;
            let digest = text
                .split_whitespace()
                .next()
                .ok_or("missing artifact SHA-256")?;
            if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err("invalid artifact SHA-256".into());
            }
            Ok(digest.to_owned())
        });
        match result {
            Ok(digest) => {
                if digest != pin.sha256 {
                    digest_error = Some("artifact SHA-256 differs from the E25 record".into());
                }
                sha256 = Some(digest);
            }
            Err(error) => digest_error = Some(error.to_string()),
        }
    }
    let ok = output.status.success() && digest_error.is_none();
    println!(
        "{}",
        serde_json::json!({
            "package": package,
            "toolchain": id,
            "commit": stamp.git.commit,
            "ok": ok,
            "exit_code": output.status.code(),
            "wall_ms": elapsed_ms(&stderr),
            "ssh_wall_ms": ssh_wall_ms,
            "artifact": pin.map(|_| artifact),
            "sha256": sha256,
            "expected_sha256": pin.map(|pin| &pin.sha256),
            "digest_error": digest_error,
        })
    );
    Ok(if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn validate_path(path: &str) -> Result<()> {
    if !path.starts_with('/')
        || !path[1..].split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
        })
    {
        return Err(
            "box paths must be absolute and contain only letters, digits, /, -, _ or .".into(),
        );
    }
    Ok(())
}

fn run() -> Result<ExitCode> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args == ["--help"] {
        println!("{USAGE}");
        return Ok(ExitCode::SUCCESS);
    }
    let [host_option, host, source_option, source, cache_option, cache, command @ ..] =
        args.as_slice()
    else {
        return Err(USAGE.into());
    };
    if host_option != "--host" || source_option != "--source" || cache_option != "--cache" {
        return Err(USAGE.into());
    }
    validate_path(source)?;
    validate_path(cache)?;
    if Path::new(source).starts_with(cache) || Path::new(cache).starts_with(source) {
        return Err("the source and cache must be separate directories".into());
    }
    let remote = BoxPaths {
        host,
        source,
        cache,
    };
    remote.destination(source)?;
    match command {
        [verb] if verb == "sync" => {
            let directory =
                String::from_utf8(git(Path::new("."), &["rev-parse", "--show-toplevel"])?)?;
            sync(Path::new(directory.trim_end_matches('\n')), &remote)?;
            Ok(ExitCode::SUCCESS)
        }
        [verb, package] if verb == "build" => build(&remote, package),
        _ => Err(USAGE.into()),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(status) => status,
        Err(error) => {
            eprintln!("bdev: {error}");
            ExitCode::FAILURE
        }
    }
}
