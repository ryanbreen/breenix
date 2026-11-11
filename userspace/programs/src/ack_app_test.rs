//! Acknowledge the process context received from an application launcher.
use breengel::{Event, Handled, Window};
use std::io::Write;

fn run() -> Result<(), Box<dyn std::error::Error>> {
    extern "C" { fn getppid() -> i32; }
    let argv: Vec<String> = std::env::args().collect();
    let env: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    let cwd = std::env::current_dir()?;
    let record = serde_json::json!({
        "pid": std::process::id(), "ppid": unsafe { getppid() },
        "argv": argv, "cwd": cwd.to_str().ok_or("cwd is not UTF-8")?, "env": env,
    });
    let mut line = serde_json::to_vec(&record)?;
    line.push(b'\n');
    {
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/ack-app.jsonl")?;
        file.write_all(&line)?;
    }
    let Some(option) = argv.windows(2).find(|pair| pair[0] == "--window") else { return Ok(()); };
    let mut win = Window::new_without_fonts(option[1].as_bytes(), 320, 200).map_err(|e| e.to_string())?;
    let bounds = [0, 0, 320, 200];
    win.surface().map_err(|e| e.to_string())?.clear(libgfx::color::Color::rgb(24, 32, 48));
    win.set_tree(breengel::tree::Tree {
        root: breengel::tree::Node {
            key: "ack".into(), parent: None, role: breengel::tree::Role::Window,
            name: option[1].clone(), value: record,
            bounds, clip: bounds,
            flags: vec![breengel::tree::Flag::Enabled, breengel::tree::Flag::Visible],
        }, children: Vec::new(),
    });
    win.present().map_err(|e| e.to_string())?;
    let fault_page = if argv.iter().any(|arg| arg == "--fault-key") {
        use libbreenix::memory::*;
        let page = mmap(std::ptr::null_mut(), 4096, PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS, -1, 0).map_err(|e| e.to_string())?;
        Some(page)
    } else { None };
    loop {
        let mut close = false;
        win.wait_dispatch(100_000_000, |delivered| {
            close |= matches!(delivered.event, Event::CloseRequested);
            if matches!(delivered.event, Event::KeyPress { keycode: 33, .. }) {
                if let Some(page) = fault_page {
                    // Unmap immediately before the write so no allocator can reuse the page.
                    if libbreenix::memory::munmap(page, 4096).is_err() { return Handled::NoChange; }
                    unsafe { std::ptr::write_volatile(page, 1); }
                }
            }
            Handled::NoChange
        }).map_err(|e| e.to_string())?;
        win.commit_no_change().map_err(|e| e.to_string())?;
        if close { break; }
        // A new semantic reader needs a serialization newer than its attach.
        // Metadata-only acknowledgments retain the old tree publication.
        if win.tree_reader_count() != 0 {
            win.surface().map_err(|e| e.to_string())?.clear(libgfx::color::Color::rgb(24, 32, 48));
            win.present().map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("ack-app_test: {error}");
        std::process::exit(1);
    }
}
