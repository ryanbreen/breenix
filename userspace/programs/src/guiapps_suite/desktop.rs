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
