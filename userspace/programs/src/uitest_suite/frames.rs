//! Frame schema cases from docs/gui/plan.md section 7.5.
use crate::frame_case::{fail, CaseResult};
use crate::frame_schema::{self, Context, Formula, Param, ParamType, Value};

// Suite-source signatures, also compiled by the manifest validator.
pub const FORMULAS: &[Formula] = &[Formula {
    name: "schema-pattern",
    params: &[
        Param {
            name: "unsigned",
            kind: ParamType::U32(0, 10),
        },
        Param {
            name: "signed",
            kind: ParamType::I32(-2, 2),
        },
        Param {
            name: "colour",
            kind: ParamType::Colour,
        },
        Param {
            name: "rect",
            kind: ParamType::Rect,
        },
        Param {
            name: "window",
            kind: ParamType::WindowKey,
        },
        Param {
            name: "seed",
            kind: ParamType::Seed,
        },
    ],
}];

pub fn fixtures() -> Vec<Value> {
    frame_schema::from_str(include_str!(
        "../../../../tests/fixtures/frame-schema-v1.json"
    ))
    .expect("committed frame schema fixture table")
}

pub fn verdict(row: &Value) -> Result<frame_schema::Check, String> {
    let frame = Some((
        row["frame"][0].as_u64().unwrap() as u32,
        row["frame"][1].as_u64().unwrap() as u32,
    ));
    let previous = row
        .get("previous")
        .map(|value| {
            frame_schema::validate(
                value,
                &Context {
                    frame,
                    formulas: FORMULAS,
                    previous: None,
                },
            )
        })
        .transpose()?;
    frame_schema::validate(
        &row["check"],
        &Context {
            frame,
            formulas: FORMULAS,
            previous: previous.as_ref(),
        },
    )
}

fn group(name: &str) -> CaseResult {
    let mut seen = 0;
    for row in fixtures().iter().filter(|row| row["group"] == name) {
        seen += 1;
        match (verdict(row), row["error"].as_str()) {
            (Ok(_), None) => {}
            (Err(error), Some(expected)) if error.contains(expected) => {}
            (result, expected) => {
                return fail(format!(
                    "schema {}: expected {expected:?}, got {result:?}",
                    row["name"]
                ))
            }
        }
    }
    if seen == 0 {
        return fail(format!("empty schema group {name}"));
    }
    Ok(())
}

pub fn unknown_field() -> CaseResult {
    group("unknown-field")
}
pub fn formula_params() -> CaseResult {
    group("formula-params")
}
pub fn schema_version() -> CaseResult {
    group("schema-version")
}

pub fn mask() -> CaseResult {
    group("mask")?;
    let rows = fixtures();
    let row = rows.iter().find(|r| r["name"] == "valid-mask").unwrap();
    let check = verdict(row)?;
    if check.compares(0, 0) || !check.compares(10, 10) || check.compared_area != 9900 {
        return fail("mask did not exclude exactly its rectangle");
    }
    Ok(())
}

pub fn mask_cap() -> CaseResult {
    group("mask-cap")?;
    let rows = fixtures();
    let check = verdict(
        rows.iter()
            .find(|r| r["name"] == "four-border-strips")
            .unwrap(),
    )?;
    if check.compared_area != 9604 {
        return fail("border strip union must leave 9604 pixels");
    }
    Ok(())
}

pub fn structural_vacuous() -> CaseResult {
    group("structural-vacuous")?;
    let rows = fixtures();
    let row = rows
        .iter()
        .find(|r| r["name"] == "outside-unmasked")
        .unwrap();
    let check = verdict(row)?;
    if check.effective_area != 396
        || !check.in_effective_domain(0, 0)
        || check.in_effective_domain(1, 1)
    {
        return fail("outside effective domain differs from border");
    }
    let mut pixels = vec![0u8; 200 * 100 * 4];
    let outside_matches = |pixels: &[u8]| {
        use libpixcheck::{Format, Frame, Options, Predicate, Rect};
        let frame =
            Frame::new(pixels, 200, 100, 800, Format::Bgrx).map_err(|e| format!("{e:?}"))?;
        libpixcheck::structural(
            frame,
            Options {
                regions: &[Rect {
                    x: 0,
                    y: 0,
                    w: 100,
                    h: 100,
                }],
                ..Options::default()
            },
            Predicate::Outside {
                region: 0,
                rect: Rect {
                    x: 1,
                    y: 1,
                    w: 98,
                    h: 98,
                },
                colour: [255, 0, 0],
            },
            None,
        )
        .map_err(|e| format!("{e:?}"))
    };
    if !outside_matches(&pixels)? {
        return fail("clean outside region failed");
    }
    pixels[2] = 255;
    if outside_matches(&pixels)? {
        return fail("outside missed planted border pixel");
    }
    Ok(())
}

