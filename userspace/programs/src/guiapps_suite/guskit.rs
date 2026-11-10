use breenix_suite::suite::CaseResult;
use libuitest::App;
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
