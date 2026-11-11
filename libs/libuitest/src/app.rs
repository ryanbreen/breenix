//! Owned application sessions for short UI scenarios.
pub const WATCHDOG_NS: u64 = 10_000_000_000;
pub fn matches_row(rows: &[&str], expected: &str) -> bool {
    rows.iter().any(|row| row.trim() == expected)
}
/// Convert a node-local point to window coordinates only if it is visible.
pub fn drag_start(node: &libbui::tree::Node, from: (i32, i32)) -> Result<(i32, i32), String> {
    use libbui::tree::Flag;
    let [x, y, w, h] = node.bounds;
    let [cx, cy, cw, ch] = node.clip;
    let px = i128::from(x) + i128::from(from.0);
    let py = i128::from(y) + i128::from(from.1);
    if !node.flags.contains(&Flag::Visible) || w <= 0 || h <= 0 || cw <= 0 || ch <= 0
        || from.0 < 0 || from.1 < 0 || i128::from(from.0) >= i128::from(w)
        || i128::from(from.1) >= i128::from(h) || px < i128::from(cx) || py < i128::from(cy)
        || px >= i128::from(cx) + i128::from(cw) || py >= i128::from(cy) + i128::from(ch) {
        return Err(format!("drag start outside visible node {:?}", node.key));
    }
    Ok((i32::try_from(px).map_err(|_| "drag x outside pointer range")?,
        i32::try_from(py).map_err(|_| "drag y outside pointer range")?))
}
pub fn deadline_expired(start: u64, now: u64) -> bool {
    now.saturating_sub(start) >= WATCHDOG_NS
}
pub fn timeout_message(expected: &str, rows: &[&str]) -> String {
    format!("expect-text hang watchdog: expected row={expected:?}; last rows={rows:?}")
}

pub fn node_timeout_message(key: &str, last: Option<&serde_json::Value>) -> String {
    format!("wait-node hang watchdog: key={key:?}; last value={last:?}")
}

/// Closing input has two durable witnesses: targeted delivery and supervised exit.
pub fn exit_input_verdict(disposition: &str, window: u64, wanted: u64, status: i32, expected: i32) -> Result<(), String> {
    if disposition != "delivered" || window != wanted {
        return Err(format!("exit input disposition={disposition} window={window} expected={wanted}"));
    }
    if !(0..=255).contains(&expected) || status != expected << 8 {
        return Err(format!("exit input status={status} expected={expected}"));
    }
    Ok(())
}

/// A background press is handled by bwm and must still cause a clean exit.
pub fn desktop_exit_input_verdict(disposition: &str, window: u64, status: i32) -> Result<(), String> {
    if disposition == "wm-handled" && window == 0 && status == 0 { Ok(()) }
    else { Err(format!("desktop exit disposition={disposition} window={window} status={status}")) }
}

/// After a witnessed exit, a release can have no focused client left.
pub fn exit_release_verdict(disposition: &str, window: u64, reason: &str) -> Result<(), String> {
    if (disposition == "delivered" && window != 0)
        || (disposition == "wm-handled" && window == 0)
        || (disposition == "rejected" && window == 0 && reason == "no-focus") {
        Ok(())
    } else {
        Err(format!("exit release disposition={disposition} window={window} reason={reason}"))
    }
}

/// Expected statuses are raw wait statuses already witnessed before teardown.
pub fn finish_verdict(tracked: &[(u64, Option<i32>)], exits: &[(u64, i32)]) -> Result<(), String> {
    for &(pid, expected) in tracked {
        let status = exits.iter().find(|&&(id, _)| id == pid)
            .map(|&(_, status)| status).ok_or_else(|| format!("app exit not witnessed pid={pid}"))?;
        if expected.map_or(status != 9, |wanted| status != wanted) {
            return Err(format!("unexpected app exit pid={pid} status={status} expected={expected:?}"));
        }
    }
    Ok(())
}

#[cfg(target_os = "breenix")]
mod live {
    use super::*;
    use crate::{input::{Backend, Keyboard, KeyboardBackend, LivePointer, Modifier}, tree::{self, Query, Role, Tree}, Session};
    use breenix_suite::suite::control::{Wait, Watchdog};
    use libbreenix::{input::key, time};

