"""D4: pin the PID-1 bodies and inventory the admitted GUI role changes."""
import pathlib
import subprocess
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
PATH = 'libs/libbreenix/src/suite.rs'
BASE = 'a0ee971de81168391d0a45f3a21dfd215f72253c'  # result transport, before roles
MAIN = '242f37ad'  # D4's main basis


def historical(commit, path):
    return subprocess.check_output(['git', 'show', f'{commit}:{path}'], cwd=ROOT, text=True)


def item(source, signature):
    start = source.index(signature)
    brace = source.index('{', start)
    depth = 0
    for i in range(brace, len(source)):
        depth += (source[i] == '{') - (source[i] == '}')
        if depth == 0:
            return source[start:i + 1]
    raise AssertionError(signature)


# Coordinator ledger 2026-11-07 15:07 ET (#2786): exact additions for
# start_record and the session boundary from u3-u11; no wildcard replacement.
SESSION_WIRING = {
    'pub fn run_with_finalizer(&self, finalize: impl FnOnce()) -> !': [
        (r'''            Screen::open(self.gui, self.id == "gpu")
        };
        // Final text-panel storage belongs in both memory samples. Each reason
        // already has the serial MSG_MAX bound; keep its slot for the panel's life.
''', r'''            Screen::open(self.gui, self.id == "gpu")
        };
        if self.gui && self.defer_display && self.categories.iter().any(|c| c.cases.iter().any(|case| case.supervisor.is_some())) {
            screen = Screen::open(false, self.id == "gpu");
            // Deferred suites keep their existing quiet case execution, including
            // inner scenarios that start a supervisor inside the case.
            screen.live = false;
        }
        // Final text-panel storage belongs in both memory samples. Each reason
        // already has the serial MSG_MAX bound; keep its slot for the panel's life.
'''),
        (r'''
        let pace_suffix = if pace_ms > 0 { std::format!(" pace_ms={pace_ms}") } else { String::new() };
        emit(&std::format!("SUITE {} START cases={}{}{}", self.id, total, suffix, pace_suffix));
        if let Some(message) = &error {
            emit(&std::format!("SUITE {} ERROR {}", self.id, message));
''', r'''
        let pace_suffix = if pace_ms > 0 { std::format!(" pace_ms={pace_ms}") } else { String::new() };
        emit(&start_record(self.id, total, &suffix, &pace_suffix, proof));
        if let Some(message) = &error {
            emit(&std::format!("SUITE {} ERROR {}", self.id, message));
'''),
        (r'''                if let Some(slot) = panel_reasons.get_mut(c).and_then(|v| v.get_mut(k)) { retain_panel_reason(&mut states[c][k], slot); }
                for checkpoint in screen::collect(self.id, &name) { screen::emit(&checkpoint); }
                if let Some(picture) = intent_picture.as_deref() {
                    intent::show(self.id, &name, picture, intent_pace_ms);
                    if screen.gui { let _ = crate::suite_stage::redraw(); }
                }
                if let Some(observe) = self.case_observer { observe(done, total, &name); }
''', r'''                if let Some(slot) = panel_reasons.get_mut(c).and_then(|v| v.get_mut(k)) { retain_panel_reason(&mut states[c][k], slot); }
                for checkpoint in screen::collect(self.id, &name) { screen::emit(&checkpoint); }
                screen::upload_pending(self.id, &name);
                if let Some(picture) = intent_picture.as_deref() {
                    intent::show(self.id, &name, picture, intent_pace_ms);
                    screen::upload_pending(self.id, &name);
                    if screen.gui { let _ = crate::suite_stage::redraw(); }
                }
                if self.gui && case.supervisor.is_some() {
                    screen.draw(self, &title, &subtitle, &states, Verdict::Running(&progress), &feed, only);
                }
                if let Some(observe) = self.case_observer { observe(done, total, &name); }
'''),
    ],
    'fn run_case(case: &Case, limit_ms: u64, suite: &str, name: &str, gui: bool, screen: &mut Screen, feed: &mut Feed) -> Outcome': [
        (r'''fn run_case(case: &Case, limit_ms: u64, suite: &str, name: &str, gui: bool, screen: &mut Screen, feed: &mut Feed) -> Outcome {
    screen.judged = None;screen.timing=None;
    let mut results = if gui {
        match ResultPipe::new(suite, name, case.checks, case.collects) {
            Ok(pipe) => Some(pipe),
            Err(msg) => return Outcome::Fail { ms: 0, msg: plain(&msg) },
        }
    } else { None };
    // The live strip's copy of the case's records, when the strip is on.
    let live = if screen.live_on() || screen.gui {
        io::pipe2(status_flags::O_CLOEXEC | status_flags::O_NONBLOCK).ok()
    } else {
        None
    };
    let realtime = realtime_offset();
    // Close-on-exec so a case that execs never leaves the report pipe open.
    let (reader, writer) = match io::pipe2(status_flags::O_CLOEXEC) {
        Ok(ends) => ends,
        Err(error) => {
            if let Some((r, w)) = live { let _ = (io::close(r), io::close(w)); }
            return Outcome::Fail { ms: 0, msg: plain(&std::format!("could not start the case: pipe failed: {error}")) };
        }
    };
    JUDGED_RUNNER.store(process::getpid().map_or(0, |pid| pid.raw()) as u64, Ordering::Relaxed);
    JUDGED_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    // A prior interrupted boot may have left this PID/sequence on a reused disk.
    JUDGED_ENABLED.store(screen.gui, Ordering::Relaxed);
    READBACK_ENABLED.store(suite == "gpu", Ordering::Relaxed);
    let _screen_clock = screen::begin(suite);
    // G1-G6 retain the runner owner they were checked against. G7's console
    // cases explicitly start above the kernel console, with no runner owner.
    if screen.gpu.is_some() && name.starts_with("present/") { let _ = graphics::give_back_display(); }
    for path in [judged_path(), std::format!("{}.new", judged_path()), std::format!("{}.time",judged_path()), std::format!("{}.input",judged_path()), std::format!("{}.values",judged_path())].into_iter().filter(|_| judged_transport_enabled(screen.gui, suite)) {
        if let Err(e) = std::fs::remove_file(path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                let _ = (io::close(reader),io::close(writer));
                if let Some((r,w)) = live { let _ = (io::close(r),io::close(w)); }
                return Outcome::Fail{ms:0,msg:std::format!("cannot clear judged transport: {e}")};
            }
        }
    }
    if screen.gui {
        screen.stage_result(crate::suite_stage::begin());
        screen.stage_result(crate::suite_stage::release_for(name));
    }
    let window = if WARMED_WINDOWS.contains(&(suite, name)) {
        match io::pipe2(status_flags::O_CLOEXEC | status_flags::O_NONBLOCK) {
            Ok(ends) => Some(ends),
            Err(error) => {
                let _ = (io::close(reader), io::close(writer));
                if let Some((r, w)) = live { let _ = (io::close(r), io::close(w)); }
                return Outcome::Fail { ms: 0, msg: plain(&std::format!("timed-window pipe failed: {error}")) };
            }
        }
    } else { None };
    // A GUI suite's case tells the runner what it published for the stage's pictures, and
    // waits for the runner to sample it (ruling B36); its live checks ask the runner to draw
    // the stage on the other two pipes.
    let pictures = if screen.gui {
        match (io::pipe2(status_flags::O_CLOEXEC | status_flags::O_NONBLOCK), io::pipe2(status_flags::O_CLOEXEC),
               io::pipe2(status_flags::O_CLOEXEC), io::pipe2(status_flags::O_CLOEXEC)) {
            (Ok(tell), Ok(answer), Ok(request), Ok(drawn)) => Some((tell, answer, request, drawn)),
            (tell, answer, request, drawn) => {
                for (r, w) in [tell, answer, request, drawn].into_iter().flatten() { let _ = (io::close(r), io::close(w)); }
                let _ = (io::close(reader), io::close(writer));
                if let Some((r, w)) = live { let _ = (io::close(r), io::close(w)); }
                if let Some((r, w)) = window { let _ = (io::close(r), io::close(w)); }
                return Outcome::Fail { ms: 0, msg: String::from("could not start the case: the picture pipes failed") };
            }
        }
    } else { None };
    let start = monotonic_ns();
    HOSTKEY_SEQ.fetch_add(RECORDS_PER_CASE as u64, Ordering::Relaxed);
    match process::fork() {
        Ok(ForkResult::Child) => {
            sequence::in_child();
            if let Some(results) = &results { results.in_child(); }
            let _ = io::close(reader);
            if let Some((r, w)) = window {
                let _ = io::close(r);
                CASE_WINDOW_FD.store(w.raw() as i32, Ordering::Relaxed);
            }
            if let Some((live_reader, live_writer)) = live {
                let _ = io::close(live_reader);
                LIVE_FD.store(live_writer.raw() as i32, Ordering::Relaxed);
            }
            if let Some(((tell_r, tell_w), (answer_r, answer_w), (request_r, request_w), (drawn_r, drawn_w))) = pictures {
                let _ = (io::close(tell_r), io::close(answer_w), io::close(request_r), io::close(drawn_w));
                PUBLISHED_FD.store(tell_w.raw() as i32, Ordering::Relaxed);
                SAMPLED_FD.store(answer_r.raw() as i32, Ordering::Relaxed);
                STAGE_REQUEST_FD.store(request_w.raw() as i32, Ordering::Relaxed);
                STAGE_ANSWER_FD.store(drawn_r.raw() as i32, Ordering::Relaxed);
            }
            let initial_limit = if window.is_some() { 120_000 } else { limit_ms };
            let deadline = start.map_or(0, |start| (start + initial_limit as i128 * 1_000_000) as u64);
            CASE_DEADLINE_NS.store(deadline, Ordering::Relaxed);
            let _ = CASE_NAME.set(String::from(name));
            keep_record_output(suite, name);
            detach_transcript();
            let ready = if screen.gui && crate::suite_stage::case_takes_display(name) { graphics::take_over_display() } else { Ok(()) };
            let outcome = match ready.and_then(|()| silence_output()) {
                Ok(()) => run_inline(case),
                Err(error) => Outcome::Fail {
                    ms: 0,
                    msg: plain(&std::format!("could not send the case's output to /dev/null: {error}")),
                },
            };
            let line = report(&outcome);
            let _ = io::write(writer, line.as_bytes());
            screen::finish();
            hand_over_picture();
            process::exit(0)
        }
        Ok(ForkResult::Parent(pid)) => {
            let pid = pid.raw() as i32;
            if let Some(results) = results.as_mut() { results.in_parent(); }
            let _ = io::close(writer);
            if let Some((_, w)) = window { let _ = io::close(w); }
            let live_reader = live.map(|(live_reader, live_writer)| {
                let _ = io::close(live_writer);
                live_reader
            });
            let (tell, answer, stage) = match pictures {
                Some(((tell_r, tell_w), (answer_r, answer_w), (request_r, request_w), (drawn_r, drawn_w))) => {
                    let _ = (io::close(tell_w), io::close(answer_r), io::close(request_w), io::close(drawn_r));
                    (Some(tell_r), Some(answer_w), Some((request_r, drawn_w)))
                }
                None => (None, None, None),
            };
            let mut waited = {
                let mut frame = || {
                    if let Some(fd) = live_reader { feed.drain(fd); }
                    screen.frame(feed);
                };
                wait_exit(pid, start, limit_ms, &mut frame, window.map(|(r, _)| r), tell, Some(reader), stage, results.as_mut())
            };
            if let Waited::Published(message) = waited {
                // The case has reported: its measured window ends before the stage composes.
                let ms = ms_since(start);
                screen.stage_result(crate::suite_stage::end());
                if let Some(p) = published(&message) {
                    screen.stage_result(crate::suite_stage::sample(p));
                }
                if let Some(fd) = answer { let _ = io::write(fd, &[1]); }
                waited = match wait_exit(pid, monotonic_ns(), 5_000, &mut || {}, None, None, None, None, results.as_mut()) {
                    Waited::TimedOut(_) => Waited::TimedOut(ms),
                    other => other,
                };
            }
            for fd in [tell, answer].into_iter().chain(stage.into_iter().flat_map(|(r, w)| [Some(r), Some(w)])).flatten() { let _ = io::close(fd); }
            let outcome = match waited {
                Waited::Exited(status) => exited(status, reader, ms_since(start)),
                Waited::Published(_) => Outcome::Fail { ms: ms_since(start), msg: String::from("the case published two pictures") },
                Waited::TimedOut(ms) => {
                    let note = stop_child(pid);
                    Outcome::Fail { ms, msg: plain(&std::format!("timed out after {limit_ms} ms{note}")) }
                }
                Waited::ClockStopped(ms) => {
                    let note = stop_child(pid);
                    Outcome::Fail {
                        ms,
                        msg: plain(&std::format!("the monotonic clock stopped {ms} ms into the case; it was killed{note}")),
                    }
                }
                Waited::Failed(error) => {
                    let note = stop_child(pid);
                    Outcome::Fail { ms: ms_since(start), msg: plain(&std::format!("waitpid failed: {error}{note}")) }
                }
            };
            if screen.gui {
                screen.stage_result(crate::suite_stage::end());
                screen.stage_result(crate::suite_stage::restore_after(name));
            }
            let _ = io::close(reader);
            if let Some((r, _)) = window { let _ = io::close(r); }
            if let Some(fd) = live_reader {
                feed.drain(fd);
                let _ = io::close(fd);
            }
            let outcome = match sweep_for_role(gui) {
                Some(note) => with_note(outcome, note),
                None => outcome,
            };
            let outcome = if let Some(results) = results.as_mut() { results.finish(outcome) } else { outcome };
            let outcome = match collect_judged() {
                Ok(frame) => { screen.judged = frame.map(std::sync::Arc::new); outcome }
                Err(note) => with_note(outcome, &note),
            };
            let outcome=match collect_timing(){Ok(timing)=>{screen.timing=timing;outcome},Err(note)=>with_note(outcome,&note)};
            match restore_realtime(realtime) {
                Some(note) => with_note(outcome, &note),
                None => outcome,
            }
        }
        Err(error) => {
            let _ = io::close(reader);
            let _ = io::close(writer);
            if let Some((r, w)) = window { let _ = (io::close(r), io::close(w)); }
            if let Some((r, w)) = live { let _ = (io::close(r), io::close(w)); }
            if let Some((a, b, c, d)) = pictures {
                for (r, w) in [a, b, c, d] { let _ = (io::close(r), io::close(w)); }
            }
            Outcome::Fail { ms: 0, msg: plain(&std::format!("could not start the case: fork failed: {error}")) }
        }
    }
}''', r'''fn run_case(case: &Case, limit_ms: u64, suite: &str, name: &str, gui: bool, screen: &mut Screen, feed: &mut Feed) -> Outcome {
    let snapshot = if gui {
        match config::Snapshot::capture() {
            Ok(snapshot) => Some(snapshot),
            Err(msg) => return Outcome::Fail { ms: 0, msg },
        }
    } else { None };
    let session = gui && case.supervisor.is_some();
    let outcome = (|| {
        if session {
            if let Err(msg) = screen.release_for_session() { return Outcome::Fail { ms: 0, msg }; }
        }
        let outcome = run_case_body(case, limit_ms, suite, name, gui, screen, feed);
        if session {
            if let Err(note) = screen.reclaim_after_session() { return with_note(outcome, &note); }
        }
        outcome
    })();
    match snapshot.map(config::Snapshot::verify_restore).transpose() {
        Err(note) => match outcome {
            Outcome::Pass { ms } => Outcome::Fail { ms, msg: plain(&note) },
            Outcome::Fail { ms, msg } => with_note(Outcome::Fail { ms, msg: note }, &msg),
            Outcome::Skip { msg } => with_note(Outcome::Fail { ms: 0, msg: note }, &msg),
        },
        Ok(_) => outcome,
    }
}'''),
    ],
}


