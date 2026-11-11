use breenix_suite::suite::CaseResult;
use libuitest::{input::Modifier, App};

pub fn init_desktop() -> CaseResult {
    let mut t = App::init_desktop("Terminal")?;
    t.wait_node("grid", |v| v["lines"].as_array().is_some_and(|lines|
        lines.iter().any(|line| line.as_str().is_some_and(|line| line.starts_with("bsh />")))))?;
    let mut windows = vec![libbreenix::window::abi::WinInfo::default(); 256];
    let count = libbreenix::window::list(&mut windows).map_err(|e| e.to_string())?;
    if count > windows.len() { return Err("window list overflow".into()); }
    let terminal = windows[..count].iter().find(|w| w.id == t.window()).ok_or("terminal window missing")?.owner;
    let mut compositor = libbreenix::window::abi::CompositorStatus::default();
    libbreenix::window::call(libbreenix::window::abi::op::COMPOSITOR_STATUS, &mut compositor)
        .map_err(|e| e.to_string())?;
    let identity = |pid: u32| -> Result<(String, u32), String> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|e| e.to_string())?;
        let (_, rest) = stat.split_once('(').ok_or("bad process stat")?;
        let (name, rest) = rest.rsplit_once(") ").ok_or("bad process stat")?;
        let parent = rest.split_whitespace().nth(1).ok_or("missing process parent")?
            .parse().map_err(|_| "invalid process parent")?;
        Ok((name.to_string(), parent))
    };
    let (name, init) = identity(terminal)?;
    let (wm_name, wm_parent) = identity(compositor.owner)?;
    let (init_name, _) = identity(init)?;
    if name != "bterm" || wm_name != "bwm" || init_name != "init" || wm_parent != init {
        return Err(format!("desktop process tree: terminal={name} init={init_name} bwm={wm_name} parent={wm_parent} expected={init}").into());
    }
    t.type_line("echo desktop-ok")?;
    t.expect_text("desktop-ok")?;
    Ok(t.finish()?)
}

pub fn copy_paste() -> CaseResult {
    let mut t = App::launch(&[b"/usr/bin/bterm"], "Terminal")?;
    t.type_line("echo Picker")?;
    t.expect_text("Picker")?;
    let tree = t.tree()?;
    let v = &tree.root.value;
    let row = v["lines"].as_array().ok_or("missing terminal lines")?.iter()
        .position(|line| line.as_str() == Some("Picker")).ok_or("missing output row")?;
    let cell = &v["cell"];
    let number = |key: &str| -> Result<i32, String> {
        i32::try_from(cell[key].as_i64().ok_or_else(|| format!("missing cell {key}"))?)
            .map_err(|_| format!("invalid cell {key}"))
    };
    let (x, y, w, h) = (number("x")?, number("y")?, number("w")?, number("h")?);
    let row = i32::try_from(row).map_err(|_| "terminal row out of range")?;
    t.drag("grid", (x + w / 2, y + row * h + h / 2), &[(5 * w, 0)])?;
    t.wait_node("grid", |v| v["selection"]["text"] == "Picker")?;
    t.key(46, &[Modifier::Ctrl, Modifier::Shift])?;
    t.launch_next(&[b"/usr/bin/blauncher"], "\u{1}Launcher")?;
    t.key(47, &[Modifier::Ctrl])?;
    t.wait_node("apps", |v| v["query"] == "Picker" && v["rows"].as_array()
        .is_some_and(|rows| rows.len() == 1 && rows[0]["name"] == "Font Picker"))?;
    Ok(t.finish()?)
}


