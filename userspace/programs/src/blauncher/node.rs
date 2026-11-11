use super::catalog::{self, Entry, Mode, ParseError};
use serde_json::{json, Value};

pub fn value(entries: &[Entry], errors: &[ParseError], query: &str, selected: usize, limit: usize, focused: bool, font_path: &str, font_size: f32) -> Value {
    let rows: Vec<_> = catalog::filter(entries, query).into_iter().take(limit).map(|entry| json!({
        "name": entry.name, "description": entry.description,
        "mode": match entry.mode { Mode::Gui => "gui", Mode::Terminal => "terminal" },
    })).collect();
    let empty = if rows.is_empty() { Some("No matches") } else { None };
    let errors: Vec<_> = errors.iter().map(|error| json!({"line": error.line, "reason": error.reason})).collect();
    json!({"query": query, "focused": if focused { Some("query") } else { None }, "selected": selected, "rows": rows, "errors": errors, "empty": empty, "font": {"path": font_path, "size": font_size}})
}
