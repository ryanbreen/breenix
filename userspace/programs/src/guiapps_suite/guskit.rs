use breenix_suite::suite::CaseResult;
use libuitest::{App, input::Modifier};
#[path = "guskit_expect.rs"] pub(super) mod guskit_expect;
use guskit_expect::Expected;
#[path = "../guskit/save_wait.rs"] mod save_wait;

fn launch() -> Result<App, String> {
    App::launch(&[b"/usr/bin/guskit", b"--canvas", b"200x120"], "Gus Kit")
}
fn expect(t: &mut App, image: &Expected) -> Result<(), String> {
    let digest = image.digest();
    t.wait_node("canvas", |v| v["width"] == 200 && v["height"] == 120 && v["sha256"] == digest)
}
fn tool(t: &mut App, name: &str) -> Result<(), String> {
    t.drag(&format!("tool-{name}"), (4, 4), &[])?;
    t.wait_node("status", |v| v["tool"] == name)
}
fn capture(t: &App, expected: &Expected) -> Result<(), String> {
    let mut bgra = vec![0; 800 * 600 * 4];
    let frame = libbreenix::window::capture(t.window(), &mut bgra, None).map_err(|e| e.to_string())?;
    if frame.width < 200 || frame.height < 296 { return Err("capture too small for canvas".into()); }
    let mut rgb = Vec::with_capacity(200 * 120 * 3);
    for y in 176..296 {
        for x in 0..200 {
            let i = (y * frame.width as usize + x) * 4;
            rgb.extend_from_slice(&[bgra[i + 2], bgra[i + 1], bgra[i]]);
        }
    }
    let actual: String = libbreenix::crypto::sha256::sha256(&rgb).iter().map(|b| format!("{b:02x}")).collect();
    if actual != expected.digest() { return Err(format!("canvas capture digest {actual}, expected {}", expected.digest())); }
    Ok(())
}
pub fn pencil() -> CaseResult {
    let mut t = launch()?;
    t.drag("canvas", (10, 10), &[(30, 0), (0, 20)])?;
    let mut image = Expected::white(); image.horizontal(10, 40, 10); image.vertical(40, 10, 30);
    expect(&mut t, &image)?; capture(&t, &image)?;
    Ok(t.finish()?)
}
pub fn shapes() -> CaseResult {
    let mut t = launch()?; let mut image = Expected::white();
    tool(&mut t, "line")?; t.drag("canvas", (10, 10), &[(30, 0)])?;
    image.horizontal(10, 40, 10); expect(&mut t, &image)?;
    tool(&mut t, "rectangle")?; t.drag("canvas", (20, 30), &[(30, 20)])?;
    image.rectangle(20, 30, 50, 50); expect(&mut t, &image)?;
    tool(&mut t, "circle")?; t.drag("canvas", (80, 60), &[(5, 0)])?;
    image.circle5(80, 60); expect(&mut t, &image)?;
    Ok(t.finish()?)
}
pub fn fill() -> CaseResult {
    let mut t = launch()?; let mut image = Expected::white();
    tool(&mut t, "rectangle")?; t.drag("canvas", (20, 30), &[(30, 20)])?;
    image.rectangle(20, 30, 50, 50); expect(&mut t, &image)?;
    tool(&mut t, "fill")?; t.drag("canvas", (25, 35), &[])?;
    image.interior(20, 30, 50, 50); expect(&mut t, &image)?;
    Ok(t.finish()?)
}

fn absent(t: &mut App, key: &str) -> Result<(), String> {
    let tree = t.tree()?;
    if tree.root.key == key || tree.children.iter().any(|node| node.key == key) {
        return Err(format!("unexpected node {key}"));
    }
    Ok(())
}
fn stroke(t: &mut App, y: i32) -> Result<(), String> {
    t.drag("canvas", (10, y), &[(30, 0)])
}
fn undo(t: &mut App) -> Result<(), String> { t.key(44, &[Modifier::Ctrl]) }
fn redo(t: &mut App) -> Result<(), String> { t.key(44, &[Modifier::Ctrl, Modifier::Shift]) }