pub fn font_everywhere() -> CaseResult {
    use super::bfontpicker::{self, FIRST, PLAIN, FAKE};
    let mut t = bfontpicker::launch()?;
    bfontpicker::seed(&mut t, &format!("mono.font={FIRST}\nmono.size=14\ndisplay.font={FIRST}\ndisplay.size=17\n"))?;
    t.launch_next(&[b"/usr/bin/bterm"], "Terminal")?;
    t.type_line("echo Font comparison 12345")?;
    t.expect_text("Font comparison 12345")?;
    t.wait_node("grid", |v| v["font"]["path"] == FIRST && v["font"]["size"].as_f64() == Some(14.0))?;
    let terminal = t.window();
    let terminal_before = bfontpicker::pixels(terminal)?;
    let log_path = "/tmp/desktop-font-everywhere-log";
    std::fs::write(log_path, "Font comparison 12345\n").map_err(|e| e.to_string())?;
    t.launch_next(&[b"/usr/bin/blog", log_path.as_bytes()], "Log Viewer")?;
    t.wait_node("status", |v| v["font"]["path"] == FIRST && v["font"]["size"].as_f64() == Some(14.0))?;
    t.wait_node("lines", |v| v["total"] == 1)?;
    let blog = t.window();
    let blog_before = bfontpicker::pixels(blog)?;
    t.launch_next(&[b"/usr/bin/guskit", b"--canvas", b"200x120"], "Gus Kit")?;
    t.wait_node("status", |v| v["font"]["path"] == FIRST && v["font"]["size"].as_f64() == Some(17.0))?;
    let guskit = t.window();
    let guskit_before = bfontpicker::pixels(guskit)?;
    t.launch_next(&[b"/usr/bin/blauncher"], "\u{1}Launcher")?;
    t.wait_node("apps", |v| v["font"]["path"] == FIRST && v["font"]["size"].as_f64() == Some(17.0))?;
    let launcher_before = bfontpicker::pixels(t.window())?;
    t.key_exit(1, 0)?;
    t.attach("Font Picker")?;
    bfontpicker::select(&mut t)?;
    t.key(108, &[])?;
    t.wait_node("fonts", |v| v["selected"] == 2 && v["rows"][2]["path"] == FAKE)?;
    t.key(28, &[])?;
    t.wait_node("status", |v| v["display"]["path"] == FAKE && v["save"] == "saved")?;
    t.step(250)?;
    t.attach("Terminal")?;
    t.wait_node("grid", |v| v["font"]["path"] == PLAIN && v["font"]["size"].as_f64() == Some(14.0))?;
    if bfontpicker::pixels(terminal)? == terminal_before { return Err("terminal pixels did not change".into()); }
    t.attach("Log Viewer")?;
    t.wait_node("status", |v| v["font"]["path"] == PLAIN && v["font"]["size"].as_f64() == Some(14.0))?;
    if bfontpicker::pixels(blog)? == blog_before { return Err("blog pixels did not change".into()); }
    t.attach("Gus Kit")?;
    t.wait_node("status", |v| v["font"]["path"] == FAKE && v["font"]["size"].as_f64() == Some(17.0))?;
    if bfontpicker::pixels(guskit)? == guskit_before { return Err("Gus Kit pixels did not change".into()); }
    t.launch_next(&[b"/usr/bin/blauncher"], "\u{1}Launcher")?;
    t.wait_node("apps", |v| v["font"]["path"] == FAKE && v["font"]["size"].as_f64() == Some(17.0))?;
    if bfontpicker::pixels(t.window())? == launcher_before { return Err("launcher pixels did not change".into()); }
    Ok(t.finish()?)
}


