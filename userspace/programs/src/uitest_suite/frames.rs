// Guest capture race cases.
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

    fn overlap(app: &mut App, reader: &Reader, root: &Path, old: Commit, action: &str, witnessed: &mut [i64; 2]) -> Result<Commit, String> {
        let path = destination(root, "race-a.arena")?;
        let mut stall = Stall::arm(old.id)?;
        reader.start(path, old.id)?;
        stall.wait()?;
        // The capture is stalled inside its copy while the producer acts.
        app.send(action)?;
        let next = app.reply();
        let release = stall.release();
        let got = reader.finish()?;
        release?;
        let next = next?;
        let got = got.map_err(|e| e.to_string())?;
        stall.judge(&got, Some(old))?;
        capture_race::record_witness(stall.before, stall.fired, stall.release_before,
            tc::fired(tc_point::RELEASE_STALL).map_err(|e| e.to_string())?,
            |label, value| witnessed[usize::from(label == "capture-release-fired")] += value)?;
        Ok(next)
    }
    fn run(kind: u8) -> Result<(), String> {
        let mut session = Session::connect()?;
        let root = PathBuf::from(session.config_root());
        let mut app = App::launch(&mut session)?;
        let reader = Reader::new();
        let mut witnessed = [0; 2];
        let result: Result<(), String> = (|| {
            let first = app.command("create")?;
            match kind {
                0 => { overlap(&mut app, &reader, &root, first, "commit 0", &mut witnessed)?; }
                2 => {
                    let next = overlap(&mut app, &reader, &root, first, "resize", &mut witnessed)?;
                    if (next.width, next.height, next.generation.commit) != (160, 100, first.generation.commit + 1) {
                        return Err("resize did not commit the new extent".into());
                    }
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
        for (label, value) in ["capture-stall-fired", "capture-release-fired"].into_iter().zip(witnessed) {
            breenix_suite::suite::value(label, value, "count", None);
        }
        Ok(())
    }
    pub fn capture_while_drawing() -> breenix_suite::suite::CaseResult { run(0).map_err(Into::into) }
    pub fn capture_resize_race() -> breenix_suite::suite::CaseResult { run(2).map_err(Into::into) }
}
#[cfg(target_os = "breenix")]
pub use races::{capture_while_drawing, capture_resize_race};