pub fn eraser_color() -> CaseResult {
    let mut t = launch()?;
    // x=260 gives hue 120; (80,19) gives saturation and value 204.
    t.drag("hue", (260, 4), &[])?;
    t.drag("sv", (80, 19), &[])?;
    t.wait_node("status", |v| v["color"] == serde_json::json!([40, 204, 40]))?;
    stroke(&mut t, 10)?;
    let mut image = Expected::white();
    image.colored_horizontal(10, 40, 10, [40, 204, 40]);
    expect(&mut t, &image)?;
    tool(&mut t, "eraser")?;
    stroke(&mut t, 10)?;
    expect(&mut t, &Expected::white())?;
    Ok(t.finish()?)
}

pub fn undo_redo() -> CaseResult {
    let mut t = launch()?;
    let mut first = Expected::white(); first.horizontal(10, 40, 10);
    let mut second = Expected::white(); second.horizontal(10, 40, 10); second.horizontal(10, 40, 20);
    for y in [10, 20, 30] { stroke(&mut t, y)?; }
    t.wait_node("status", |v| v["undo"] == 3 && v["redo"] == 0)?;
    undo(&mut t)?; undo(&mut t)?;
    expect(&mut t, &first)?;
    t.wait_node("status", |v| v["undo"] == 1 && v["redo"] == 2)?;
    redo(&mut t)?; expect(&mut t, &second)?;
    t.wait_node("status", |v| v["undo"] == 2 && v["redo"] == 1)?;
    stroke(&mut t, 40)?;
    second.horizontal(10, 40, 40); expect(&mut t, &second)?;
    t.wait_node("status", |v| v["undo"] == 3 && v["redo"] == 0)?;
    Ok(t.finish()?)
}

pub fn quit_unsaved() -> CaseResult {
    let mut t = launch()?;
    stroke(&mut t, 10)?;
    t.key(16, &[Modifier::Ctrl])?;
    t.wait_node("confirm", |_| true)?;
    t.wait_node("status", |v| v["dirty"] == true)?;
    t.key(1, &[])?; absent(&mut t, "confirm")?;
    t.key(16, &[Modifier::Ctrl])?; t.wait_node("confirm", |_| true)?;
    t.key_exit(32, 0)?;
    Ok(t.finish()?)
}

