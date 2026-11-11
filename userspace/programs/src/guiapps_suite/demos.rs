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
        t.resize(300, 200)?;
        t.step(16)?;
        t.wait_node("status", |v| v["width"] == 300 && v["height"] == 200 && v["inBounds"] == true)
    })
}
