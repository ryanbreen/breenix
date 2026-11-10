use breenix_suite::suite::CaseResult;
use libuitest::App;

const PICKER: &[u8] = b"/usr/bin/bfontpicker";
const DIR: &str = "/tmp/bfontpicker-suite-fonts";
const FIRST: &str = "/tmp/bfontpicker-suite-fonts/First.ttf";
const PLAIN: &str = "/tmp/bfontpicker-suite-fonts/Plain.ttf";
const FAKE: &str = "/tmp/bfontpicker-suite-fonts/SansMono-Fake.ttf";

fn launch() -> Result<App, String> {
    std::fs::create_dir_all(DIR).map_err(|e| e.to_string())?;
    for (source, dest) in [("DejaVuSansMono.ttf", FIRST), ("Hack-Regular.ttf", PLAIN), ("DejaVuSans.ttf", FAKE)] {
        std::fs::copy(format!("/usr/share/fonts/{source}"), dest).map_err(|e| e.to_string())?;
    }
    App::launch(&[PICKER, b"--fonts", DIR.as_bytes()], "Font Picker")
}

fn reopen(t: &mut App) -> Result<(), String> {
    t.key_exit(1, 0)?;
    t.launch_next(&[PICKER, b"--fonts", DIR.as_bytes()], "Font Picker")
}

fn seed(t: &mut App, contents: &str) -> Result<String, String> {
    let path = format!("{}/etc/fonts.conf", t.config_root());
    std::fs::write(&path, contents).map_err(|e| e.to_string())?;
    reopen(t)?;
    Ok(path)
}

fn select(t: &mut App) -> Result<(), String> {
    t.wait_node("fonts", |v| v["selected"] == 0 && v["rows"][0]["path"] == FIRST && v["rows"][1]["path"] == PLAIN)?;
    t.key(108, &[])?; // Down
    t.wait_node("fonts", |v| v["selected"] == 1 && v["rows"][1]["path"] == PLAIN)?;
    t.key(28, &[])?; // Enter
    t.wait_node("status", |v| v["mono"]["path"] == PLAIN && v["save"] == "saved")
}

pub fn lists_fonts() -> CaseResult {
    let mut t = launch()?;
    t.wait_node("fonts", |v| v["rows"].as_array().is_some_and(|rows| rows.len() == 3 && rows.iter().any(|r| r["name"] == "Plain" && r["path"] == PLAIN && r["section"] == "mono" && r["usable"] == true) && rows.iter().any(|r| r["name"] == "SansMono Fake" && r["path"] == FAKE && r["section"] == "display" && r["usable"] == true)))?;
    Ok(t.finish()?)
}

pub fn select_apply() -> CaseResult {
    let mut t = launch()?;
    seed(&mut t, &format!("mono.font={FIRST}\nmono.size=14\ndisplay.font={FAKE}\ndisplay.size=17\n"))?;
    select(&mut t)?;
    Ok(t.finish()?)
}

pub fn persists() -> CaseResult {
    let mut t = launch()?;
    let path = seed(&mut t, &format!("mono.font={FIRST}\nmono.size=14\ndisplay.font={FAKE}\ndisplay.size=17\n"))?;
    select(&mut t)?;
    let actual = std::fs::read(&path).map_err(|e| e.to_string())?;
    if actual != format!("mono.font={PLAIN}\nmono.size=14\ndisplay.font={FAKE}\ndisplay.size=17\n").as_bytes() { return Err(format!("unexpected fonts.conf: {actual:?}").into()); }
    reopen(&mut t)?;
    t.wait_node("fonts", |v| v["selected"] == 1 && v["rows"][1]["path"] == PLAIN && v["rows"][1]["active"] == true)?;
    Ok(t.finish()?)
}

pub fn keeps_unknown() -> CaseResult {
    let mut t = launch()?;
    let original = format!("# keep this comment\ndisplay.size=17\n\ndisplay.font={FAKE}\nmono.font={FIRST}\n# trailing comment\nmono.size=14\n");
    let expected = original.replace(FIRST, PLAIN);
    let path = seed(&mut t, &original)?;
    select(&mut t)?;
    let actual = std::fs::read(&path).map_err(|e| e.to_string())?;
    if actual != expected.as_bytes() { return Err(format!("fonts.conf changed beyond value: {actual:?}").into()); }
    Ok(t.finish()?)
}

fn pixels(window: u64) -> Result<(u32, u32, Vec<u8>), String> {
    let mut bytes = vec![0; 16 * 1024 * 1024];
    let captured = libbreenix::window::capture(window, &mut bytes, None).map_err(|e| e.to_string())?;
    let len = captured.width as usize * captured.height as usize * 4;
    if len > bytes.len() { return Err("capture exceeds buffer".into()); }
    bytes.truncate(len);
    Ok((captured.width, captured.height, bytes))
}

pub fn live_bterm() -> CaseResult {
    let mut t = launch()?;
    seed(&mut t, &format!("mono.font={FIRST}\nmono.size=14\ndisplay.font={FAKE}\ndisplay.size=17\n"))?;
    t.launch_next(&[b"/usr/bin/bterm"], "Terminal")?;
    t.type_line("echo Font comparison 12345")?;
    t.expect_text("Font comparison 12345")?;
    t.wait_node("grid", |v| v["font"]["path"] == FIRST && v["font"]["size"].as_f64() == Some(14.0))?;
    let terminal = t.window();
    let first = pixels(terminal)?;
    let second = pixels(terminal)?;
    if first != second { return Err("baseline terminal captures differ".into()); }
    t.attach("Font Picker")?;
    select(&mut t)?;
    t.attach("Terminal")?;
    t.wait_node("grid", |v| v["font"]["path"] == PLAIN && v["font"]["size"].as_f64() == Some(14.0))?;
    if pixels(terminal)? == first { return Err("terminal pixels did not change".into()); }
    Ok(t.finish()?)
}

pub fn save_fails_cleanly() -> CaseResult {
    std::fs::create_dir_all(DIR).map_err(|e| e.to_string())?;
    for (source, dest) in [("DejaVuSansMono.ttf", FIRST), ("Hack-Regular.ttf", PLAIN), ("DejaVuSans.ttf", FAKE)] {
        std::fs::copy(format!("/usr/share/fonts/{source}"), dest).map_err(|e| e.to_string())?;
    }
    let mut t = App::launch_as(&[PICKER, b"--fonts", DIR.as_bytes()], "Font Picker", 1000)?;
    let etc = format!("{}/etc", t.config_root());
    let path = format!("{etc}/fonts.conf");
    let before = std::fs::read(&path).map_err(|e| e.to_string())?;
    t.wait_node("fonts", |v| v["rows"].as_array().is_some_and(|rows| rows.len() == 3))?;
    t.key(28, &[])?;
    t.wait_node("status", |v| v["save"] == "failed" && v["error"].as_str().is_some_and(|e| !e.is_empty()))?;
    if std::fs::read(&path).map_err(|e| e.to_string())? != before { return Err("failed save changed fonts.conf".into()); }
    let entries = std::fs::read_dir(&etc).map_err(|e| e.to_string())?;
    for entry in entries {
        let name = entry.map_err(|e| e.to_string())?.file_name();
        if name.to_string_lossy().starts_with(".fonts.conf.atomic-") { return Err(format!("temporary file remains: {name:?}").into()); }
    }
    Ok(t.finish()?)
}