pub fn shortcuts() -> CaseResult {
    const FILE: &str = "/tmp/guskit-shortcuts.png";
    match std::fs::remove_file(FILE) { Ok(()) => (), Err(e) if e.kind() == std::io::ErrorKind::NotFound => (), Err(e) => return Err(e.to_string().into()) }
    let mut t = App::launch(&[b"/usr/bin/guskit", b"--canvas", b"200x120", FILE.as_bytes()], "Gus Kit")?;
    for (key, name) in [(25,"pencil"), (48,"brush"), (38,"line"), (19,"rectangle"), (46,"circle"), (33,"fill"), (18,"eraser")] {
        t.key(key, &[])?; t.wait_node("status", |v| v["tool"] == name)?;
    }
    for (key, size) in [(2,"small"), (3,"medium"), (4,"large")] {
        t.key(key, &[])?; t.wait_node("status", |v| v["size"] == size)?;
    }
    t.key(25, &[])?; stroke(&mut t, 10)?;
    let mut image = Expected::white(); image.horizontal(10, 40, 10); expect(&mut t, &image)?;
    undo(&mut t)?; expect(&mut t, &Expected::white())?;
    t.wait_node("status", |v| v["undo"] == 0 && v["redo"] == 1)?;
    redo(&mut t)?; expect(&mut t, &image)?;
    t.wait_node("status", |v| v["undo"] == 1 && v["redo"] == 0)?;
    t.key(31, &[Modifier::Ctrl])?;
    t.wait_node("status", |v| v["file"] == FILE && v["message"] == "Saved")?;
    let original = std::fs::read(FILE).map_err(|e| e.to_string())?;
    t.key(49, &[Modifier::Ctrl])?; expect(&mut t, &Expected::white())?;
    t.wait_node("status", |v| v["file"].is_null())?;
    stroke(&mut t, 20)?;
    t.key(31, &[Modifier::Ctrl])?;
    t.wait_node("status", |v| v["file"].as_str().is_some_and(|p| p != FILE && std::path::Path::new(p).file_name().is_some_and(|n| n.to_string_lossy().starts_with("untitled-"))) && v["message"] == "Saved")?;
    let tree = t.tree()?;
    let saved = tree.children.iter().find(|n| n.key == "status").and_then(|n| n.value["file"].as_str()).ok_or("missing untitled path")?;
    let bytes = std::fs::read(saved).map_err(|e| e.to_string())?;
    if std::fs::read(FILE).map_err(|e| e.to_string())? != original { return Err("New and Save changed the opened drawing".into()); }
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") { return Err("saved file is not PNG".into()); }
    t.key(24, &[Modifier::Ctrl])?; t.wait_node("open", |_| true)?;
    t.key(1, &[])?; absent(&mut t, "open")?;
    Ok(t.finish()?)
}

pub fn save_open() -> CaseResult {
    // Discard a new, unsaved canvas to close the process without ending the
    // case's single session. Reopening must recover the saved stroke instead.
    fn close(t: &mut App) -> Result<(), String> {
        t.key(49, &[Modifier::Ctrl])?;
        t.key(16, &[Modifier::Ctrl])?;
        t.wait_node("confirm", |_| true)?;
        t.key_exit(32, 0)
    }
    let files = ["/tmp/a.bmp", "/tmp/a.png"];
    for file in files {
        match std::fs::remove_file(file) { Ok(()) => (), Err(e) if e.kind() == std::io::ErrorKind::NotFound => (), Err(e) => return Err(e.to_string().into()) }
    }
    let mut t = App::launch(&[b"/usr/bin/guskit", b"--canvas", b"200x120", files[0].as_bytes()], "Gus Kit")?;
    for (index, file) in files.into_iter().enumerate() {
        let argv: &[&[u8]] = &[b"/usr/bin/guskit", b"--canvas", b"200x120", file.as_bytes()];
        if index != 0 { t.launch_next(argv, "Gus Kit")?; }
        stroke(&mut t, 10)?;
        let mut image = Expected::white(); image.horizontal(10, 40, 10);
        expect(&mut t, &image)?;
        t.wait_node("status", |v| v["dirty"] == true)?;
        t.key(31, &[Modifier::Ctrl])?;
        t.wait_node("status", |v| v["dirty"] == false && v["message"] == "Saved")?;
        close(&mut t)?;
        t.launch_next(argv, "Gus Kit")?;
        expect(&mut t, &image)?;
        close(&mut t)?;
    }
    Ok(t.finish()?)
}

