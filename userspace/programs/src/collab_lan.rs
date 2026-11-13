#[path = "guskit/canvas.rs"] mod canvas;
#[path = "guskit/collab.rs"] mod collab;
#[path = "guskit/node.rs"] mod node;
#[path = "collab_lan_script.rs"] mod script;

use libcollab::peer::Response;
use std::time::{Duration, Instant};

fn run(host: bool) -> Result<[u8; 32], String> {
    // Probe stops children at 60 seconds; reserve time to report failure.
    let deadline = Instant::now() + Duration::from_secs(58);
    let mut session = if host {
        let mut session = collab::Session::host([0, 0, 0, 0], 7890, "host", canvas::Canvas::new(64, 64), 64, 64)
            .map_err(|e| format!("bind-{e:?}"))?;
        while session.core.model.peers.is_empty() {
            if Instant::now() >= deadline { return Err("peer-timeout".into()); }
            session.poll(10);
        }
        session
    } else {
        loop {
            if Instant::now() >= deadline { return Err("connect-timeout".into()); }
            if let Ok(mut session) = collab::Session::join([10, 0, 2, 2], 7890, "join") {
                while session.core.model.state != "connected" && session.core.model.state != "disconnected" && Instant::now() < deadline {
                    session.poll(10);
                }
                if session.core.model.state == "connected" { break session; }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    for stroke in script::strokes() { session.core.submit_stroke(stroke); }
    loop {
        session.poll(10);
        let model = &session.core.model;
        if model.state == "disconnected" { return Err("disconnected".into()); }
        if model.submitted.iter().any(|id| matches!(model.peer.response(*id), Some(Response::Refused(_)))) {
            return Err("refused".into());
        }
        if model.peer.log().len() == 8 && model.submitted.len() == 4 &&
            model.submitted.iter().all(|id| matches!(model.peer.response(*id), Some(Response::Ack { .. }))) {
            let digest = model.canvas.digest();
            // Give the other side time to receive its final frames before closing TCP.
            let until = deadline.min(Instant::now() + Duration::from_secs(2));
            while Instant::now() < until { session.poll(10); }
            return Ok(digest);
        }
        if Instant::now() >= deadline { return Err("operations-timeout".into()); }
    }
}

fn main() {
    let host = std::env::args().next().is_some_and(|name| name.ends_with("collab-lan-host"));
    let role = if host { "host" } else { "join" };
    match run(host) {
        Ok(digest) => {
            let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
            println!("COLLAB_LAN role={role} ops=8 digest={hex}");
        }
        Err(reason) => { println!("COLLAB_LAN role={role} FAIL reason={reason}"); std::process::exit(1); }
    }
}
