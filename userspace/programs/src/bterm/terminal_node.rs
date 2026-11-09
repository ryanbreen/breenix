use serde_json::{json, Value};

pub fn terminal_value(cols: usize, rows: usize, cursor: [usize; 2], cells: &[char], title: &str) -> Value {
    let lines: Vec<String> = cells.chunks(cols).take(rows).map(|row| {
        row.iter().collect::<String>().trim_end_matches(' ').to_owned()
    }).collect();
    json!({"cols": cols, "rows": rows, "cursor": cursor, "lines": lines, "title": title})
}
