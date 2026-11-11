use breenix_suite::suite::CaseResult;
use libuitest::{input::Modifier, App};

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
