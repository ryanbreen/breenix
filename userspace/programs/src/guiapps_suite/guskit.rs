use breenix_suite::suite::CaseResult;
use libuitest::{App, input::Modifier};
#[path = "guskit_expect.rs"] mod guskit_expect;
use guskit_expect::Expected;

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
    t.key(49, &[Modifier::Ctrl])?; expect(&mut t, &Expected::white())?;
    t.key(31, &[Modifier::Ctrl])?;
    t.wait_node("status", |v| v["file"] == FILE && v["message"] == "Saved")?;
    let bytes = std::fs::read(FILE).map_err(|e| e.to_string())?;
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") { return Err("saved file is not PNG".into()); }
    t.key(24, &[Modifier::Ctrl])?; t.wait_node("open", |_| true)?;
    t.key(1, &[])?; absent(&mut t, "open")?;
    Ok(t.finish()?)
}
