//! Seed-bounded driver for the design section 5 host crash corpora.
use breenfs_host_tests::{
    commit::{Committer, DeviceStore},
    corrupt::{self, Flip, Kind},
    crash::{self, Image, TornTail},
    format::{Superblock, Time, BLOCK_SIZE},
    minimize::{self, Case, Fault, Outcome},
    ops::Files,
    recording::SECTOR_SIZE,
    recover, Fs, WriterAdapter,
};
use std::{
    any::Any,
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Seek, SeekFrom, Write},
    panic::{catch_unwind, panic_any, AssertUnwindSafe},
    path::PathBuf,
    process::{Command, ExitCode, Output},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use breenfs_host_tests::stream::{torture_operations, torture_seed};
const USAGE: &str =
    "breenfs-torture --seeds N [--seed-start S] --checker PATH --out DIR [--progress FILE]";
type Result<T> = std::result::Result<T, String>;

struct Args {
    seeds: u64,
    last_seed: u64,
    seed: u64,
    checker: PathBuf,
    out: PathBuf,
    progress: Option<PathBuf>,
}

/// How often `--progress` rewrites its file while images are checked.
const PROGRESS_INTERVAL: Duration = Duration::from_secs(5);
/// Waiting seeds are listed only for ranges up to this length; started seeds
/// are listed whatever the range.
const PROGRESS_LISTED_RANGE: u64 = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SeedState {
    Running,
    Pass,
    Fail,
}

#[derive(Debug)]
struct SeedProgress {
    seed: u64,
    state: SeedState,
    images: u64,
    failures: u64,
    first_image: u64,
}

/// Observation only: `--progress FILE` reports per-seed state and image counts
/// for the opt-in battery. The checks themselves are unchanged by it.
#[derive(Debug)]
struct Progress {
    path: PathBuf,
    first_seed: u64,
    last_seed: u64,
    seeds: Vec<SeedProgress>,
    written: Option<Instant>,
    warned: bool,
}

impl Progress {
    fn new(path: PathBuf, first_seed: u64, last_seed: u64) -> Self {
        Self {
            path,
            first_seed,
            last_seed,
            seeds: Vec::new(),
            written: None,
            warned: false,
        }
    }

    fn start(&mut self, seed: u64, images: u64) {
        self.seeds.push(SeedProgress {
            seed,
            state: SeedState::Running,
            images: 0,
            failures: 0,
            first_image: images,
        });
    }

    /// Count checked images toward the running seed; a finished seed keeps its count.
    fn checked(&mut self, images: u64) {
        if let Some(seed) = self.seeds.last_mut() {
            if seed.state == SeedState::Running {
                seed.images = images - seed.first_image;
            }
        }
    }

    fn finish(&mut self, images: u64, passed: bool) {
        self.checked(images);
        if let Some(seed) = self.seeds.last_mut() {
            seed.state = if passed {
                SeedState::Pass
            } else {
                SeedState::Fail
            };
            seed.failures = u64::from(!passed);
        }
    }

    fn json(&self) -> String {
        let entry = |seed: u64, state: &str, images: u64, failures: u64| {
            format!(
                "{{\"seed\":{seed},\"state\":\"{state}\",\"images\":{images},\"failures\":{failures}}}"
            )
        };
        let mut entries: Vec<String> = self
            .seeds
            .iter()
            .map(|s| {
                let state = match s.state {
                    SeedState::Running => "running",
                    SeedState::Pass => "pass",
                    SeedState::Fail => "fail",
                };
                entry(s.seed, state, s.images, s.failures)
            })
            .collect();
        if self.last_seed - self.first_seed < PROGRESS_LISTED_RANGE {
            let next = self.seeds.last().map_or(self.first_seed, |s| s.seed + 1);
            if self.seeds.last().map_or(true, |s| s.seed < self.last_seed) {
                entries.extend((next..=self.last_seed).map(|seed| entry(seed, "waiting", 0, 0)));
            }
        }
        let images: u64 = self.seeds.iter().map(|s| s.images).sum();
        let failures: u64 = self.seeds.iter().map(|s| s.failures).sum();
        format!(
            "{{\"seeds\":[{}],\"images\":{images},\"failures\":{failures}}}\n",
            entries.join(",")
        )
    }