    pub struct App { keys: Keyboard<LivePointer<'static>>, launched: Option<u64>, tracked: Vec<(u64, Option<i32>)>, window: u64 }
    impl App {
        pub fn launch(argv: &[&[u8]], title: &str) -> Result<Self, String> {
            Self::launch_as(argv, title, 0)
        }
        pub fn launch_as(argv: &[&[u8]], title: &str, uid: u32) -> Result<Self, String> {
            if argv.is_empty() || argv[0].is_empty() { return Err("app argv is empty".into()); }
            let mut live = LivePointer::owned(Session::desktop()?)?;
            let pid = live.session().launch_app(argv, uid)?;
            let watchdog = Watchdog::new(Wait::AppOutput)?;
            let window = live.session().wait_window_until(title, watchdog.deadline_ns())?.id;
            live.watch(window)?;
            live.activate(window)?;
            Ok(Self { keys: Keyboard::new(live), launched: Some(pid), tracked: vec![(pid, None)], window })
        }
        pub fn hotkey(keys: &[u16], title: &str) -> Result<Self, String> {
            let mut live = LivePointer::owned(Session::desktop()?)?;
            for &code in keys {
                live.wm_key(code, true)?;
                live.wm_key(code, false)?;
            }
            let watchdog = Watchdog::new(Wait::AppOutput)?;
            let window = live.session().wait_window_until(title, watchdog.deadline_ns())?.id;
            let pid = u64::from(live.info(window)?.owner);
            live.watch(window)?;
            Ok(Self { keys: Keyboard::new(live), launched: Some(pid), tracked: vec![(pid, None)], window })
        }
        /// Start another app without consuming a second per-case session.
        pub fn launch_next(&mut self, argv: &[&[u8]], title: &str) -> Result<(), String> {
            if argv.is_empty() || argv[0].is_empty() { return Err("app argv is empty".into()); }
            let pid = self.keys.backend_mut().session().launch_app(argv, 0)?;
            self.tracked.push((pid, None));
            self.launched = Some(pid);
            self.attach(title)
        }
        pub fn attach(&mut self, title: &str) -> Result<(), String> {
            let live = self.keys.backend_mut();
            let watchdog = Watchdog::new(Wait::AppOutput)?;
            let window = live.session().wait_window_until(title, watchdog.deadline_ns())?.id;
            let pid = u64::from(live.info(window)?.owner);
            live.watch(window)?;
            live.activate(window)?;
            if !self.tracked.iter().any(|&(id, _)| id == pid) { self.tracked.push((pid, None)); }
            self.launched = Some(pid);
            self.window = window;
            Ok(())
        }
        pub fn window(&self) -> u64 { self.window }
        pub fn config_root(&mut self) -> &str { self.keys.backend_mut().session().config_root() }
        /// Advance the frozen UI clock and wait for participant acknowledgments.
        pub fn step(&mut self, dt_ms: u64) -> Result<(), String> {
            self.keys.backend_mut().session().step(dt_ms).map(|_| ())
        }
        pub fn type_text(&mut self, text: &str) -> Result<(), String> {
            self.keys.backend_mut().text(text, self.window).map(|_| ())
        }
        /// A key whose action destroys its window. Delivery and exit replace a
        /// post-action frame; ordinary key() still requires a processed commit.
        pub fn key_exit(&mut self, code: u16, status: i32) -> Result<(), String> {
            self.input_exit(code, false, status, false)
        }
        /// Click at content coordinates, including an app's dismissal margin.
        pub fn click_exit(&mut self, x: i32, y: i32, status: i32) -> Result<(), String> {
            let live = self.keys.backend_mut();
            let info = live.info(self.window)?;
            live.move_to((info.x + x, info.y + y))?;
            self.input_exit(key::BUTTON_LEFT, true, status, false)
        }
        /// Click the desktop to the left of this window, outside its content.
        pub fn click_desktop_exit(&mut self) -> Result<(), String> {
            let live = self.keys.backend_mut();
            let info = live.info(self.window)?;
            let x = info.x.checked_sub(24).filter(|x| *x >= 0).ok_or("no desktop left of window")?;
            live.move_to((x, info.y + 80))?;
            self.input_exit(key::BUTTON_LEFT, true, 0, true)
        }
        fn input_exit(&mut self, code: u16, pointer: bool, expected: i32, desktop: bool) -> Result<(), String> {
            let pid = self.launched.ok_or("no launched app")?;
            let live = self.keys.backend_mut();
            let press = if pointer { live.send_button(code, true) } else { live.send_key(code, true) };
            let result: Result<i32, String> = (|| {
                let event = press?;
                crate::input::wait_pointer_idle(event.serial)?;
                let ledger = std::fs::read_to_string("/proc/breenix/input-ledger").map_err(|e| e.to_string())?;
                let row = crate::input::ledger_entry(&ledger, event.serial)?.ok_or("exit input delivery missing")?;
                let exit = live.session().wait_exit(pid)?;
                if desktop { desktop_exit_input_verdict(row.disposition, row.window, exit.status)?; }
                else { exit_input_verdict(row.disposition, row.window, self.window, exit.status, expected)?; }
                Ok(exit.status)
            })();
            // Focus can belong to the launched child by now. A release needs a
            // terminal disposition, not a commit from the destroyed old window.
            let release = (|| {
                let event = if pointer { live.send_button(code, false)? } else { live.send_key(code, false)? };
                let watchdog = Watchdog::new(Wait::InputAck)?;
                loop {
                    let ledger = std::fs::read_to_string("/proc/breenix/input-ledger").map_err(|e| e.to_string())?;
                    if let Some(row) = crate::input::ledger_entry(&ledger, event.serial)? {
                        return exit_release_verdict(row.disposition, row.window, row.reason);
                    }
                    if now()? >= watchdog.deadline_ns() { return Err(format!("exit release missing serial={}", event.serial)); }
                    time::sleep_ms(1).map_err(|e| e.to_string())?;
                }
            })();
            let status = result?;
            release?;
            self.tracked.iter_mut().find(|(id, _)| *id == pid).ok_or("launched app not tracked")?.1 = Some(status);
            Ok(())
        }
        pub fn wait_node(&mut self, key: &str, predicate: impl Fn(&serde_json::Value) -> bool) -> Result<(), String> {
            let watchdog = Watchdog::new(Wait::AppOutput)?;
            let mut last = None;
            loop {
                if now()? >= watchdog.deadline_ns() { return Err(node_timeout_message(key, last.as_ref())); }
                let tree = self.tree()?;
                let matches: Vec<_> = core::iter::once(&tree.root).chain(tree.children.iter()).filter(|node| node.key == key).collect();
                if matches.len() > 1 { return Err(format!("wait-node ambiguous key={key:?}")); }
                if let Some(node) = matches.first() { last = Some(node.value.clone()); }
                if now()? >= watchdog.deadline_ns() { return Err(node_timeout_message(key, last.as_ref())); }
                if matches.first().is_some_and(|node| predicate(&node.value)) { return Ok(()); }
                time::sleep_ms(10).map_err(|e| e.to_string())?;
            }
        }
        pub fn type_line(&mut self, text: &str) -> Result<(), String> {
            // Submission can open a window and change focus before a physical
            // Enter release. Keep the newline in the same terminal text source.
            self.keys.backend_mut().text(&format!("{text}\n"), self.window).map(|_| ())
        }
        pub fn key(&mut self, code: u16, mods: &[Modifier]) -> Result<(), String> {
            self.keys.chord(self.window, mods, code).map(|_| ())
        }
        /// For a blocking action, wait for its committed result before checking
        /// the initiating key. Subsequent input still has the normal 2 s bound.
        pub fn key_wait_node(&mut self, code: u16, mods: &[Modifier], node: &str,
            predicate: impl Fn(&serde_json::Value) -> bool) -> Result<(), String> {
            if !(1..=255).contains(&code) || mods.iter().enumerate().any(|(i, m)|
                m.code() == code || mods[..i].contains(m)) {
                return Err("input chord has invalid or duplicate keys".into());
            }
            let mut held = Vec::new();
            let result = (|| {
                for key in mods.iter().map(|m| m.code()).chain([code]) {
                    held.push(key);
                    let event = self.keys.backend_mut().send_key(key, true)?;
                    if key == code { self.wait_node(node, &predicate)?; }
                    let delivery = crate::input::delivered(event.serial)?;
                    if delivery.window != self.window { return Err("action key reached another window".into()); }
                }
                Ok(())
            })();
            let mut release = Ok(());
            for key in held.into_iter().rev() {
                let result = self.keys.backend_mut().send_key(key, false)
                    .and_then(|event| crate::input::delivered(event.serial))
                    .and_then(|delivery| if delivery.window == self.window { Ok(()) }
                        else { Err("action release reached another window".into()) });
                if release.is_ok() { release = result; }
            }
            result.and(release)
        }
        /// Drag from a node-local point using relative device-pixel steps.
        pub fn drag(&mut self, key: &str, from: (i32, i32), steps: &[(i32, i32)]) -> Result<(), String> {
            let tree = self.tree()?;
            let node = tree::find(&tree, &Query { key: Some(key), ..Query::default() })?;
            let point = drag_start(node, from)?;
            let live = self.keys.backend_mut();
            let info = live.info(self.window)?;
            let x = info.x.checked_add(point.0).ok_or("drag x outside pointer range")?;
            let y = info.y.checked_add(point.1).ok_or("drag y outside pointer range")?;
            live.move_to((x, y))?;
            let result: Result<(), String> = (|| {
                live.button(true, self.window)?;
                for &step in steps { live.motion(step, self.window)?; }
                Ok(())
            })();
            let release = live.button(false, self.window);
            result?;
            release?;
            Ok(())
        }
        pub fn scroll(&mut self, dy: i32) -> Result<(), String> {
            let live = self.keys.backend_mut();
            let info = live.info(self.window)?;
            live.move_to((info.x + (info.width / 2) as i32, info.y + (info.height / 2) as i32))?;
            live.wheel(dy, self.window).map(|_| ())
        }
        pub fn tree(&mut self) -> Result<Tree, String> { self.keys.backend_mut().tree(self.window) }
        pub fn expect_node(&mut self, key: &str, predicate: impl FnOnce(&serde_json::Value) -> bool) -> Result<(), String> {
            let tree = self.tree()?;
            let node = tree::find(&tree, &Query { key: Some(key), ..Query::default() })?;
            if predicate(&node.value) { Ok(()) }
            else { Err(format!("expect-node key={key:?} value={:?}", node.value)) }
        }
        pub fn expect_text(&mut self, expected: &str) -> Result<(), String> {
            let watchdog = Watchdog::new(Wait::AppOutput)?;
            let start = watchdog.deadline_ns().saturating_sub(WATCHDOG_NS);
            let mut last = Vec::<String>::new();
            let result = (|| {
                loop {
                    if deadline_expired(start, now()?) { return Err("expect-text hang watchdog".to_string()); }
                    let tree = self.tree()?;
                    let node = tree::find(&tree, &Query { role: Some(Role::Terminal), ..Query::default() })?;
                    let lines = node.value["lines"].as_array().ok_or("terminal lines missing")?;
                    last = lines.iter().map(|v| v.as_str().map(String::from).ok_or("terminal row is not text"))
                        .collect::<Result<Vec<_>, _>>()?;
                    if deadline_expired(start, now()?) { return Err("expect-text hang watchdog".into()); }
                    if matches_row(&last.iter().map(String::as_str).collect::<Vec<_>>(), expected) { return Ok(()); }
                    time::sleep_ms(10).map_err(|e| e.to_string())?;
                }
            })();
            result.map_err(|error| format!("{error}; {}", timeout_message(expected, &last.iter().map(String::as_str).collect::<Vec<_>>())))
        }
        /// Resize content through bwm's right and bottom edges, in device pixels.
        pub fn resize(&mut self, width: u32, height: u32) -> Result<(), String> {
            let live = self.keys.backend_mut();
            for bottom in [false, true] {
                let info = live.info(self.window)?;
                let (current, wanted) = if bottom { (info.height, height) } else { (info.width, width) };
                let delta = i32::try_from(i64::from(wanted) - i64::from(current)).map_err(|_| "resize delta outside pointer range")?;
                if delta == 0 { continue; }
                let from = if bottom { (info.x + 80, info.y + info.height as i32 + 1) }
                    else { (info.x + info.width as i32 + 1, info.y + 80) };
                live.move_to(from)?;
                time::sleep_ms(150).map_err(|e| e.to_string())?;
                let result = (|| {
                    live.session().raw_press(key::BUTTON_LEFT)?;
                    time::sleep_ms(150).map_err(|e| e.to_string())?;
                    live.session().raw_motion(if bottom { 0 } else { delta }, if bottom { delta } else { 0 })?;
                    time::sleep_ms(150).map_err(|e| e.to_string())
                })();
                let release = live.session().raw_release(key::BUTTON_LEFT);
                result?; release?;
                let watchdog = Watchdog::new(Wait::AppOutput)?;
                loop {
                    let info = live.info(self.window)?;
                    if now()? >= watchdog.deadline_ns() { return Err(format!("resize hang watchdog: wanted={width}x{height} last={}x{}", info.width, info.height)); }
                    if (if bottom { info.height } else { info.width }) == wanted { break; }
                    time::sleep_ms(10).map_err(|e| e.to_string())?;
                }
            }
            Ok(())
        }
        pub fn finish(mut self) -> Result<(), String> {
            let exits = self.keys.backend_mut().session().teardown()?;
            let statuses: Vec<_> = exits.iter().map(|exit| (exit.pid, exit.status)).collect();
            finish_verdict(&self.tracked, &statuses)
        }
    }
    fn now() -> Result<u64, String> { time::now_monotonic().map(|t| t.as_nanos() as u64).map_err(|e| e.to_string()) }
}
#[cfg(target_os = "breenix")]
pub use live::App;
