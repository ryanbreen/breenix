//! Config scenarios run through real bwm, BreenGL and the sibling supervisor.
use breenix_suite::suite::{self, check, CaseResult};
use libuitest::Session;
use std::{fs, path::Path};
const APP: &[u8] = b"/usr/libexec/breenix/uitest-app_test";

pub fn settings_round(variant: u8) -> CaseResult {
    let mut session = Session::connect()?;
    let root = Path::new(session.config_root()).to_path_buf();
    let (size, visible, mono, display, ms, ds, theme, keys) = match variant {
        0 => (14, true, "DejaVuSansMono.ttf", "DejaVuSans.ttf", 10, 14, "default", "Super+Super = exec /usr/bin/blauncher\nSuper+Return = exec /usr/bin/bterm\nAlt+q = close\nAlt+Tab = focus_next\nAlt+Shift+Tab = focus_prev\n"),
        1 => (12, false, "DejaVuSans.ttf", "DejaVuSansMono.ttf", 11, 13, "a", "Ctrl+a = focus_next\n"),
        2 => (16, true, "DejaVuSans.ttf", "DejaVuSansMono.ttf", 12, 15, "b", "Ctrl+b = focus_prev\n"),
        _ => return Err("unknown config scenario".into()),
    };
    let fonts = format!("mono.font=/usr/share/fonts/{mono}\nmono.size={ms}\ndisplay.font=/usr/share/fonts/{display}\ndisplay.size={ds}\n");
    if variant != 0 {
        for (name, bytes) in [("fonts.conf", fonts.clone()), ("hotkeys.conf", keys.into()),
            ("bwm.conf", format!("font.size={size}\nclock.visible={visible}\n")), ("app.conf", format!("theme={theme}\n"))] {
            fs::write(root.join("etc").join(name), bytes).map_err(|e| e.to_string())?;
        }
    } else {
        // Check the fresh files before launching any reader, including after a crash.
        check(fs::read(root.join("etc/fonts.conf")).map_err(|e| e.to_string())? == fonts.as_bytes(), "fresh session inherited font settings")?;
        check(fs::read(root.join("etc/bwm.conf")).map_err(|e| e.to_string())? == b"font.size=14\nclock.visible=true\n", "fresh session inherited bwm settings")?;
        check(fs::read(root.join("etc/hotkeys.conf")).map_err(|e| e.to_string())? == keys.as_bytes(), "fresh session inherited hotkeys")?;
        check(fs::read(root.join("etc/app.conf")).map_err(|e| e.to_string())? == b"theme=default\n", "fresh session inherited app settings")?;
    }
    let wm = session.start_compositor(false)?;
    let wanted_settings = format!("[bwm] settings font.size={size} clock.visible={visible}");
    let wanted_keys = format!("[bwm] hotkeys: loaded {} sha256={} bindings={}", root.join("etc/hotkeys.conf").display(), libuitest::fixtures::digest(keys.as_bytes()), if variant == 0 { 5 } else { 1 });
    let wanted_fonts = format!("FontConfig {{ mono_path: \"/usr/share/fonts/{mono}\", mono_size: {ms}.0, display_path: \"/usr/share/fonts/{display}\", display_size: {ds}.0 }}");
    let mut settings_seen = false;
    let mut keys_seen = false;
    let mut fonts_seen = false;
    loop {
        let line = session.read_line(wm)?;
        settings_seen |= line == wanted_settings;
        keys_seen |= line == wanted_keys;
        fonts_seen |= line == format!("[bwm] fonts {wanted_fonts}");
        if line.starts_with("[bwm] first frame read back:") { break; }
    }
    check(settings_seen && keys_seen && fonts_seen, "bwm did not load the rooted settings, hotkeys and font config")?;
    let app = session.launch_app(&[APP, b"config reader", b"--config-probe"], 0)?;
    session.read_line(app)?; // byte-exact argv/environment echo has its own case
    check(session.read_line(app)? == format!("UITEST-CONFIG {wanted_fonts} theme={theme}"), "app did not read the rooted font and app settings")?;
    session.wait_window("config reader")?;
    check(fs::read(root.join("var/log/uitest-app")).map_err(|e| e.to_string())? == format!("theme={theme}\n").as_bytes(), "app write escaped the rooted log directory")?;
    session.teardown()?;
    check(!root.exists(), "session root survived teardown")?;
    let signature = format!("{size}:{visible}:{fonts}:{keys}:{theme}");
    let digest = libbreenix::crypto::sha256::sha256(signature.as_bytes());
    suite::value("settings", i64::from_le_bytes(digest[..8].try_into().unwrap()), "hash", None);
    Ok(())
}
pub fn config_root() -> CaseResult { settings_round(1) }
pub fn global_untouched() -> CaseResult {
    let before = suite::config::Snapshot::capture()?;
    settings_round(1)?;
    before.verify_restore()?;
    Ok(())
}
fn values(text: &str) -> Vec<(&str, &str)> {
    text.lines().filter_map(|line| {
        let rest = line.strip_prefix("SUITE runner-fixture VALUE fixture/")?;
        let (name, value) = rest.split_once(" settings=")?;
        Some((name, value))
    }).collect()
}
pub fn order_independent() -> CaseResult {
    let (ab, aexit) = super::runner::config_fixture("ab")?;
    let (ba, bexit) = super::runner::config_fixture("ba")?;
    check(aexit == 0 && bexit == 0, &format!("config order fixture failed: {ab}\n{ba}"))?;
    let a = values(&ab); let b = values(&ba);
    check(a.len() == 2 && b.len() == 2 && a[0].0 == "settings-a" && a[1].0 == "settings-b"
        && b[0].0 == "settings-b" && b[1].0 == "settings-a", "settings scenarios did not run in both orders")?;
    check(a[0].1 == b[1].1 && a[1].1 == b[0].1 && a[0].1 != a[1].1, "settings results depend on execution order")
}
pub fn restore_after_crash() -> CaseResult {
    let (text, exit) = super::runner::config_fixture("crash")?;
    check(exit == 1, "planted crash fixture did not fail")?;
    check(text.contains("the case crashed: killed by signal 9"), "fixture failed without the requested SIGKILL")?;
    let failed = text.find(" CASE fixture/settings-crash FAIL ").ok_or("missing crashed case verdict")?;
    let defaults = text.find(" CASE fixture/settings-defaults PASS ").ok_or("next case did not see defaults")?;
    check(failed < defaults && text.contains("/etc/fonts.conf") && text.contains("DONE passed=1 failed=1 skipped=0 total=2"), "crash was not restored and reported before the default case")
}
