use breenix_suite::suite::CaseResult;
use libuitest::App;
use std::io::Write;

fn append(path: &str, text: &str) -> Result<(), String> {
    let mut file = std::fs::OpenOptions::new().append(true).open(path).map_err(|e| e.to_string())?;
    file.write_all(text.as_bytes()).map_err(|e| e.to_string())
}

fn rows(lines: &[&str]) -> serde_json::Value {
    serde_json::Value::Array(lines.iter().enumerate().map(|(i, text)|
        serde_json::json!({"n": i + 1, "text": text, "match": false})).collect())
}

pub fn tail() -> CaseResult {
    let path = "/tmp/blog-tail";
    std::fs::write(path, "one\n").map_err(|e| e.to_string())?;
    let mut t = App::launch(&[b"/usr/bin/blog", path.as_bytes()], "Log Viewer")?;
    t.wait_node("lines", |v| v["rows"] == rows(&["one"]) && v["total"] == 1)?;
    append(path, "two\n")?;
    t.wait_node("lines", |v| v["rows"] == rows(&["one", "two"]) && v["total"] == 2)?;
    append(path, "thr")?;
    t.wait_node("lines", |v| v["rows"] == rows(&["one", "two"]) && v["total"] == 2)?;
    append(path, "ee\n")?;
    t.wait_node("lines", |v| v["rows"] == rows(&["one", "two", "three"]) && v["total"] == 3)?;
    append(path, "four\n")?;
    t.wait_node("lines", |v| v["rows"] == rows(&["one", "two", "three", "four"]) && v["total"] == 4)?;
    t.wait_node("status", |v| v["state"] == "open" && v["follow"] == true)?;
    Ok(t.finish()?)
}

pub fn follow() -> CaseResult {
    let path = "/tmp/blog-follow";
    let initial: String = (1..=100).map(|n| format!("line {n}\n")).collect();
    std::fs::write(path, initial).map_err(|e| e.to_string())?;
    let mut t = App::launch(&[b"/usr/bin/blog", path.as_bytes()], "Log Viewer")?;
    t.wait_node("lines", |v| v["total"] == 100 && v["rows"].as_array().is_some_and(|r| r.last() == Some(&serde_json::json!({"n": 100, "text": "line 100", "match": false}))))?;
    append(path, "line 101\n")?;
    t.wait_node("lines", |v| v["total"] == 101 && v["rows"].as_array().is_some_and(|r| r.last() == Some(&serde_json::json!({"n": 101, "text": "line 101", "match": false}))))?;
    t.key(33, &[])?;
    t.wait_node("status", |v| v["follow"] == false && v["state"] == "open")?;
    let offset = std::cell::Cell::new(None);
    t.wait_node("lines", |v| { offset.set(v["offset"].as_u64()); offset.get().is_some() && v["total"] == 101 })?;
    append(path, "line 102\n")?;
    t.wait_node("lines", |v| v["total"] == 102 && v["offset"].as_u64() == offset.get())?;
    Ok(t.finish()?)
}

pub fn truncate_rotate() -> CaseResult {
    let path = "/tmp/blog-truncate-rotate";
    let replacement = "/tmp/blog-truncate-rotate-new";
    std::fs::write(path, "original one\noriginal two\n").map_err(|e| e.to_string())?;
    let mut t = App::launch(&[b"/usr/bin/blog", path.as_bytes()], "Log Viewer")?;
    t.wait_node("lines", |v| v["rows"] == rows(&["original one", "original two"]))?;
    std::fs::write(path, "").map_err(|e| e.to_string())?;
    t.wait_node("lines", |v| v["total"] == 0 && v["rows"] == rows(&[]))?;
    append(path, "rewritten\n")?;
    t.wait_node("lines", |v| v["total"] == 1 && v["rows"] == rows(&["rewritten"]))?;
    std::fs::write(replacement, "rotated\n").map_err(|e| e.to_string())?;
    std::fs::rename(replacement, path).map_err(|e| e.to_string())?;
    t.wait_node("lines", |v| v["total"] == 1 && v["rows"] == rows(&["rotated"]))?;
    append(path, "next\n")?;
    t.wait_node("lines", |v| v["total"] == 2 && v["rows"] == rows(&["rotated", "next"]))?;
    t.wait_node("status", |v| v["state"] == "open")?;
    Ok(t.finish()?)
}