pub fn launch_each() -> CaseResult {
    // Keep this in shipped catalog order; the catalog comparison catches additions,
    // removals and mode changes that would otherwise silently escape this case.
    let outcomes = [
        ("Terminal", "gui", "Terminal", ""),
        ("System Check", "gui", "Self-Check", ""),
        ("Log Viewer", "terminal", "Terminal", "bless"),
        ("Bounce", "gui", "Bounce", ""),
        ("Breenix Log", "gui", "Log Viewer", ""),
        ("Font Picker", "gui", "Font Picker", ""),
        ("Gus Kit", "gui", "Gus Kit", ""),
        ("Shell", "terminal", "Terminal", "shell"),
        ("URL Fetch", "terminal", "Terminal", "burl"),
        ("cat", "terminal", "Terminal", "cat"),
        ("ls", "terminal", "Terminal", "exit"),
        ("echo", "terminal", "Terminal", "exit"),
        ("ps", "terminal", "Terminal", "exit"),
        ("Particles", "gui", "Particles", ""),
        ("Confetti", "gui", "Confetti", ""),
        ("Demo", "gui", "Demo", ""),
        ("Rectangles", "gui", "Rectangles", ""),
        ("Breenix Icon Kit", "gui", "Breenix Icon Kit", ""),
    ];
    let catalog = std::fs::read_to_string("/usr/share/applications/catalog").map_err(|e| e.to_string())?;
    let entries: Vec<_> = catalog.lines().filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| { let fields: Vec<_> = line.split('\t').collect();
            if fields.len() < 4 { return Err("malformed shipped catalog"); }
            Ok((fields[0], fields[2]))
        }).collect::<Result<_, _>>()?;
    if entries != outcomes.iter().map(|&(name, mode, _, _)| (name, mode)).collect::<Vec<_>>() {
        return Err(format!("shipped catalog differs from launch outcomes: {entries:?}").into());
    }
    let launcher: &[&[u8]] = &[b"/usr/bin/blauncher"];
    let mut t = App::launch(launcher, "\u{1}Launcher")?;
    let mut timings = String::new();
    for (index, &(name, _, title, outcome)) in outcomes.iter().enumerate() {
        let start = libbreenix::time::now_monotonic().map_err(|e| e.to_string())?.as_nanos();
        let result: Result<(), String> = (|| {
            if index != 0 { t.launch_next(launcher, "\u{1}Launcher")?; }
            t.type_text(name)?;
            let tree = t.wait_tree(|tree| tree.root.value["query"] == name &&
                tree.root.value["rows"].as_array().is_some_and(|rows|
                    rows.iter().any(|row| row["name"] == name)))?;
            let rows = tree.root.value["rows"].as_array().ok_or("launcher rows missing")?;
            let selected = rows.iter().position(|row| row["name"] == name).ok_or("entry missing")?;
            for _ in 0..selected { t.key(108, &[])?; }
            t.wait_node("apps", |v| v["selected"].as_u64() == Some(selected as u64) &&
                v["rows"][selected]["name"] == name)?;
            t.key_exit(28, 0)?;
            t.attach(title)?;
            match outcome {
                "exit" => t.expect_text("[exited 0]")?,
                "shell" => t.expect_text("bsh />")?,
                "burl" => { t.expect_text("Usage: burl [OPTIONS] URL")?; t.expect_text("[exited 1]")?; }
                "bless" => t.wait_node("grid", |v| v["lines"].as_array().is_some_and(|lines|
                    lines.iter().any(|line| line.as_str().is_some_and(|line| line.contains("/var/log/kernel.log") && line.contains(" L:")))))?,
                "cat" => {
                    t.type_line("launch-each-cat")?;
                    t.wait_node("grid", |v| v["lines"].as_array().is_some_and(|lines|
                        lines.iter().filter(|line| line.as_str() == Some("launch-each-cat")).count() >= 2))?;
                }
                _ => {}
            }
            t.close_exit()?;
            Ok(())
        })();
        let elapsed = libbreenix::time::now_monotonic().map_err(|e| e.to_string())?.as_nanos().saturating_sub(start);
        timings.push_str(&format!("{name}\telapsed_ms={}\t{}\n", elapsed / 1_000_000, if result.is_ok() { "ok" } else { "failed" }));
        if result.is_err() || index + 1 == outcomes.len() {
            std::fs::write("/tmp/desktop-launch-each-timings", &timings).map_err(|e| e.to_string())?;
            // Selected runs without an evidence route still retain the local file.
            let _ = breenix_suite::suite::upload_diagnostic("gui-apps", "desktop-launch-each-timings", timings.as_bytes());
        }
        result.map_err(|e| format!("{name}: {e}"))?;
    }
    Ok(t.finish()?)
}

