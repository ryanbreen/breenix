//! Six collaboration paths, with pixels replayed from the submitted script.
use breenix_suite::suite::CaseResult;
use libuitest::{App, input::Modifier};
use serde_json::Value;
use super::guskit::guskit_expect::Expected;

const HOST: &str = "Host";
const PEER: &str = "Peer";
const THIRD: &str = "Third";

#[derive(Clone, Copy)]
enum Op {
    Stroke { y: usize, color: [u8; 3] },
    Undo,
}
struct Submitted { owner: &'static str, id: u64, op: Op, refusal: Option<&'static str> }
struct Script(Vec<Submitted>, std::collections::BTreeMap<String, Value>);
impl Script {
    fn new() -> Self { Self(Vec::new(), std::collections::BTreeMap::new()) }
    fn record(&mut self, owner: &'static str, op: Op, refusal: Option<&'static str>) {
        let id = self.0.iter().filter(|s| s.owner == owner).count() as u64 + 1;
        self.0.push(Submitted { owner, id, op, refusal });
    }
    fn stroke(&mut self, t: &mut App, owner: &'static str, y: usize, green: bool) -> Result<(), String> {
        attach(t, owner)?;
        t.key(38, &[])?; // Line: one completed gesture is one submitted operation.
        let color = if green {
            t.drag("hue", (260, 4), &[])?;
            t.drag("sv", (80, 19), &[])?;
            [40, 204, 40]
        } else { [0, 0, 0] };
        t.wait_node("status", |v| v["tool"] == "line" && v["color"] == serde_json::json!(color))?;
        t.drag("canvas", (10, y as i32), &[(30, 0)])?;
        self.record(owner, Op::Stroke { y, color }, None);
        Ok(())
    }
    fn undo(&mut self, t: &mut App, owner: &'static str, refused: bool) -> Result<(), String> {
        attach(t, owner)?;
        t.key(44, &[Modifier::Ctrl])?;
        self.record(owner, Op::Undo, refused.then_some("nothing-to-undo"));
        Ok(())
    }
    // Each submitted id must have exactly one answer. Ack globals determine the
    // replay order; no pixel expectation is derived from a peer's canvas.
    fn replay(&mut self, t: &mut App, owners: &[&str]) -> Result<(Expected, Value), String> {
        let mut ordered = std::collections::BTreeMap::new();
        for &owner in owners {
            attach(t, owner)?;
            let wanted: Vec<_> = self.0.iter().filter(|s| s.owner == owner).collect();
            t.wait_node("collab", |v| v["submitted"].as_array().is_some_and(|rows|
                rows.len() == wanted.len() && rows.iter().all(|row| row.get("global").is_some() || row.get("refused").is_some())))?;
            self.1.insert(owner.into(), collab(t)?);
        }
        for (owner, value) in &self.1 {
            let wanted: Vec<_> = self.0.iter().filter(|s| s.owner == owner).collect();
            let rows = value["submitted"].as_array().ok_or("missing submitted list")?;
            if rows.len() != wanted.len() { return Err(format!("{owner}: unanswered script operations")); }
            for (row, script) in rows.iter().zip(wanted) {
                if row["id"].as_u64() != Some(script.id) { return Err(format!("{owner}: missing or duplicate id: {rows:?}")); }
                match script.refusal {
                    Some(reason) if row["refused"] == reason && row.get("global").is_none() => (),
                    None if row.get("refused").is_none() => {
                        let global = row["global"].as_u64().filter(|g| *g > 0).ok_or("missing ack global")?;
                        if ordered.insert(global, script).is_some() { return Err("duplicate ack global".into()); }
                    }
                    _ => return Err(format!("{owner}: wrong answer {row}")),
                }
            }
        }
        if self.0.iter().any(|s| !self.1.contains_key(s.owner)) { return Err("unwitnessed script owner".into()); }
        let mut live = Vec::<&Submitted>::new();
        for (index, (&global, &script)) in ordered.iter().enumerate() {
            if global != index as u64 + 1 { return Err(format!("global sequence gap at {global}")); }
            match script.op {
                Op::Undo => {
                    let index = live.iter().rposition(|s| s.owner == script.owner).ok_or("accepted undo without owned stroke")?;
                    live.remove(index);
                }
                Op::Stroke { .. } => live.push(script),
            }
        }
        let mut expected = Expected::white();
        for script in live {
            if let Op::Stroke { y, color } = script.op { expected.colored_horizontal(10, 40, y, color); }
        }
        let globals = serde_json::json!((1..=ordered.len() as u64).collect::<Vec<_>>());
        Ok((expected, globals))
    }
    fn converge(&mut self, t: &mut App, owners: &[&str]) -> Result<(), String> {
        let (expected, globals) = self.replay(t, owners)?;
        for &owner in owners { check_peer(t, owner, &expected, &globals)?; }
        Ok(())
    }
}
fn title(name: &str) -> String { format!("Gus Kit ({name})") }
fn attach(t: &mut App, name: &str) -> Result<(), String> { t.attach(&title(name)) }
fn host(port: u16) -> Result<App, String> {
    App::launch(&[b"/usr/bin/guskit", b"--canvas", b"200x120", b"--host", port.to_string().as_bytes(), b"--name", HOST.as_bytes()], &title(HOST))
}
fn join(t: &mut App, port: u16, name: &str) -> Result<(), String> {
    t.launch_next(&[b"/usr/bin/guskit", b"--join", port.to_string().as_bytes(), b"--name", name.as_bytes()], &title(name))?;
    attach(t, name)?;
    t.wait_node("collab", |v| v["state"] == "connected" && v["name"] == name)
}
fn collab(t: &mut App) -> Result<Value, String> {
    let tree = t.tree()?;
    tree.children.iter().find(|n| n.key == "collab").map(|n| n.value.clone()).ok_or("missing collab node".into())
}
fn check_peer(t: &mut App, owner: &str, expected: &Expected, globals: &Value) -> Result<(), String> {
    attach(t, owner)?;
    t.wait_node("collab", |v| v["applied"] == *globals)?;
    let digest = expected.digest();
    t.wait_node("canvas", |v| v["width"] == 200 && v["height"] == 120 && v["sha256"] == digest)
}
fn close(t: &mut App, owner: &str) -> Result<(), String> {
    attach(t, owner)?;
    t.wait_node("status", |v| v["dirty"] == true)?;
    t.key(16, &[Modifier::Ctrl])?;
    t.wait_node("confirm", |_| true)?;
    t.key_exit(32, 0)
}

