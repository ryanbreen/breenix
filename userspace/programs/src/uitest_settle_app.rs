//! Controlled BreenGL producer: event draining and accepted commits are distinct.
use breengel::{Event, Handled, Window};
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    time::Duration,
};

fn paint(window: &mut Window, saved: bool) -> Result<(), String> {
    window.acquire(500_000_000).map_err(|e| e.to_string())?;
    window
        .surface()
        .map_err(|e| e.to_string())?
        .batch(|surface| {
            surface.clear(if saved {
                libgfx::color::Color::rgb(0, 128, 0)
            } else {
                libgfx::color::Color::rgb(32, 48, 64)
            });
        });
    // Explicit tree bytes also let the controller inspect the initial publication
    // without waking the producer just to attach a tree reader.
    let tree = format!("{{\"key\":\"root\",\"parent\":null,\"role\":\"window\",\"name\":\"settle\",\"value\":\"{}\",\"bounds\":[0,0,320,200],\"clip\":[0,0,320,200],\"flags\":[\"enabled\",\"visible\"]}}\0",
        if saved { "Saved" } else { "Waiting" });
    window
        .write_tree(tree.as_bytes())
        .map_err(|e| e.to_string())?;
    window.present().map_err(|e| e.to_string())
}

pub fn run(title: &str, drain_no_render: bool) -> Result<(), String> {
    let root =
        std::env::var_os("BREENIX_CONFIG_ROOT").ok_or("settle control requires session root")?;
    let mut socket = UnixStream::connect(std::path::Path::new(&root).join("settle-control.sock"))
        .map_err(|e| e.to_string())?;
    socket.set_nonblocking(true).map_err(|e| e.to_string())?;
    let mut window =
        Window::new_without_fonts(title.as_bytes(), 320, 200).map_err(|e| e.to_string())?;
    let mut saved = false;
    paint(&mut window, saved)?;
    writeln!(socket, "ready {}", window.id()).map_err(|e| e.to_string())?;
    libuitest::startup::mark("app-commit");
    let mut held = false;
    let mut no_change = false;
    let mut blocked = false;
    let mut pending = Vec::new();
    loop {
        let mut drained = Vec::new();
        let mut dirty = false;
        let count = window
            .dispatch(|event| {
                if event.serial != 0 {
                    drained.push(event.serial);
                }
                if matches!(
                    event.event,
                    Event::KeyPress { .. } | Event::KeyRelease { .. } | Event::Text { .. }
                ) {
                    blocked |= drain_no_render;
                }
                let redraw = !no_change && !matches!(event.event, Event::KeyRelease { .. });
                dirty |= redraw;
                if redraw {
                    Handled::Redraw
                } else {
                    Handled::NoChange
                }
            })
            .map_err(|e| e.to_string())?;
        // The witness follows handler return, and precedes any app-issued commit.
        for serial in drained {
            writeln!(socket, "drained {serial}").map_err(|e| e.to_string())?;
        }
        if count != 0 && !held && !blocked {
            if dirty {
                paint(&mut window, saved)?;
            } else {
                window.commit_no_change().map_err(|e| e.to_string())?;
            }
        }
        let mut bytes = [0; 256];
        match socket.read(&mut bytes) {
            Ok(0) => return Ok(()),
            Ok(n) => pending.extend_from_slice(&bytes[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.to_string()),
        }
        if pending.len() > 1024 {
            return Err("settle control command too long".into());
        }
        while let Some(end) = pending.iter().position(|b| *b == b'\n') {
            let command =
                String::from_utf8(pending.drain(..=end).collect()).map_err(|e| e.to_string())?;
            let command = command.trim_end();
            match command {
                "hold" => {
                    held = true;
                    no_change = false;
                }
                "no-change" => {
                    held = true;
                    no_change = true;
                }
                "pixel" | "save" if !blocked => {
                    saved |= command == "save";
                    paint(&mut window, saved)?;
                }
                "ack" if !blocked => {
                    window.commit_no_change().map_err(|e| e.to_string())?;
                }
                "hang" => {
                    writeln!(socket, "ok hang").map_err(|e| e.to_string())?;
                    loop {
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
                "quit" => return Ok(()),
                _ => return Err(format!("invalid settle command {command:?}")),
            }
            writeln!(socket, "ok {command}").map_err(|e| e.to_string())?;
        }
        libbreenix::time::sleep_ms(1).map_err(|e| e.to_string())?;
    }
}
