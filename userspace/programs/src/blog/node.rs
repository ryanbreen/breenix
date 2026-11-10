use super::tail::Tail;
use serde_json::{json, Value};

pub fn lines(tail: &Tail, offset: usize, visible: usize) -> Value {
    let rows: Vec<_> = (offset..tail.line_count().min(offset.saturating_add(visible)))
        .map(|i| json!({"n": tail.line_number(i), "text": String::from_utf8_lossy(tail.line(i))}))
        .collect();
    json!({"rows": rows, "offset": offset, "total": tail.line_count()})
}

pub fn status(tab: usize, path: &str, follow: bool, open: bool) -> Value {
    json!({"tab": tab, "path": path, "follow": follow, "state": if open { "open" } else { "waiting" }, "search": null})
}
