use breenix_suite::suite::CaseResult;
use libuitest::App;

const PICKER: &[u8] = b"/usr/bin/bfontpicker";
const DIR: &str = "/tmp/bfontpicker-suite-fonts";
const FIRST: &str = "/tmp/bfontpicker-suite-fonts/First.ttf";
const PLAIN: &str = "/tmp/bfontpicker-suite-fonts/Plain.ttf";
const FAKE: &str = "/tmp/bfontpicker-suite-fonts/SansMono-Fake.ttf";

fn launch() -> Result<App, String> {
    std::fs::create_dir_all(DIR).map_err(|e| e.to_string())?;
    for (source, dest) in [("DejaVuSansMono.ttf", FIRST), ("DejaVuSansMono.ttf", PLAIN), ("DejaVuSans.ttf", FAKE)] {
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