    /// Rewrite the file by atomic rename; unforced rewrites wait five seconds.
    fn write(&mut self, now: Instant, force: bool) {
        if !force
            && self
                .written
                .is_some_and(|at| now.duration_since(at) < PROGRESS_INTERVAL)
        {
            return;
        }
        self.written = Some(now);
        let mut tmp = self.path.clone().into_os_string();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        let result = fs::write(&tmp, self.json()).and_then(|()| fs::rename(&tmp, &self.path));
        if let Err(error) = result {
            // Reporting must not change the run: note the first failure and go on.
            if !self.warned {
                self.warned = true;
                eprintln!("progress {}: {error}", self.path.display());
            }
        }
    }
}

fn args() -> Result<Args> {
    parse_args(std::env::args_os().skip(1))
}

fn parse_args(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<Args> {
    let mut values = BTreeMap::new();
    let mut args = args.into_iter();
    while let Some(flag) = args.next() {
        if ![
            "--seeds",
            "--seed-start",
            "--checker",
            "--out",
            "--progress",
        ]
        .iter()
        .any(|s| flag == *s)
        {
            return Err(format!(
                "unknown option {}\n{USAGE}",
                flag.to_string_lossy()
            ));
        }
        let value = args.next().ok_or_else(|| USAGE.to_owned())?;
        if values.insert(flag, value).is_some() {
            return Err(format!("repeated option\n{USAGE}"));
        }
    }
    let seeds: u64 = values
        .remove(std::ffi::OsStr::new("--seeds"))
        .and_then(|s| s.to_str().and_then(|s| s.parse().ok()))
        .filter(|n| *n != 0)
        .ok_or("--seeds must be a positive unsigned 64-bit integer")?;
    let seed: u64 = match values.remove(std::ffi::OsStr::new("--seed-start")) {
        None => 1,
        Some(s) => s
            .to_str()
            .and_then(|s| s.parse().ok())
            .ok_or("--seed-start must be an unsigned 64-bit integer")?,
    };
    let last_seed = seed
        .checked_add(seeds - 1)
        .ok_or("seed range overflows u64")?;
    let checker = values
        .remove(std::ffi::OsStr::new("--checker"))
        .filter(|s| !s.is_empty())
        .ok_or(USAGE)?
        .into();
    let out = values
        .remove(std::ffi::OsStr::new("--out"))
        .filter(|s| !s.is_empty())
        .ok_or(USAGE)?
        .into();
    let progress = match values.remove(std::ffi::OsStr::new("--progress")) {
        None => None,
        Some(path) if path.is_empty() => return Err(USAGE.to_owned()),
        Some(path) => Some(path.into()),
    };
    Ok(Args {
        seeds,
        last_seed,
        seed,
        checker,
        out,
        progress,
    })
}

// Use the shared formatter entry points, with fixed metadata. No second
// executable or host source directory is needed to prepare the input images.
fn fixture(populated: bool) -> Vec<u8> {
    let total = 16384;
    let device = corrupt::device(Image {
        sectors: BTreeMap::new(),
        sector_count: total * 8,
    });
    let sb = Superblock {
        fs_uuid: [0x24; 16],
        generation: 1,
        slot: 1,
        total_blocks: total,
        global_reserve: 4096,
        next_root_id: 2,
        dirhash_key: std::array::from_fn(|i| i as u8),
        ..Superblock::default()
    };
    let mut files = Files::format(DeviceStore(&device), &sb).expect("format fixture");
    if populated {
        let at = Time::default();
        let dir = files
            .mkdir(2, b"nested", 0o755, at)
            .expect("fixture directory");
        let ino = files.create(dir, b"file", 0o644, at).expect("fixture file");
        files
            .write(&device, ino, 0, &vec![0x5a; BLOCK_SIZE * 3 + 17], at)
            .expect("fixture data");
        files
            .create(2, b"empty", 0o644, at)
            .expect("fixture empty file");
        files
            .symlink(2, b"link", b"nested/file", at)
            .expect("fixture symlink");
    }
    files
        .fsync(&device, &mut Committer::format(sb))
        .expect("fixture commit");
    let image = device.image();
    let mut bytes = vec![0; image.sector_count as usize * SECTOR_SIZE];
    for (sector, data) in image.sectors {
        let at = sector as usize * SECTOR_SIZE;
        bytes[at..at + SECTOR_SIZE].copy_from_slice(&data);
    }
    bytes
}

#[derive(Debug)]
struct Failure {
    kind: String,
    text: String,
}
fn reject(kind: &str, text: impl Into<String>) -> ! {
    panic_any(Failure {
        kind: kind.into(),
        text: text.into(),
    })
}
fn panic_failure(payload: Box<dyn Any + Send>) -> Failure {
    let payload = match payload.downcast::<Failure>() {
        Ok(failure) => return *failure,
        Err(payload) => payload,
    };
    let text = if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else {
        "non-string panic".into()
    };
    // Corpus panic messages put their image coordinates before the assertion.
    let kind = if text.contains(": recover ") {
        "crash recovery"
    } else if text.contains(": state outside commits ") {
        "crash model bounds"
    } else {
        text.lines().next().unwrap_or("panic")
    }
    .to_owned();
    Failure { kind, text }
}
fn attempt(
    run: impl FnOnce() -> breenfs_host_tests::Result<Outcome>,
) -> std::result::Result<Outcome, Failure> {
    match catch_unwind(AssertUnwindSafe(run)) {
        Ok(Ok(outcome)) => Ok(outcome),
        Ok(Err(error)) => Err(Failure {
            kind: format!("{error:?}"),
            text: format!("{error:?}"),
        }),
        Err(payload) => Err(panic_failure(payload)),
    }
}

struct Checker {
    executable: PathBuf,
    path: PathBuf,
    file: File,
    images: u64,
    deepest_generation: u64,
    tree_height: u8,
    progress: Option<Progress>,
}
impl Checker {
    fn report(&mut self, force: bool) {
        if let Some(progress) = self.progress.as_mut() {
            progress.checked(self.images);
            progress.write(Instant::now(), force);
        }
    }
    fn check(&mut self, image: &Image) -> Output {
        let write = (|| -> std::io::Result<()> {
            self.file.set_len(0)?;
            self.file.set_len(image.sector_count * SECTOR_SIZE as u64)?;
            for (&sector, bytes) in &image.sectors {
                self.file
                    .seek(SeekFrom::Start(sector * SECTOR_SIZE as u64))?;
                self.file.write_all(bytes)?;
            }
            self.file.flush()
        })();
        write.unwrap_or_else(|e| reject("image I/O", e.to_string()));
        let output = Command::new(&self.executable)
            .arg("--full")
            .arg(&self.path)
            .output()
            .unwrap_or_else(|e| reject("checker execution", e.to_string()));
        self.images += 1;
        self.report(false);
        output
    }
    fn clean(&mut self, image: &Image) -> breenfs_host_tests::Result<()> {
        let output = self.check(image);
        if output.status.code() != Some(0) {
            reject("checker clean image", format!("{output:?}"));
        }
        let (generation, height) = corrupt::coverage(image)?;
        self.deepest_generation = self.deepest_generation.max(generation);
        self.tree_height = self.tree_height.max(height);
        Ok(())
    }
    fn flipped(
        &mut self,
        image: &Image,
        flip: Flip,
        files: &[(u64, u64)],
    ) -> breenfs_host_tests::Result<()> {
        let output = self.check(image);
        let diagnostic = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if !corrupt::checker_detected(output.status.code(), &diagnostic, flip) {
            reject(
                "checker corruption address",
                format!("{flip:?}: {output:?}"),
            );
        }
        if matches!(flip.kind, Kind::Node { .. })
            && corrupt::read_flipped_node(image, flip) != Err(breenfs_host_tests::Errno::EIO.into())
        {
            reject("node corruption read succeeded", format!("{flip:?}"));
        }
        for writable in [false, true] {
            let device = Arc::new(corrupt::device(image.clone()));
            if catch_unwind(AssertUnwindSafe(|| {
                recover::recover(device.as_ref(), writable)
            }))
            .is_err()
            {
                reject(
                    "corruption recovery panic",
                    format!("writable={writable} {flip:?}"),
                );
            }
            if !writable && device.image() != *image {
                reject("read-only recovery wrote", format!("{flip:?}"));
            }
        }
        let device = Arc::new(corrupt::device(image.clone()));
        let mounted = catch_unwind(AssertUnwindSafe(|| WriterAdapter::new(device)))
            .unwrap_or_else(|_| reject("corruption mount panic", format!("{flip:?}")));
        if let Ok(writer) = mounted {
            if catch_unwind(AssertUnwindSafe(|| writer.state())).is_err() {
                reject("corruption state panic", format!("{flip:?}"));
            }
            for &(ino, size) in files {
                if catch_unwind(AssertUnwindSafe(|| {
                    for offset in (0..=size).step_by(BLOCK_SIZE) {
                        let _ = writer.read(ino, offset, &mut [0; BLOCK_SIZE]);
                    }
                }))
                .is_err()
                {
                    reject("corruption read panic", format!("ino={ino} {flip:?}"));
                }
            }
        }
        Ok(())
    }
}

fn coordinate(text: &str, key: &str) -> Option<usize> {
    text.split_once(key)?
        .1
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

// Translate a callback's ordinal to the case coordinates without depending on
// checker stdout. The corpus panic itself supplies coordinates for a recovery
// failure, which happens before the callback.
fn crash_case(
    initial: &[u8],
    seed: u64,
    tail: Option<TornTail>,
    checked: usize,
    failure: &Failure,
) -> Result<Case> {
    if let (Some(boundary), Some(image)) = (
        coordinate(&failure.text, "boundary="),
        coordinate(&failure.text, "image="),
    ) {
        return crash::case_at(
            initial.to_vec(),
            seed,
            torture_operations(seed),
            boundary,
            image,
            tail,
        )
        .map_err(|e| format!("locate crash: {e:?}"));
    }
    let mut index = checked.saturating_sub(1);
    for boundary in 0.. {
        let case = crash::case_at(
            initial.to_vec(),
            seed,
            torture_operations(seed),
            boundary,
            0,
            tail,
        )
        .map_err(|e| format!("locate crash: {e:?}"))?;
        let Fault::Crash { writes, .. } = case.fault else {
            unreachable!()
        };
        let count = if writes <= 12 {
            1 << writes
        } else {
            writes * 2 + 4097
        };
        if index < count {
            return crash::case_at(
                initial.to_vec(),
                seed,
                torture_operations(seed),
                boundary,
                index,
                tail,
            )
            .map_err(|e| format!("locate crash: {e:?}"));
        }
        index -= count;
    }
    unreachable!()
}

fn replay(
    checker: &mut Checker,
    empty: &[u8],
    populated: &[u8],
    case: &Case,
) -> std::result::Result<Outcome, Failure> {
    attempt(|| match &case.fault {
        Fault::Crash { .. } => {
            crash::replay_case(empty.to_vec(), case, |image| checker.clean(image))
        }
        Fault::Flip(flip) => corrupt::replay_flip(
            if flip.operation.is_some() {
                empty
            } else {
                populated
            }
            .to_vec(),
            case,
            |image, flip, files| checker.flipped(image, flip, files),
        ),
    })
}

fn seed(
    checker: &mut Checker,
    empty: &[u8],
    populated: &[u8],
    seed: u64,
) -> std::result::Result<(), (Option<Box<Case>>, Failure)> {
    let seed = torture_seed(seed);
    for tail in [None, Some(TornTail::Old), Some(TornTail::Garbage)] {
        let mut checked = 0;
        let result = attempt(|| {
            let check = |image: &Image| {
                checked += 1;
                checker.clean(image)
            };
            match tail {
                None => crash::subset_images(empty.to_vec(), seed, torture_operations(seed), check),
                Some(tail) => {
                    crash::torn_images(empty.to_vec(), seed, torture_operations(seed), tail, check)
                }
            }?;
            Ok(Outcome::Accepted)
        });
        if let Err(failure) = result {
            let case = catch_unwind(AssertUnwindSafe(|| {
                crash_case(empty, seed, tail, checked, &failure)
            }))
            .ok()
            .and_then(std::result::Result::ok);
            // A source-run failure precedes a crash image. replay_case records
            // that same stream before testing fault applicability.
            let case = case.unwrap_or_else(|| {
                Case::seeded(
                    seed,
                    torture_operations(seed),
                    Fault::Crash {
                        boundary: 0,
                        writes: 0,
                        selected: vec![],
                        tail: None,
                        tear_seed: 0,
                    },
                )
            });
            return Err((Some(Box::new(case)), failure));
        }
    }
    let mut case = None;
    let result = attempt(|| {
        corrupt::bit_flips(
            populated.to_vec(),
            empty.to_vec(),
            &[seed],
            torture_operations(seed),
            |image, flip, files| {
                case = Some(Box::new(Case::seeded(
                    seed,
                    torture_operations(seed),
                    Fault::Flip(flip),
                )));
                checker.flipped(image, flip, files)
            },
        )?;
        Ok(Outcome::Accepted)
    });
    result.map(|_| ()).map_err(|failure| (case, failure))
}

fn summary(
    args: &Args,
    started: f64,
    clock: Instant,
    last: Option<u64>,
    images: u64,
    failures: u64,
    coverage: (u64, u8),
) -> Result<()> {
    let first = last
        .map(|_| args.seed.to_string())
        .unwrap_or_else(|| "null".into());
    let last = last.map(|s| s.to_string()).unwrap_or_else(|| "null".into());
    let json = format!("{{\n  \"started\": {started},\n  \"seeds_requested\": {},\n  \"elapsed\": {},\n  \"first_seed\": {first},\n  \"last_seed\": {last},\n  \"images_checked\": {images},\n  \"deepest_generation\": {},\n  \"tree_height\": {},\n  \"failures\": {failures}\n}}\n", args.seeds, clock.elapsed().as_secs_f64(), coverage.0, coverage.1);
    let tmp = args.out.join("summary.json.tmp");
    fs::write(&tmp, json).map_err(|e| e.to_string())?;
    fs::rename(tmp, args.out.join("summary.json")).map_err(|e| e.to_string())
}

fn run(args: Args) -> Result<()> {
    fs::create_dir_all(&args.out).map_err(|e| e.to_string())?;
    let clock = Instant::now();
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs_f64();
    summary(&args, started, clock, None, 0, 0, (0, 0))?;
    // Keep this invocation's sparse artifact private even when a directory is
    // reused. A successful run removes it; a failure retains its last image.
    let path = args.out.join(format!("image-{}.img", std::process::id()));
    let mut checker = Checker {
        executable: args.checker.clone(),
        path: path.clone(),
        file: OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(|e| e.to_string())?,
        images: 0,
        deepest_generation: 0,
        tree_height: 0,
        progress: args
            .progress
            .clone()
            .map(|path| Progress::new(path, args.seed, args.last_seed)),
    };
    let empty = fixture(false);
    let populated = fixture(true);
    let mut current = args.seed;
    loop {
        if let Some(progress) = checker.progress.as_mut() {
            progress.start(current, checker.images);
        }
        checker.report(true);
        let result = seed(&mut checker, &empty, &populated, current);
        if let Some(progress) = checker.progress.as_mut() {
            progress.finish(checker.images, result.is_ok());
        }
        checker.report(true);
        if let Err((case, failure)) = result {
            // Publish the failure count before minimization, which may take
            // longer than the requested run. Failure takes the exit-1 path.
            summary(
                &args,
                started,
                clock,
                Some(current),
                checker.images,
                1,
                (checker.deepest_generation, checker.tree_height),
            )?;
            let failure_path = args.out.join(format!("failure-{current}.txt"));
            fs::write(
                &failure_path,
                format!("{}\n{}\n", failure.kind, failure.text),
            )
            .map_err(|e| e.to_string())?;
            let case = case.ok_or("failure occurred before a replayable image")?;
            let reduced = minimize::minimize(*case, |candidate| {
                replay(&mut checker, &empty, &populated, candidate)
                    .is_err_and(|error| error.kind == failure.kind)
            })
            .map_err(|e| format!("minimize: {e:?}"))?;
            let mut file = OpenOptions::new()
                .append(true)
                .open(failure_path)
                .map_err(|e| e.to_string())?;
            writeln!(file, "{}", reduced.regression_row()).map_err(|e| e.to_string())?;
            summary(
                &args,
                started,
                clock,
                Some(current),
                checker.images,
                1,
                (checker.deepest_generation, checker.tree_height),
            )?;
            return Err(format!("seed {current}: {}", failure.kind));
        }
        summary(
            &args,
            started,
            clock,
            Some(current),
            checker.images,
            0,
            (checker.deepest_generation, checker.tree_height),
        )?;
        eprintln!(
            "seed={current} images_checked={} elapsed={:.3}s failures=0",
            checker.images,
            clock.elapsed().as_secs_f64()
        );
        if current == args.last_seed {
            drop(checker);
            fs::remove_file(path).map_err(|e| e.to_string())?;
            return Ok(());
        }
        current = current
            .checked_add(1)
            .ok_or("seed range exhausted before requested count")?;
    }
}

fn main() -> ExitCode {
    // The corpus uses assertions for acceptance. Capture them as failures and
    // minimize them without printing the panic hook during candidate replay.
    std::panic::set_hook(Box::new(|_| {}));
    match args().and_then(run) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn seed_count_selects_an_exact_nonwrapping_range() {
        let parse = |flags: &[&str]| super::parse_args(flags.iter().map(std::ffi::OsString::from));
        let args = parse(&[
            "--seeds",
            "2",
            "--seed-start",
            "7",
            "--checker",
            "check",
            "--out",
            "out",
        ])
        .unwrap();
        assert_eq!((args.seed, args.seeds, args.last_seed), (7, 2, 8));
        let args = parse(&[
            "--seeds",
            "1",
            "--seed-start",
            "18446744073709551615",
            "--checker",
            "check",
            "--out",
            "out",
        ])
        .unwrap();
        assert_eq!(args.last_seed, u64::MAX);
        for flags in [
            vec!["--seeds", "0", "--checker", "check", "--out", "out"],
            vec![
                "--seeds",
                "2",
                "--seed-start",
                "18446744073709551615",
                "--checker",
                "check",
                "--out",
                "out",
            ],
            vec!["--hours", "1", "--checker", "check", "--out", "out"],
            vec![
                "--seeds",
                "1",
                "--checker",
                "check",
                "--out",
                "out",
                "--progress",
                "",
            ],
        ] {
            assert!(parse(&flags).is_err(), "accepted {flags:?}");
        }
        let args = parse(&[
            "--seeds",
            "1",
            "--checker",
            "check",
            "--out",
            "out",
            "--progress",
            "p.json",
        ])
        .unwrap();
        assert_eq!(args.progress, Some(std::path::PathBuf::from("p.json")));
    }

    #[test]
    fn progress_reports_each_seed_and_throttles_rewrites() {
        use std::time::{Duration, Instant};
        let dir = std::env::temp_dir().join(format!("breenfs-progress-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("progress.json");
        let mut progress = super::Progress::new(path.clone(), 3, 5);
        let at = Instant::now();
        progress.start(3, 10);
        progress.checked(14);
        progress.write(at, true);
        let first = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            first,
            "{\"seeds\":[{\"seed\":3,\"state\":\"running\",\"images\":4,\"failures\":0},\
             {\"seed\":4,\"state\":\"waiting\",\"images\":0,\"failures\":0},\
             {\"seed\":5,\"state\":\"waiting\",\"images\":0,\"failures\":0}],\
             \"images\":4,\"failures\":0}\n"
        );
        // A rewrite inside five seconds is skipped; one after it is not.
        progress.checked(20);
        progress.write(at + Duration::from_secs(4), false);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first);
        progress.write(at + Duration::from_secs(5), false);
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("\"images\":10,\"failures\":0}\n"));
        // A finished seed keeps its count when later checks (minimization) run.
        progress.finish(20, false);
        progress.start(4, 25);
        progress.checked(27);
        progress.finish(27, true);
        progress.write(at + Duration::from_secs(6), true);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\"seeds\":[{\"seed\":3,\"state\":\"fail\",\"images\":10,\"failures\":1},\
             {\"seed\":4,\"state\":\"pass\",\"images\":2,\"failures\":0},\
             {\"seed\":5,\"state\":\"waiting\",\"images\":0,\"failures\":0}],\
             \"images\":12,\"failures\":1}\n"
        );
        assert!(!dir.join("progress.json.tmp").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
