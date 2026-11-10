use breenix_suite::suite::CaseResult;
use libuitest::App;
use std::io::Write;

fn append(path: &str, text: &str) -> Result<(), String> {
    let mut file = std::fs::OpenOptions::new().append(true).open(path).map_err(|e| e.to_string())?;
    file.write_all(text.as_bytes()).map_err(|e| e.to_string())
}

fn rows(lines: &[&str]) -> serde_json::Value {
    serde_json::Value::Array(lines.iter().enumerate().map(|(i, text)|
        serde_json::json!({"n": i + 1, "text": text})).collect())
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
    t.wait_node("lines", |v| v["total"] == 100 && v["rows"].as_array().is_some_and(|r| r.last() == Some(&serde_json::json!({"n": 100, "text": "line 100"}))))?;
    append(path, "line 101\n")?;
    t.wait_node("lines", |v| v["total"] == 101 && v["rows"].as_array().is_some_and(|r| r.last() == Some(&serde_json::json!({"n": 101, "text": "line 101"}))))?;
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