// These cases use the guest device interface; the schema host harness above
// compiles the pure schema cases on its own.
#[cfg(target_os = "breenix")]
mod races {
    use libbreenix::{test_control as tc, window::abi::tc_point};
    use libuitest::{arena::{Arena, Layout, Record, Slot, SlotRead}, capture,
        capture_marker::Generation, capture_race, Session};
    use std::{io::{BufRead, BufReader, Write}, os::unix::net::{UnixListener, UnixStream},
        path::{Path, PathBuf}, sync::mpsc, thread, time::{Duration, Instant}};

    struct App { socket: BufReader<UnixStream>, pid: u64 }
    #[derive(Clone, Copy)]
    struct Commit { id: u64, generation: Generation, width: u32, height: u32 }
    impl App {
        fn launch(session: &mut Session) -> Result<Self, String> {
            let path = Path::new(session.config_root()).join("capture-control.sock");
            let listener = UnixListener::bind(&path).map_err(|e| e.to_string())?;
            listener.set_nonblocking(true).map_err(|e| e.to_string())?;
            let pid = session.launch_app(&[b"/usr/libexec/breenix/uitest-app_test", b"capture races", b"--capture-control"], 0)?;
            let end = Instant::now() + Duration::from_secs(2);
            let socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < end => thread::sleep(Duration::from_millis(1)),
                    Err(e) => return Err(format!("capture app connection: {e}")),
                }
            };
            socket.set_read_timeout(Some(Duration::from_secs(2))).map_err(|e| e.to_string())?;
            socket.set_write_timeout(Some(Duration::from_secs(2))).map_err(|e| e.to_string())?;
            std::fs::remove_file(path).map_err(|e| e.to_string())?;
            Ok(Self { socket: BufReader::new(socket), pid })
        }
        fn send(&mut self, command: &str) -> Result<(), String> {
            writeln!(self.socket.get_mut(), "{command}").map_err(|e| e.to_string())
        }
        fn reply(&mut self) -> Result<Commit, String> {
            let mut line = String::new();
            self.socket.read_line(&mut line).map_err(|e| e.to_string())?;
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() != 6 || fields[0] != "ok" { return Err(format!("capture app: {line:?}")); }
            let number = |i: usize| fields[i].parse::<u64>().map_err(|e| e.to_string());
            Ok(Commit { id: number(1)?, generation: Generation { commit: number(2)?,
                producer: u32::try_from(number(3)?).map_err(|e| e.to_string())? },
                width: u32::try_from(number(4)?).map_err(|e| e.to_string())?,
                height: u32::try_from(number(5)?).map_err(|e| e.to_string())? })
        }
        fn command(&mut self, command: &str) -> Result<Commit, String> { self.send(command)?; self.reply() }
    }

    struct Captured { record: Record, pixels: Vec<u8>, tree: Vec<u8> }
    fn read_capture(path: &Path, id: u64) -> Result<Captured, capture::WindowError> {
        let attempt = || -> Result<Captured, capture::WindowError> {
            let mut arena = Arena::open(path).map_err(capture::WindowError::arena)?;
            let record = capture::window_result(&mut arena, "race", id)?;
            let slot = |which| match arena.read(which) {
                Ok(SlotRead::Final { record, bytes }) if !record.oversize => Ok(bytes),
                other => Err(capture::WindowError::arena(format!("race slot incomplete: {other:?}"))),
            };
            Ok(Captured { record, pixels: slot(Slot::Frame)?, tree: slot(Slot::Tree)? })
        };
        let result = attempt();
        let _ = std::fs::remove_file(path);
        result
    }
    struct Reader {
        jobs: Option<mpsc::Sender<(PathBuf, u64)>>,
        results: mpsc::Receiver<Result<Captured, capture::WindowError>>,
        thread: Option<thread::JoinHandle<()>>,
    }
    impl Reader {
        fn new() -> Self {
            let (jobs, input) = mpsc::channel::<(PathBuf, u64)>();
            let (output, results) = mpsc::channel();
            let thread = thread::spawn(move || {
                while let Ok((path, id)) = input.recv() {
                    if output.send(read_capture(&path, id)).is_err() { break; }
                }
            });
            Self { jobs: Some(jobs), results, thread: Some(thread) }
        }
        fn start(&self, path: PathBuf, id: u64) -> Result<(), String> {
            self.jobs.as_ref().unwrap().send((path, id)).map_err(|e| e.to_string())
        }
        fn finish(&self) -> Result<Result<Captured, capture::WindowError>, String> {
            self.results.recv_timeout(Duration::from_secs(2)).map_err(|e| e.to_string())
        }
    }
    impl Drop for Reader {
        fn drop(&mut self) { drop(self.jobs.take()); if let Some(thread) = self.thread.take() { let _ = thread.join(); } }
    }
    fn destination(root: &Path, name: &str) -> Result<PathBuf, String> {
        let path = root.join(name);
        let mode = libbreenix::gpu::current_mode().map_err(|e| e.to_string())?;
        drop(Arena::create(&path, Layout::for_mode(mode.width, mode.height)?)?);
        Ok(path)
    }
    struct Stall { before: u64, fired: u64, release_before: u64, began: Instant }
    impl Stall {
        fn arm(id: u64) -> Result<Self, String> {
            let before = tc::fired(tc_point::CAPTURE_STALL).map_err(|e| e.to_string())?;
            let release_before = tc::fired(tc_point::RELEASE_STALL).map_err(|e| e.to_string())?;
            let began = Instant::now();
            tc::arm(tc_point::CAPTURE_STALL, id, 0).map_err(|e| e.to_string())?;
            Ok(Self { before, fired: before, release_before, began })
        }
        fn wait(&mut self) -> Result<(), String> {
            loop {
                self.fired = tc::fired(tc_point::CAPTURE_STALL).map_err(|e| e.to_string())?;
                if self.fired > self.before { return Ok(()); }
                if self.began.elapsed() >= Duration::from_millis(900) { return Err("capture-stall did not fire within 900 ms".into()); }
                thread::sleep(Duration::from_millis(1));
            }
        }
        fn release(&self) -> Result<(), String> {
            tc::arm(tc_point::RELEASE_STALL, 0, 0).map_err(|e| e.to_string())?;
            // The kernel's stall self-releases at one second. A late action is
            // a failed race even if it happens to return the expected pixels.
            if self.began.elapsed() >= Duration::from_secs(1) { return Err("capture action exceeded the stall lifetime".into()); }
            Ok(())
        }
        fn judge(&self, got: &Captured, expected: Option<Commit>) -> Result<Generation, String> {
            let released = tc::fired(tc_point::RELEASE_STALL).map_err(|e| e.to_string())?;
            if released <= self.release_before { return Err("capture release did not fire".into()); }
            if let Some(expected) = expected {
                if (got.record.window, got.record.width, got.record.height) != (expected.id, expected.width, expected.height) {
                    return Err("capture changed the pinned window extent".into());
                }
            }
            capture_race::judge(self.before, self.fired, &got.pixels, got.record.width, got.record.height,
                &got.tree, got.record.commit, expected.map(|c| c.generation))
        }
    }
    impl Drop for Stall {
        fn drop(&mut self) { let _ = tc::arm(tc_point::RELEASE_STALL, 0, 0); let _ = tc::disarm_all(); }
    }

    fn overlap(app: &mut App, reader: &Reader, root: &Path, old: Commit, action: &str, second_reader: bool) -> Result<Commit, String> {
        let path = destination(root, "race-a.arena")?;
        let other_path = if second_reader { Some(destination(root, "race-b.arena")?) } else { None };
        let mut stall = Stall::arm(old.id)?;
        reader.start(path, old.id)?;
        stall.wait()?;
        // Sending without waiting lets B's CAPTURE overlap the producer's commit;
        // A is already inside its forced copy in either ordering of B and commit.
        app.send(action)?;
        let other = other_path.as_ref().map(|path| read_capture(path, old.id));
        let next = app.reply();
        let release = stall.release();
        let got = reader.finish()?;
        release?;
        let next = next?;
        let got = got.map_err(|e| e.to_string())?;
        stall.judge(&got, Some(old))?;
        if let Some(other) = other {
            let other = other.map_err(|e| e.to_string())?;
            if other.record.window != old.id || (other.record.width, other.record.height) != (old.width, old.height) {
                return Err("reader B changed the window extent".into());
            }
            let generation = stall.judge(&other, None)?;
            if generation != old.generation && generation != next.generation { return Err("reader B returned an unproduced generation".into()); }
        }
        Ok(next)
    }
    fn free_kb() -> Result<u64, String> {
        let text = std::fs::read_to_string("/proc/meminfo").map_err(|e| e.to_string())?;
        text.lines().find_map(|line| line.strip_prefix("MemFree:").and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse().ok())).ok_or_else(|| "MemFree missing".into())
    }
    fn destroyed_before_copy(root: &Path, reader: &Reader, old: Commit) -> Result<(), String> {
        reader.start(destination(root, "race-gone.arena")?, old.id)?;
        match reader.finish()? {
            Err(capture::WindowError { syscall: Some(libbreenix::error::Error::Os(libbreenix::errno::Errno::ENOENT)), .. }) => Ok(()),
            Err(other) => Err(format!("capture before copy needs ENOENT: {other}")),
            Ok(_) => Err("capture after destroy returned pixels".into()),
        }
    }
    fn run(kind: u8) -> Result<(), String> {
        let mut session = Session::connect()?;
        let root = PathBuf::from(session.config_root());
        let mut app = App::launch(&mut session)?;
        let reader = Reader::new();
        let result = (|| {
            let first = app.command("create")?;
            match kind {
                0 | 1 => { overlap(&mut app, &reader, &root, first, "commit 0", kind == 1)?; }
                2 => {
                    let next = overlap(&mut app, &reader, &root, first, "resize", false)?;
                    if (next.width, next.height, next.generation.commit) != (160, 100, first.generation.commit + 1) {
                        return Err("resize did not commit the new extent".into());
                    }
                }
                3 => {
                    let round = |app: &mut App, old| -> Result<(), String> {
                        overlap(app, &reader, &root, old, "destroy", false)?;
                        destroyed_before_copy(&root, &reader, old)
                    };
                    round(&mut app, first)?;
                    // Match windows_capture_across_destroy's three warm-up and
                    // twenty measured rounds, with its 64 KiB free-memory bound.
                    for _ in 1..3 { let old = app.command("create")?; round(&mut app, old)?; }
                    let _ = free_kb()?;
                    let before = free_kb()?;
                    for _ in 0..20 { let old = app.command("create")?; round(&mut app, old)?; }
                    let after = free_kb()?;
                    breenix_suite::suite::value("capture-free-change", after as i64 - before as i64, "kb", None);
                    if after + 64 < before { return Err(format!("capture retained {} KiB across destroy rounds", before - after)); }
                }
                4 => {
                    let second = overlap(&mut app, &reader, &root, first, "commit 1", false)?;
                    let third = overlap(&mut app, &reader, &root, second, "commit 0", false)?;
                    if first.generation.producer == 0 || second.generation.producer == 0
                        || first.generation.producer == second.generation.producer
                        || third.generation.producer != first.generation.producer
                        || second.generation.commit != first.generation.commit + 1
                        || third.generation.commit != second.generation.commit + 1 {
                        return Err("two producers did not alternate thread/ordinal pairs".into());
                    }
                    // Capture the final producer too, pinned across thread 1's commit.
                    overlap(&mut app, &reader, &root, third, "commit 1", false)?;
                }
                _ => unreachable!(),
            }
            app.command("quit")?;
            session.wait(app.pid)?;
            Ok(())
        })();
        // Drop releases any stall before the reader is joined on error paths.
        drop(app); drop(reader);
        let teardown = session.teardown();
        result?; teardown?;
        Ok(())
    }
    pub fn capture_while_drawing() -> crate::frame_case::CaseResult { run(0).map_err(Into::into) }
    pub fn capture_two_readers() -> crate::frame_case::CaseResult { run(1).map_err(Into::into) }
    pub fn capture_resize_race() -> crate::frame_case::CaseResult { run(2).map_err(Into::into) }
    pub fn capture_destroy_race() -> crate::frame_case::CaseResult { run(3).map_err(Into::into) }
    pub fn capture_threads() -> crate::frame_case::CaseResult { run(4).map_err(Into::into) }
}
#[cfg(target_os = "breenix")]
pub use races::{capture_while_drawing, capture_two_readers, capture_resize_race, capture_destroy_race, capture_threads};