def session_wiring(test, expected):
    signature = expected.split("{")[0].strip()
    for old, new in SESSION_WIRING[signature]:
        test.assertEqual(expected.count(old), 1)
        expected = expected.replace(old, new)
    return expected


class RunnerRoles(unittest.TestCase):
    def test_pid_one_bodies_equal_main(self):
        source = (ROOT / PATH).read_text()
        main = historical(MAIN, PATH)
        for signature in ['fn sweep()', 'fn idle(']:
            self.assertEqual(item(source, signature), item(main, signature))
        seq = 'libs/libbreenix/src/suite_sequence.rs'
        self.assertEqual((ROOT / seq).read_text(), historical(MAIN, seq))

    def test_only_admitted_role_branches_change_the_runner(self):
        source = (ROOT / PATH).read_text()
        base = historical(BASE, PATH)
        expected = item(base, 'pub fn run_with_finalizer(').replace(
            'let mut result_table_error = None;',
            'let scoped = scoped_gui(self.gui);\n        let mut result_table_error = None;\n        if scoped { result_table_error = start_scoped_runner().err(); }')
        expected = expected.replace(
            'if self.defer_display { screen = Screen::open(false, self.id == "gpu"); }',
            'if scoped { process::exit(1); }\n            if self.defer_display { screen = Screen::open(false, self.id == "gpu"); }', 1)
        expected = expected.replace('finalize();\n        feed.finish(',
            'finalize();\n        if scoped { process::exit(if counts.failed == 0 && reclaimed { 0 } else { 1 }); }\n        feed.finish(')
        # G2: this unmerged inventory also includes the deferred-display repair.
        # Suite::defer_display promises no display until the final panel; GUI
        # metadata keeps transport, while Screen.gui controls visual stages.
        for old, new in [
            ('let textual = !self.gui &&', 'let textual = !screen.gui &&'),
            ('let pace_ms = if self.gui {', 'let pace_ms = if screen.gui {'),
            ('let baseline_ready = if recording && self.gui {', 'let baseline_ready = if recording && screen.gui {'),
            ('if recording && self.gui {\n            crate::suite_stage::init(self);',
             'if recording && screen.gui {\n            crate::suite_stage::init(self);'),
            ('if !self.gui { screen.judged = None; }', 'if !screen.gui { screen.judged = None; }'),
            ('if self.gui {\n                    // Use the exact emitted VALUE records',
             'if screen.gui {\n                    // Use the exact emitted VALUE records'),
            ('if !self.gui && screen.fb.is_none()', 'if !screen.gui && screen.fb.is_none()'),
            ('if self.gui { let _ = crate::suite_stage::redraw(); }', 'if screen.gui { let _ = crate::suite_stage::redraw(); }'),
            ('if self.gui { feed = Feed::default(); }', 'if screen.gui { feed = Feed::default(); }'),
            ('let retired = if self.gui {', 'let retired = if screen.gui {'),
            ('if !self.gui { screen.draw(self, &title, &subtitle, &states, verdict, &feed, only); }',
             'if !screen.gui { screen.draw(self, &title, &subtitle, &states, verdict, &feed, only); }'),
        ]:
            self.assertEqual(expected.count(old), 1)
            expected = expected.replace(old, new)
        self.assertEqual(item(source, 'pub fn run_with_finalizer('), session_wiring(self, expected))
        expected = item(base, 'fn run_case(').replace('match sweep()', 'match sweep_for_role(gui)')
        expected = expected.replace('keep_record_output(suite, name);',
                                    'keep_record_output(suite, name);\n            detach_transcript();')
        self.assertEqual(item(source, 'fn run_case('), session_wiring(self, expected))
        for signature in ['pub(super) fn emit(', 'fn keep_record_output(']:
            self.assertEqual(item(source, signature), item(base, signature).replace('Fd::STDOUT', 'transcript_output()'))
        self.assertEqual(item(source, 'fn scoped_gui('), '''fn scoped_gui(gui: bool) -> bool {
    gui && process::getpid().is_ok_and(|pid| pid.raw() != 1)
}''')
        self.assertEqual(item(source, 'fn sweep_for_role('), '''fn sweep_for_role(gui: bool) -> Option<&'static str> {
    if scoped_gui(gui) { descendants::sweep_descendants() } else { sweep() }
}''')
        # New role queries live at this gate only, beyond the two pre-existing PID-1 queries.
        self.assertEqual(source.count('process::getpid()'), base.count('process::getpid()') + 1)
        start = item(source, 'fn start_scoped_runner(')
        self.assertIn('process::set_child_subreaper(true)', start)
        self.assertIn('BREENIX_SUITE_TRANSCRIPT_FD', start)
        self.assertIn('FD_CLOEXEC', start)
        self.assertEqual(source.count('start_scoped_runner()'), 2)
        self.assertEqual(source.count('scoped_gui('), 3)
        self.assertEqual(source.count('descendants::sweep_descendants()'), 1)
        # Mandatory result transport remains table-gated, including deferred displays.
        self.assertIn('if self.gui {\n            for category in self.categories', source)
        self.assertIn('let mut results = if gui {', source)
        self.assertEqual(source.count('ResultPipe::new('), 1)


if __name__ == '__main__':
    unittest.main()