pub fn save_fails_cleanly() -> CaseResult {
    use std::os::unix::fs::PermissionsExt;
    const DIR: &str = "/tmp/guskit-denied";
    const FILE: &str = "/tmp/guskit-denied/a.png";
    const PNG: &[u8] = &[137,80,78,71,13,10,26,10,0,0,0,13,73,72,68,82,0,0,0,1,0,0,0,1,8,2,0,0,0,144,119,83,222,0,0,0,12,73,68,65,84,120,156,99,248,255,255,63,0,5,254,2,254,13,239,70,184,0,0,0,0,73,69,78,68,174,66,96,130];
    match std::fs::remove_dir_all(DIR) { Ok(()) => (), Err(e) if e.kind() == std::io::ErrorKind::NotFound => (), Err(e) => return Err(e.to_string().into()) }
    std::fs::create_dir_all(DIR).map_err(|e| e.to_string())?;
    std::fs::set_permissions(DIR, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    std::fs::write(FILE, PNG).map_err(|e| e.to_string())?;
    let mut t = App::launch_as(&[b"/usr/bin/guskit", b"--canvas", b"200x120", FILE.as_bytes()], "Gus Kit", 1000)?;
    t.wait_node("status", |v| v["message"] == "" && v["dirty"] == false && v["file"] == FILE)?;
    stroke(&mut t, 10)?;
    t.wait_node("status", |v| v["dirty"] == true)?;
    t.key(31, &[Modifier::Ctrl])?;
    t.wait_node("status", |v| v["dirty"] == true && v["message"].as_str().is_some_and(|s| !s.is_empty() && s != "Saved"))?;
    if std::fs::read(FILE).map_err(|e| e.to_string())? != PNG { return Err("failed save changed PNG".into()); }
    for entry in std::fs::read_dir(DIR).map_err(|e| e.to_string())? {
        let name = entry.map_err(|e| e.to_string())?.file_name();
        if name != "a.png" { return Err(format!("unexpected file after failed save: {name:?}").into()); }
    }
    Ok(t.finish()?)
}

pub fn open_malformed() -> CaseResult {
    const DIR: &str = "/tmp/guskit-malformed";
    const FILE: &str = "/tmp/guskit-malformed/bad.png";
    const INITIAL: &str = "/tmp/guskit-malformed/new.png";
    match std::fs::remove_dir_all(DIR) { Ok(()) => (), Err(e) if e.kind() == std::io::ErrorKind::NotFound => (), Err(e) => return Err(e.to_string().into()) }
    std::fs::create_dir_all(DIR).map_err(|e| e.to_string())?;
    std::fs::write(FILE, b"\x89PNG\r\n\x1a\n\0\0").map_err(|e| e.to_string())?;
    let mut t = App::launch(&[b"/usr/bin/guskit", b"--canvas", b"200x120", INITIAL.as_bytes()], "Gus Kit")?;
    stroke(&mut t, 10)?;
    let mut image = Expected::white(); image.horizontal(10, 40, 10);
    expect(&mut t, &image)?;
    t.key(24, &[Modifier::Ctrl])?; t.wait_node("open", |_| true)?;
    t.key(108, &[])?; t.key(28, &[])?;
    t.wait_node("status", |v| v["message"].as_str().is_some_and(|s| s.starts_with("PNG decode:")))?;
    expect(&mut t, &image)?;
    Ok(t.finish()?)
}

fn require_shares(case: &str) -> Result<(), String> {
    use std::io::Write;
    let state = std::fs::read_to_string("/run/breenix/services").map_err(|e| e.to_string())?;
    libbreenix::services::shares_completed_once(&state)?;
    let service = state.lines().find(|line| line.starts_with("service shares "))
        .ok_or("missing shares service record")?;
    let mounts = std::fs::read_to_string("/proc/mounts").map_err(|e| e.to_string())?;
    let mut output = SMB_RESTART_OUTPUT.get().ok_or("missing SMB diagnostic output")?;
    writeln!(output, "GUSKIT_SMB_EVIDENCE guskit/{case} {service}").map_err(|e| e.to_string())?;
    for mount in ["/mnt/guskit-rw", "/mnt/guskit-ro"] {
        let line = mounts.lines().find(|line| {
            let mut fields = line.split_whitespace();
            fields.next().is_some_and(|source| source.starts_with("//"))
                && fields.next() == Some(mount) && fields.next() == Some("ufs")
        })
            .ok_or_else(|| format!("missing mount {mount}"))?;
        writeln!(output, "GUSKIT_SMB_EVIDENCE guskit/{case} mount {line}").map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn save_as(t: &mut App, path: &str) -> Result<(), String> {
    t.key(31, &[Modifier::Ctrl, Modifier::Shift])?;
    t.wait_node("save-as", |_| true)?;
    t.key(15, &[])?; // Focus the path field.
    t.key(102, &[])?; // Home, then select the existing directory.
    t.key(107, &[Modifier::Shift])?;
    t.type_text(path)?;
    t.key(28, &[])
}

fn close_saved(t: &mut App) -> Result<(), String> {
    t.key(49, &[Modifier::Ctrl])?;
    t.key(16, &[Modifier::Ctrl])?;
    t.wait_node("confirm", |_| true)?;
    t.key_exit(32, 0)
}

pub fn smb_save_open() -> CaseResult {
    require_shares("smb-save-open")?;
    let dir = format!("/mnt/guskit-rw/guskit-{}-{}", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos());
    std::fs::create_dir(&dir).map_err(|e| e.to_string())?;
    // Always remove the per-run files, including when a case assertion fails.
    struct Folder(String);
    impl Drop for Folder { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
    let folder = Folder(dir);
    let mut t = launch()?;
    stroke(&mut t, 10)?;
    let mut image = Expected::white(); image.horizontal(10, 40, 10);
    expect(&mut t, &image)?;
    for ext in ["png", "bmp"] {
        let file = format!("{}/drawing.{ext}", folder.0);
        save_as(&mut t, &file)?;
        t.wait_node("status", |v| v["file"] == file && v["dirty"] == false && v["message"] == "Saved")?;
    }
    close_saved(&mut t)?;
    for ext in ["png", "bmp"] {
        let file = format!("{}/drawing.{ext}", folder.0);
        t.launch_next(&[b"/usr/bin/guskit", b"--canvas", b"200x120", file.as_bytes()], "Gus Kit")?;
        expect(&mut t, &image)?;
        close_saved(&mut t)?;
    }
    t.finish()?;
    std::fs::remove_dir_all(&folder.0).map_err(|e| e.to_string())?;
    std::mem::forget(folder);
    Ok(())
}

fn directory_names(path: &str) -> Result<Vec<std::ffi::OsString>, String> {
    let mut names = std::fs::read_dir(path).map_err(|e| e.to_string())?
        .map(|entry| entry.map(|e| e.file_name()).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    names.sort();
    Ok(names)
}

pub fn smb_save_fails_cleanly() -> CaseResult {
    require_shares("smb-save-fails-cleanly")?;
    const FILE: &str = "/mnt/guskit-ro/drawing.png";
    const SHA256: &str = "471226f2710862db5b66577c43497117c6011f58ee90b3449d9d5f31ad69bc93";
    let names = vec![std::ffi::OsString::from("README.txt"), std::ffi::OsString::from("drawing.png")];
    if directory_names("/mnt/guskit-ro")? != names { return Err("unexpected read-only fixture listing".into()); }
    let hash = |bytes: &[u8]| -> String {
        libbreenix::crypto::sha256::sha256(bytes).iter().map(|b| format!("{b:02x}")).collect()
    };
    let before = std::fs::read(FILE).map_err(|e| e.to_string())?;
    if before.len() != 93 || hash(&before) != SHA256 { return Err("unexpected read-only drawing bytes".into()); }
    let mut t = App::launch(&[b"/usr/bin/guskit", b"--canvas", b"200x120", FILE.as_bytes()], "Gus Kit")?;
    t.wait_node("status", |v| v["file"] == FILE && v["dirty"] == false && v["message"] == "")?;
    // The fixture is a 16x16 blue/white checkerboard, copied onto the white canvas.
    let mut image = Expected::white();
    for y in 0..16 {
        for x in 0..16 {
            if (x / 4 + y / 4) % 2 == 0 { image.colored_horizontal(x, x, y, [0x1e, 0x6f, 0xd9]); }
        }
    }
    expect(&mut t, &image)?;
    stroke(&mut t, 10)?;
    image.horizontal(10, 40, 10);
    expect(&mut t, &image)?;
    t.key(31, &[Modifier::Ctrl])?;
    t.wait_node("status", |v| v["file"] == FILE && v["dirty"] == true && v["message"].as_str().is_some_and(|s| !s.is_empty() && s != "Saved"))?;
    expect(&mut t, &image)?;
    if directory_names("/mnt/guskit-ro")? != names { return Err("read-only save changed directory entries".into()); }
    let after = std::fs::read(FILE).map_err(|e| e.to_string())?;
    if hash(&after) != SHA256 || after != before { return Err("read-only save changed the overwritten drawing".into()); }
    use std::io::Write;
    let mut output = SMB_RESTART_OUTPUT.get().ok_or("missing SMB diagnostic output")?;
    writeln!(output, "GUSKIT_SMB_EVIDENCE guskit/smb-save-fails-cleanly bytes={} sha256={SHA256} listing=README.txt,drawing.png", after.len())
        .map_err(|e| e.to_string())?;
    Ok(t.finish()?)
}

pub(super) static SMB_RESTART_OUTPUT: std::sync::OnceLock<std::fs::File> = std::sync::OnceLock::new();
// The restart helper consumes these serial requests; both actions share one format.
const SMB_REQUEST: &str = "SUITE gui-apps SMB{} guskit/smb-share-gone seq={}";
fn smb_request(action: &str, seq: u64) -> Result<(), String> {
    use std::io::Write;
    let mut output = SMB_RESTART_OUTPUT.get().ok_or("missing SMB restart output")?;
    let line = format!("{}\n", SMB_REQUEST.replace("SMB{}", &format!("SMB{action}")).replace("{}", &seq.to_string()));
    output.write_all(line.as_bytes()).map_err(|e| e.to_string())
}

fn wait_share(path: &str, stopped: bool) -> Result<(), String> {
    let path = path.to_owned();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || loop {
        let result = std::fs::read_dir(&path).and_then(|entries| entries.collect::<Result<Vec<_>, _>>());
        match result {
            Ok(_) if !stopped => { let _ = tx.send(Ok(())); return; }
            Err(e) if stopped && e.raw_os_error() == Some(5) => { let _ = tx.send(Ok(())); return; }
            Err(e) if e.raw_os_error() != Some(5) => { let _ = tx.send(Err(e.to_string())); return; }
            _ => ()
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    });
    rx.recv_timeout(std::time::Duration::from_secs(10)).map_err(|_| "SMB listing hang watchdog expired".to_string())?
}

pub fn smb_share_gone() -> CaseResult {
    require_shares("smb-share-gone")?;
    let seq = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?.as_micros() as u64;
    let dir = format!("/mnt/guskit-rw/guskit-{}-{seq}", std::process::id());
    std::fs::create_dir(&dir).map_err(|e| e.to_string())?;
    struct Folder(String);
    impl Drop for Folder { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
    let folder = Folder(dir.clone());
    let file = format!("{dir}/drawing.png");
    let mut t = launch()?;
    stroke(&mut t, 10)?;
    save_as(&mut t, &file)?;
    t.wait_node("status", |v| v["file"] == file && v["dirty"] == false && v["message"] == "Saved")?;
    let before = std::fs::read(&file).map_err(|e| e.to_string())?;
    let names = directory_names(&dir)?;
    close_saved(&mut t)?;
    t.launch_next(&[b"/usr/bin/guskit", b"--canvas", b"200x120", file.as_bytes()], "Gus Kit")?;
    t.wait_node("status", |v| v["file"] == file && v["dirty"] == false)?;
    stroke(&mut t, 20)?;
    t.wait_node("status", |v| v["dirty"] == true)?;
    // Always request recovery on an early error; the host helper also restores on exit.
    struct Restart(Option<u64>);
    impl Drop for Restart { fn drop(&mut self) { if let Some(seq) = self.0 { let _ = smb_request("START", seq); } } }
    let mut restart = Restart(Some(seq + 1));
    let control = std::path::PathBuf::from(format!("/run/breenix/guskit-interrupt-{seq}"));
    std::fs::create_dir(&control).map_err(|e| e.to_string())?;
    struct Control(std::path::PathBuf);
    impl Drop for Control { fn drop(&mut self) {
        let _ = std::fs::remove_file("/run/breenix/guskit-save-barrier");
        let _ = std::fs::remove_dir_all(&self.0);
    } }
    let control = Control(control);
    std::fs::write("/run/breenix/guskit-save-barrier", format!("{file}\n{}\n", control.0.display())).map_err(|e| e.to_string())?;
    let (watch_dir, watch_file, watch_control) = (dir.clone(), file.clone(), control.0.clone());
    let watch_before = before.clone();
    let watcher = std::thread::spawn(move || -> Result<(), String> {
        save_wait::wait(&watch_control, "partial").map_err(|e| e.to_string())?;
        let partial = std::fs::read_to_string(watch_control.join("partial")).map_err(|e| e.to_string())?;
        if partial != "16" { return Err(format!("missing partial write witness: {partial}")); }
        if std::fs::read(&watch_file).map_err(|e| e.to_string())? != watch_before { return Err("partial save replaced old file".into()); }
        let temps: Vec<_> = std::fs::read_dir(&watch_dir).map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?.into_iter()
            .filter(|e| e.file_name().to_string_lossy().starts_with(".drawing.png.atomic-")).collect();
        if temps.len() != 1 || temps[0].metadata().map_err(|e| e.to_string())?.len() != 16 {
            return Err("save has not written a partial temporary".into());
        }
        smb_request("STOP", seq)?;
        wait_share(&watch_dir, true)?;
        std::fs::write(watch_control.join("stopped"), b"").map_err(|e| e.to_string())?;
        save_wait::wait(&watch_control, "failed").map_err(|e| e.to_string())?;
        if std::fs::read_to_string(watch_control.join("failed")).map_err(|e| e.to_string())? != "5" {
            return Err("interrupted write did not return EIO".into());
        }
        smb_request("START", seq + 1)?;
        wait_share(&watch_dir, false)?;
        std::fs::write(watch_control.join("recovered"), b"").map_err(|e| e.to_string())?;
        use std::io::Write;
        let mut output = SMB_RESTART_OUTPUT.get().ok_or("missing SMB diagnostic output")?;
        writeln!(output, "GUSKIT_SMB_EVIDENCE guskit/smb-share-gone partial_bytes=16 write_errno=5 recovered=true").map_err(|e| e.to_string())?;
        Ok(())
    });
    let key_result = t.key_wait_node(31, &[Modifier::Ctrl], "status", |v|
        v["dirty"] == true && v["message"].as_str().is_some_and(|s| !s.is_empty() && s != "Saved"));
    watcher.join().map_err(|_| "save interruption watcher panicked")??;
    restart.0 = None;
    key_result?;
    // The save-result commit is now visible; a fresh tool key must still be
    // processed within the normal input deadline.
    t.key(25, &[])?;
    t.wait_node("status", |v| v["tool"] == "pencil" && v["dirty"] == true)?;
    // tests/atomic_file.rs exercises faults at each save step; this is the app path.
    if std::fs::read(&file).map_err(|e| e.to_string())? != before { return Err("failed save changed drawing".into()); }
    if directory_names(&dir)? != names { return Err("failed save changed directory entries".into()); }
    // Witness the app's exit and window destruction before retiring the fixture.
    t.key(16, &[Modifier::Ctrl])?;
    t.wait_node("confirm", |_| true)?;
    t.key_exit(32, 0)?;
    t.finish()?;
    std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    std::mem::forget(folder);
    Ok(())
}
