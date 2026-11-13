//! Exercise the demos in one desktop session per case.
use breenix_suite::suite::CaseResult;
use libuitest::App;

const DEMOS: [(&[u8], &str); 6] = [
    (b"/usr/bin/bounce", "Bounce"),
    (b"/usr/bin/particles", "Particles"),
    (b"/usr/bin/confetti", "Confetti"),
    (b"/usr/bin/demo", "Demo"),
    (b"/usr/bin/rectangles", "Rectangles"),
    (b"/usr/bin/biconkit", "Breenix Icon Kit"),
];

fn each(mut action: impl FnMut(&mut App) -> Result<(), String>) -> CaseResult {
    let (path, title) = DEMOS[0];
    let mut t = App::launch(&[path], title)?;
    action(&mut t)?;
    for &(path, title) in &DEMOS[1..] {
        t.launch_next(&[path], title)?;
        action(&mut t)?;
    }
    Ok(t.finish()?)
}

pub fn open_each() -> CaseResult {
    each(|t| {
        t.wait_node("status", |v| v["epochs"].as_u64().is_some())?;
        let tree = t.tree()?;
        let before = tree.root.value["epochs"].as_u64().ok_or("missing epochs")?;
        t.step(16)?;
        t.step(16)?;
        t.wait_node("status", |v| v["epochs"].as_u64().is_some_and(|n| n > before))
    })
}

pub fn close_each() -> CaseResult { each(|t| t.close_exit()) }

pub fn resize() -> CaseResult {
    each(|t| {
        let tree = t.tree()?;
        let scale = if tree.root.value["demo"] == "biconkit" {
            match tree.root.value["uiScale"].as_u64() {
                Some(s @ (1 | 2)) => s as u32,
                _ => return Err("invalid icon kit UI scale".into()),
            }
        } else { 1 };
        let (width, height) = (300 * scale, 200 * scale);
        t.resize(width, height)?;
        t.step(16)?;
        t.wait_node("status", |v| v["width"] == width && v["height"] == height && v["inBounds"] == true)
    })
}

pub fn bounce_input() -> CaseResult {
    let mut t = App::launch(&[DEMOS[0].0], DEMOS[0].1)?;
    for _ in 0..40 { t.key(13, &[])?; }
    t.expect_node("status", |v| v["speed"] == 400)?;
    t.key(13, &[])?;
    t.expect_node("status", |v| v["speed"] == 400)?;
    for _ in 0..40 { t.key(12, &[])?; }
    t.expect_node("status", |v| v["speed"] == 25)?;
    t.key(12, &[])?;
    t.expect_node("status", |v| v["speed"] == 25)?;
    let before = t.tree()?;
    let sphere = before.children.iter().find(|n| n.key == "sphere-2").ok_or("missing sphere-2")?;
    let center = ((sphere.bounds[2] / 2) as i32, (sphere.bounds[3] / 2) as i32);
    t.drag("sphere-2", center, &[])?;
    let after = t.tree()?;
    if before.root.value["epochs"] != after.root.value["epochs"] { return Err("clock advanced during click".into()); }
    for old in &before.children {
        let new = after.children.iter().find(|n| n.key == old.key).ok_or("sphere disappeared")?;
        if (old.value != new.value) != (old.key == "sphere-2") {
            return Err(format!("unexpected velocity change: {}", old.key).into());
        }
    }
    Ok(t.finish()?)
}

pub fn bounded() -> CaseResult {
    let mut t = App::launch(&[DEMOS[0].0], DEMOS[0].1)?;
    for &(path, title) in &DEMOS[..2] {
        if title != DEMOS[0].1 { t.launch_next(&[path], title)?; }
        t.step(16)?;
        let before = t.tree()?.root.value["epochs"].as_u64().ok_or("missing epochs")?;
        t.step(9600)?;
        t.wait_node("status", |v| v["epochs"] == before + 600 && v["inBounds"] == true)?;
    }
    Ok(t.finish()?)
}

pub fn biconkit_interact() -> CaseResult {
    let mut t = App::launch(&[DEMOS[5].0], DEMOS[5].1)?;
    let tree = t.tree()?;
    let icon = tree.children.iter().find(|n| n.key == "icon-1").ok_or("missing icon-1")?;
    let center = ((icon.bounds[2] / 2) as i32, (icon.bounds[3] / 2) as i32);
    t.hover("icon-1", center)?;
    t.wait_node("icon-1", |v| v["state"] == "HoverIn")?;
    t.step(16)?;
    t.step(300)?;
    t.wait_node("icon-1", |v| v["state"] == "Hovering")?;
    t.drag("icon-1", center, &[])?;
    t.wait_node("icon-1", |v| v["state"] == "Clicked")?;
    t.expect_node("icons", |v| v["selected"] == 1)?;
    t.key(51, &[])?;
    t.expect_node("icons", |v| v["selected"] == 0)?;
    t.key(51, &[])?;
    t.expect_node("icons", |v| v["selected"] == 0)?;
    let count = t.tree()?.root.value["objects"].as_u64().ok_or("missing icon count")?;
    for selected in 1..count {
        t.key(52, &[])?;
        t.expect_node("icons", |v| v["selected"] == selected)?;
    }
    t.key(52, &[])?;
    t.expect_node("icons", |v| v["selected"] == count - 1)?;
    Ok(t.finish()?)
}