fn window_ids() -> Result<std::collections::BTreeSet<u64>, String> {
    let mut windows = vec![libbreenix::window::abi::WinInfo::default(); 256];
    let count = libbreenix::window::list(&mut windows).map_err(|e| e.to_string())?;
    if count > windows.len() { return Err(format!("window list overflow: {count}")); }
    Ok(windows[..count].iter().map(|w| w.id).collect())
}

fn processes() -> Result<std::collections::BTreeMap<u32, String>, String> {
    let mut result = std::collections::BTreeMap::new();
    for entry in std::fs::read_dir("/proc").map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        match std::fs::read_to_string(entry.path().join("stat")) {
            Ok(stat) => { result.insert(pid, stat); }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(result)
}

pub fn crash_isolated() -> CaseResult {
    let mut t = App::launch(&[b"/usr/bin/bterm"], "Terminal")?;
    t.expect_text("bsh />")?;
    t.launch_next(&[b"/usr/bin/guskit", b"--canvas", b"200x120"], "Gus Kit")?;
    t.wait_node("status", |v| v["tool"] == "pencil")?;
    t.launch_next(&[b"/usr/libexec/breenix/ack-app_test", b"--window", b"Crash", b"--fault-key"], "Crash")?;
    let crash = t.window();
    // kernel/src/signal/delivery.rs:1758-1761 and kernel/src/syscall/wait.rs:290-296:
    // SIGSEGV's core-dump signal is returned as raw wait status 0x80 | 11.
    t.key_exit(33, 0x8b)?;
    if window_ids()?.contains(&crash) { return Err("faulted window still registered".into()); }
    t.attach("Terminal")?;
    t.type_line("echo alive")?;
    t.expect_text("alive")?;
    t.attach("Gus Kit")?;
    t.key(48, &[])?;
    t.wait_node("status", |v| v["tool"] == "brush")?;
    Ok(t.finish()?)
}

pub fn close_all_baseline() -> CaseResult {
    let mut t = App::launch(&[b"/usr/libexec/breenix/ack-app_test", b"--window", b"Baseline"], "Baseline")?;
    t.close_exit()?;
    let windows = window_ids()?;
    let baseline = processes()?;
    t.launch_next(&[b"/usr/bin/bterm"], "Terminal")?;
    t.expect_text("bsh />")?;
    t.key(20, &[Modifier::Ctrl])?;
    t.wait_node("grid", |v| v["tab_count"] == 2)?;
    t.expect_text("bsh />")?;
    t.launch_next(&[b"/usr/bin/blog"], "Log Viewer")?;
    t.launch_next(&[b"/usr/bin/guskit", b"--canvas", b"200x120"], "Gus Kit")?;
    t.launch_next(&[b"/usr/bin/bfontpicker"], "Font Picker")?;
    t.launch_next(&[b"/usr/bin/bounce"], "Bounce")?;
    for title in ["Bounce", "Font Picker", "Gus Kit", "Log Viewer", "Terminal"] {
        t.attach(title)?;
        t.close_exit()?;
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let current_windows = window_ids()?;
        let current = processes()?;
        let missing: Vec<_> = baseline.iter().filter(|(pid, _)| !current.contains_key(pid)).map(|(_, stat)| stat.clone()).collect();
        let new: Vec<_> = current.iter().filter(|(pid, _)| !baseline.contains_key(pid)).map(|(_, stat)| stat.clone()).collect();
        if current_windows == windows && missing.is_empty() && new.is_empty() { break; }
        if std::time::Instant::now() >= deadline {
            return Err(format!("baseline mismatch windows={current_windows:?} expected={windows:?} missing_pids={missing:?} survivors={new:?}").into());
        }
        libbreenix::time::sleep_ms(10).map_err(|e| e.to_string())?;
    }
    Ok(t.finish()?)
}