pub fn join_sync() -> CaseResult {
    let mut t = host(23701)?;
    let mut script = Script::new();
    script.stroke(&mut t, HOST, 10, false)?;
    join(&mut t, 23701, PEER)?;
    script.converge(&mut t, &[HOST, PEER])?;
    // In addition to the independent replay, bind the joiner's digest to the host's.
    attach(&mut t, HOST)?;
    let tree = t.tree()?;
    let host_canvas = tree.children.iter().find(|n| n.key == "canvas").ok_or("host canvas missing")?.value.clone();
    attach(&mut t, PEER)?;
    t.expect_node("canvas", |v| *v == host_canvas)?;
    Ok(t.finish()?)
}
pub fn stroke_both_ways() -> CaseResult {
    let mut t = host(23702)?;
    join(&mut t, 23702, PEER)?;
    let mut script = Script::new();
    script.stroke(&mut t, HOST, 10, false)?;
    script.converge(&mut t, &[HOST, PEER])?;
    script.stroke(&mut t, PEER, 20, true)?;
    script.converge(&mut t, &[HOST, PEER])?;
    Ok(t.finish()?)
}
// The host is a process launched by this case. Hold its sequencing path until
// both real peer apps have submitted; Drop releases it on failure paths too.
struct SequencingBarrier(i32);
impl SequencingBarrier {
    fn hold(window: u64) -> Result<Self, String> {
        let mut windows=vec![libbreenix::window::abi::WinInfo::default();64];
        let n=libbreenix::window::list(&mut windows).map_err(|e|e.to_string())?;
        let pid=windows[..n].iter().find(|w|w.id==window).ok_or("host window missing")?.owner as i32;
        libbreenix::signal::kill(pid,libbreenix::signal::SIGSTOP).map_err(|e|e.to_string())?;
        let barrier=Self(pid);
        let until=std::time::Instant::now()+std::time::Duration::from_secs(1);
        loop {
            let stat=std::fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|e|e.to_string())?;
            if stat.rsplit_once(") ").is_some_and(|(_,tail)|tail.starts_with("T ")) { return Ok(barrier); }
            if std::time::Instant::now()>=until { return Err("host did not reach sequencing barrier".into()); }
            libbreenix::time::sleep_ms(1).map_err(|e|e.to_string())?;
        }
    }
}
impl Drop for SequencingBarrier {
    fn drop(&mut self) { let _=libbreenix::signal::kill(self.0,libbreenix::signal::SIGCONT); }
}
pub fn global_order() -> CaseResult {
    let mut t = host(23703)?;
    join(&mut t, 23703, PEER)?;
    join(&mut t, 23703, THIRD)?;
    attach(&mut t,HOST)?;
    let barrier=SequencingBarrier::hold(t.window())?;
    let mut script = Script::new();
    script.stroke(&mut t, PEER, 10, false)?;
    script.stroke(&mut t, THIRD, 10, true)?;
    for owner in [PEER,THIRD] {
        attach(&mut t,owner)?;
        t.expect_node("collab",|v|v["submitted"]==serde_json::json!([{"id":1}]) && v["applied"]==serde_json::json!([]))?;
        let white=Expected::white().digest();
        t.expect_node("canvas",|v|v["sha256"]==white)?;
    }
    // Opposite local drawing orders would leave different colors. Both pending
    // peers must instead paint only the host's order after the shared release.
    drop(barrier);
    script.converge(&mut t, &[HOST, PEER, THIRD])?;
    Ok(t.finish()?)
}
pub fn three_peers() -> CaseResult {
    let mut t = host(23704)?;
    join(&mut t, 23704, PEER)?;
    join(&mut t, 23704, THIRD)?;
    let mut script = Script::new();
    script.stroke(&mut t, HOST, 10, false)?;
    script.stroke(&mut t, PEER, 20, true)?;
    script.stroke(&mut t, THIRD, 30, false)?;
    script.converge(&mut t, &[HOST, PEER, THIRD])?;
    Ok(t.finish()?)
}
pub fn undo_ownership() -> CaseResult {
    let mut t = host(23705)?;
    join(&mut t, 23705, PEER)?;
    let mut script = Script::new();
    script.stroke(&mut t, HOST, 10, false)?;
    script.stroke(&mut t, PEER, 20, true)?;
    script.undo(&mut t, HOST, false)?;
    script.converge(&mut t, &[HOST, PEER])?;
    script.undo(&mut t, HOST, true)?;
    let (expected, globals)=script.replay(&mut t,&[HOST,PEER])?;
    for owner in [PEER,HOST] { check_peer(&mut t,owner,&expected,&globals)?; }
    t.key(48,&[])?; // Brush: many segments must remain one Undo gesture.
    t.drag("canvas",(10,40),&vec![(1,0);59])?;
    t.wait_node("status",|v|v["undo"]==1)?;
    let digest=expected.digest();
    t.expect_node("canvas",|v|v["sha256"]!=digest)?;
    t.key(44,&[Modifier::Ctrl])?;
    t.wait_node("status",|v|v["undo"]==0)?;
    let value=collab(&mut t)?;
    let rows=value["submitted"].as_array().ok_or("missing gesture answers")?;
    if rows.len()!=64 || rows[3..].iter().any(|row|row.get("global").is_none()) {
        return Err(format!("brush segments or Undo unanswered: {rows:?}").into());
    }
    for owner in [HOST,PEER] { check_peer(&mut t,owner,&expected,&value["applied"])?; }
    Ok(t.finish()?)
}
pub fn peer_leaves() -> CaseResult {
    let mut t = host(23706)?;
    join(&mut t, 23706, PEER)?;
    join(&mut t, 23706, THIRD)?;
    let mut script = Script::new();
    script.stroke(&mut t, PEER, 10, true)?;
    script.converge(&mut t, &[HOST, PEER, THIRD])?;
    close(&mut t, PEER)?;
    for owner in [HOST, THIRD] {
        attach(&mut t, owner)?;
        t.wait_node("status", |v| v["message"] == "Peer left")?;
    }
    script.stroke(&mut t, HOST, 20, false)?;
    script.stroke(&mut t, THIRD, 30, false)?;
    script.converge(&mut t, &[HOST, THIRD])?;
    close(&mut t, HOST)?;
    attach(&mut t, THIRD)?;
    t.wait_node("collab", |v| v["state"] == "disconnected" && v["reason"] == "disconnected")?;
    t.wait_node("status", |v| v["message"] == "disconnected: disconnected")?;
    Ok(t.finish()?)
}
