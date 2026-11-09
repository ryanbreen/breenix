//! Owned application sessions for short UI scenarios.
pub const WATCHDOG_NS: u64 = 10_000_000_000;
pub fn matches_row(rows: &[&str], expected: &str) -> bool {
    rows.iter().any(|row| row.trim() == expected)
}
pub fn deadline_expired(start: u64, now: u64) -> bool {
    now.saturating_sub(start) >= WATCHDOG_NS
}
pub fn timeout_message(expected: &str, rows: &[&str]) -> String {
    format!("expect-text hang watchdog: expected row={expected:?}; last rows={rows:?}")
}

#[cfg(target_os = "breenix")]
mod live {
    use super::*;
    use crate::{input::{Backend, Keyboard, KeyboardBackend, LivePointer, Modifier}, tree::{self, Query, Role, Tree}, Session};
    use breenix_suite::suite::control::{Wait, Watchdog};
    use libbreenix::{input::key, process, signal, time};

    pub struct App { keys: Keyboard<LivePointer<'static>>, pid: u64, window: u64 }
    impl App {
        pub fn launch(argv: &[&[u8]], title: &str) -> Result<Self, String> {
            if argv.is_empty() || argv[0].is_empty() { return Err("app argv is empty".into()); }
            let mut live = LivePointer::owned(Session::desktop()?)?;
            let pid = live.session().launch_app(argv, 0)?;
            let watchdog = Watchdog::new(Wait::AppOutput)?;
            let window = live.session().wait_window_until(title, watchdog.deadline_ns())?.id;
            live.watch(window)?;
            live.activate(window)?;
            Ok(Self { keys: Keyboard::new(live), pid, window })
        }
        pub fn type_line(&mut self, text: &str) -> Result<(), String> {
            self.keys.backend_mut().text(text, self.window)?;
            self.key(key::ENTER, &[])
        }
        pub fn key(&mut self, code: u16, mods: &[Modifier]) -> Result<(), String> {
            self.keys.chord(self.window, mods, code).map(|_| ())
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
            let exit = exits.iter().find(|exit| exit.pid == self.pid).ok_or("app exit not witnessed during teardown")?;
            if !process::wifsignaled(exit.status) || process::wtermsig(exit.status) != signal::SIGKILL {
                return Err(format!("app exited before teardown pid={} status={}", self.pid, exit.status));
            }
            Ok(())
        }
    }
    fn now() -> Result<u64, String> { time::now_monotonic().map(|t| t.as_nanos() as u64).map_err(|e| e.to_string()) }
}
#[cfg(target_os = "breenix")]
pub use live::App;
