use serde_json::{json, Value};
use libvt::{TermEmu, FG_COLOR, BG_COLOR};

pub fn terminal_value(vt: &TermEmu, active_tab: usize, tab_count: usize) -> Value {
    let (cols, rows) = vt.size();
    let mut lines = Vec::new();
    let mut runs = Vec::new();
    for y in 0..rows {
        let line: String = (0..cols).map(|x| vt.cell(x, y).ch).collect();
        lines.push(line.trim_end_matches(' ').to_owned());
        let mut row = Vec::new();
        let mut x = 0;
        while x < cols {
            let cell = vt.cell(x, y);
            let attr = (cell.fg, cell.bg, cell.bold, cell.underline, cell.inverse);
            let start = x;
            x += 1;
            while x < cols {
                let c = vt.cell(x, y);
                if (c.fg, c.bg, c.bold, c.underline, c.inverse) != attr { break; }
                x += 1;
            }
            if attr != (FG_COLOR, BG_COLOR, false, false, false) {
                row.push(json!({"start": start, "end": x, "fg": [attr.0.r, attr.0.g, attr.0.b], "bg": [attr.1.r, attr.1.g, attr.1.b], "bold": attr.2, "underline": attr.3, "inverse": attr.4}));
            }
        }
        runs.push(row);
    }
    let (x, y) = vt.cursor();
    json!({"active_tab": active_tab, "tab_count": tab_count, "cols": cols, "rows": rows, "cursor": [x, y], "lines": lines, "title": vt.title(), "alt": vt.alternate_screen(), "scrollback": vt.scrollback.lines.len(), "offset": 0, "runs": runs})
}
