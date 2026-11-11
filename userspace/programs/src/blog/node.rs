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

pub fn lines_view(tail: &Tail, offset: usize, visible: usize, matched: Option<usize>) -> Value {
    let mut result = lines(tail, offset, visible);
    if let Some(rows) = result["rows"].as_array_mut() {
        for (i, row) in rows.iter_mut().enumerate() { row["match"] = json!(matched == Some(offset + i)); }
    }
    result
}
pub fn status_view(tab: usize, path: &str, follow: bool, open: bool, query: &str, matched: Option<usize>, matches: usize, tail: &Tail, font_path: &str, font_size: f32) -> Value {
    let mut result = status(tab, path, follow, open);
    result["font"] = json!({"path": font_path, "size": font_size});
    if !query.is_empty() { result["search"] = json!({"query": query, "row": matched.map(|i| tail.line_number(i)), "matches": matches}); }
    result
}