pub fn missing_file() -> CaseResult {
    let path = "/tmp/blog-missing-file";
    match std::fs::remove_file(path) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.to_string().into()),
    }
    let mut t = App::launch(&[b"/usr/bin/blog", path.as_bytes()], "Log Viewer")?;
    t.wait_node("status", |v| v["state"] == "waiting" && v["path"] == path)?;
    std::fs::write(path, "created one\ncreated two\n").map_err(|e| e.to_string())?;
    t.wait_node("status", |v| v["state"] == "open" && v["path"] == path)?;
    t.wait_node("lines", |v| v["total"] == 2 && v["rows"] == rows(&["created one", "created two"]))?;
    Ok(t.finish()?)
}

fn offset(t: &mut App, expected: u64) -> Result<(), String> {
    t.wait_node("lines", |v| v["offset"] == expected && v["rows"].as_array().is_some_and(|rows|
        rows.first().is_some_and(|row| row["n"] == expected + 1)))
}

pub fn navigation() -> CaseResult {
    use libuitest::input::Modifier;
    let path = "/tmp/blog-navigation";
    let lines: String = (1..=200).map(|n| format!("line {n}\n")).collect();
    std::fs::write(path, lines).map_err(|e| e.to_string())?;
    let mut t = App::launch(&[b"/usr/bin/blog", path.as_bytes()], "Log Viewer")?;
    t.wait_node("lines", |v| v["total"] == 200)?;
    t.key(34, &[])?; // g: top
    offset(&mut t, 0)?;
    let visible = std::cell::Cell::new(0u64);
    t.wait_node("lines", |v| { visible.set(v["rows"].as_array().map_or(0, |r| r.len() as u64)); visible.get() > 3 && visible.get() < 100 })?;
    let page = visible.get();
    t.key(36, &[])?; offset(&mut t, 1)?; // j
    t.key(37, &[])?; offset(&mut t, 0)?; // k
    t.key(109, &[])?; offset(&mut t, page)?; // Page Down
    t.key(104, &[])?; offset(&mut t, 0)?; // Page Up
    t.key(34, &[Modifier::Shift])?; offset(&mut t, 200 - page)?; // G
    t.key(34, &[])?; offset(&mut t, 0)?;
    t.scroll(-1)?; offset(&mut t, 3)?;
    t.scroll(1)?; offset(&mut t, 0)?;
    Ok(t.finish()?)
}

pub fn search() -> CaseResult {
    use libuitest::input::Modifier;
    let path = "/tmp/blog-search";
    std::fs::write(path, "hit one\nplain\nhit two\nplain\nhit three\n").map_err(|e| e.to_string())?;
    let mut t = App::launch(&[b"/usr/bin/blog", path.as_bytes()], "Log Viewer")?;
    t.wait_node("lines", |v| v["total"] == 5)?;
    t.key(53, &[])?; // /
    for code in [35, 23, 20] { t.key(code, &[])?; } // hit
    t.key(28, &[])?;
    for (row, code, mods) in [(1, 0, false), (3, 49, false), (5, 49, false), (1, 49, false), (5, 49, true), (3, 49, true), (1, 49, true), (5, 49, true)] {
        if code != 0 { t.key(code, if mods { &[Modifier::Shift] } else { &[] })?; }
        t.wait_node("lines", |v| v["rows"].as_array().is_some_and(|rows| rows.iter().filter(|r| r["match"] == true).count() == 1 && rows.iter().any(|r| r["n"] == row && r["match"] == true)))?;
        t.wait_node("status", |v| v["search"]["query"] == "hit" && v["search"]["matches"] == 3 && v["search"]["row"] == row)?;
    }
    Ok(t.finish()?)
}

