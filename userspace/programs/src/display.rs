//! List monitor modes or ask bwm to change the current mode.
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.is_empty() {
        let (result, modes) = libbreenix::gpu::modes();
        if result != 0 { return Err(format!("MODES errno {}", -result)); }
        for (index, mode) in modes.mode_list() {
            if mode.scale == 0 { continue; }
            println!("{}x{} scale {}{}{}", mode.width / mode.scale, mode.height / mode.scale, mode.scale,
                if index == modes.current { " current" } else { "" }, if index == 1 { " default" } else { "" });
        }
        return Ok(());
    }
    if args.len() != 2 || (args[0] != "mode" && args[0] != "scale") || args.iter().any(|a| a.contains(['\n', '\r'])) {
        return Err("usage: display [mode WIDTHxHEIGHT | scale 1|2]".into());
    }
    let mut stream = UnixStream::connect("/run/bwm/display").map_err(|e| format!("connect bwm: {e}"))?;
    writeln!(stream, "{} {}", args[0], args[1]).map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).map_err(|e| e.to_string())?;
    if line.starts_with("ok ") { print!("{line}"); Ok(()) }
    else { Err(if line.is_empty() { "no reply from bwm".into() } else { line.trim_end().into() }) }
}
fn main() { if let Err(e) = run() { eprintln!("display: {e}"); std::process::exit(1); } }