struct Blogd(Option<libbreenix::types::Pid>);
impl Blogd {
    fn start(dir: &str, state: &str) -> Result<Self, String> {
        let args: Vec<_> = ["/usr/sbin/blogd", "--dir", dir, "--state", state].iter()
            .map(|s| std::ffi::CString::new(*s).unwrap()).collect();
        let mut pointers: Vec<_> = args.iter().map(|s| s.as_ptr() as *const u8).collect();
        pointers.push(core::ptr::null());
        let pid = libbreenix::process::spawnv(args[0].as_bytes_with_nul(), pointers.as_ptr()).map_err(|e| e.to_string())?;
        Ok(Self(Some(pid)))
    }
    fn stop(&mut self) -> Result<(), String> {
        let pid = self.0.take().ok_or("blogd already stopped")?;
        libbreenix::signal::kill(pid.raw() as i32, libbreenix::signal::SIGKILL).map_err(|e| e.to_string())?;
        let mut status = 0;
        libbreenix::process::waitpid(pid.raw() as i32, &mut status, 0).map_err(|e| e.to_string())?;
        if !libbreenix::process::wifsignaled(status) || libbreenix::process::wtermsig(status) != libbreenix::signal::SIGKILL {
            return Err(format!("blogd did not die by SIGKILL: {status}"));
        }
        Ok(())
    }
}
impl Drop for Blogd {
    fn drop(&mut self) { if self.0.is_some() { let _ = self.stop(); } }
}

fn write_records(nonce: &str, range: core::ops::Range<u32>) -> Result<(), String> {
    let mut kmsg = std::fs::OpenOptions::new().write(true).open("/dev/kmsg").map_err(|e| e.to_string())?;
    for n in range { writeln!(kmsg, "{nonce}-{n:02}").map_err(|e| e.to_string())?; }
    Ok(())
}

fn wait_records(path: &str, nonce: &str, count: usize) -> Result<(), String> {
    let start = libbreenix::time::now_monotonic().map_err(|e| e.to_string())?.as_nanos();
    loop {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        if text.lines().filter(|line| line.contains(nonce)).count() >= count {
            let elapsed = libbreenix::time::now_monotonic().map_err(|e| e.to_string())?.as_nanos() - start;
            println!("blogd wait records={count} elapsed_ms={}", elapsed / 1_000_000);
            return Ok(());
        }
        if libbreenix::time::now_monotonic().map_err(|e| e.to_string())?.as_nanos() - start >= 10_000_000_000 {
            return Err(format!("blogd records timed out: {text}"));
        }
        libbreenix::time::sleep_ms(10).map_err(|e| e.to_string())?;
    }
}

fn blogd_case(restart: bool) -> CaseResult {
    let nonce = format!("blogd-test-{}-{}", std::process::id(), libbreenix::time::now_monotonic().map_err(|e| e.to_string())?.as_nanos());
    let dir = format!("/tmp/{nonce}");
    let state = format!("{dir}/cursor");
    let path = format!("{dir}/kernel.log");
    std::fs::create_dir(&dir).map_err(|e| e.to_string())?;
    let mut child = Blogd::start(&dir, &state)?;
    write_records(&nonce, 0..10)?;
    wait_records(&path, &nonce, 10)?;
    if restart { child.stop()?; }
    write_records(&nonce, 10..20)?;
    if restart { child = Blogd::start(&dir, &state)?; }
    wait_records(&path, &nonce, 20)?;
    child.stop()?;
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let boot = std::fs::read_to_string("/run/breenix/boot-id").map_err(|e| e.to_string())?;
    let header = text.lines().next().ok_or("missing log header")?;
    if !header.split_whitespace().any(|field| field == format!("boot={}", boot.trim())) { return Err(format!("wrong log header: {header}").into()); }
    let records: Vec<_> = text.lines().filter(|line| line.contains(&nonce)).collect();
    let expected: Vec<_> = (0..20).map(|n| format!("{nonce}-{n:02}")).collect();
    if records.len() != 20 || records.iter().zip(&expected).any(|(line, expected)| !line.ends_with(expected)) {
        return Err(format!("blogd records differ: {records:?}").into());
    }
    std::fs::remove_dir_all(dir).map_err(|e| e.to_string())?;
    Ok(())
}
pub fn blogd_records_once() -> CaseResult { blogd_case(false) }
pub fn blogd_restart() -> CaseResult { blogd_case(true) }
