//! Small application used by UI session tests.
mod uitest_app_support;

use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use breengel::{Event, Window};

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    let env: Vec<Vec<u8>> = std::env::vars_os().map(|(key, value)| {
        let mut entry = key.as_bytes().to_vec();
        entry.push(b'=');
        entry.extend_from_slice(value.as_bytes());
        entry
    }).collect();
    let arg_bytes: Vec<&[u8]> = args.iter().map(|s| s.as_bytes()).collect();
    let env_bytes: Vec<&[u8]> = env.iter().map(Vec::as_slice).collect();
    println!("{}", uitest_app_support::echo(&arg_bytes, &env_bytes));
    let _ = io::stdout().flush();
    let strings: Vec<&str> = match args.iter().skip(1).map(|s| s.to_str().ok_or("non-UTF-8 argument")).collect() {
        Ok(strings) => strings,
        Err(error) => { eprintln!("{error}"); std::process::exit(2); }
    };
    let options = match uitest_app_support::parse(&strings) {
        Ok(options) => options,
        Err(error) => { eprintln!("{error}"); std::process::exit(2); }
    };
    let mut win = Window::new_without_fonts(options.title.as_bytes(), 320, 200)
        .unwrap_or_else(|error| { eprintln!("window: {error}"); std::process::exit(1); });
    win.surface().unwrap_or_else(|error| { eprintln!("surface: {error}"); std::process::exit(1); });
    win.present().unwrap_or_else(|error| { eprintln!("present: {error}"); std::process::exit(1); });
    let block = [b'x'; 4096];
    let mut remaining = options.stderr_flood;
    let mut stderr = io::stderr().lock();
    while remaining != 0 {
        let count = remaining.min(block.len());
        if stderr.write_all(&block[..count]).is_err() { break; }
        remaining -= count;
    }
    drop(stderr);
    if let Some(signal) = options.crash {
        let _ = libbreenix::signal::kill(libbreenix::process::getpid().expect("getpid").raw() as i32, signal);
    }
    if options.hang { loop { let _ = libbreenix::time::sleep_ms(1000); } }
    loop {
        if win.wait_event().iter().any(|event| matches!(event, Event::CloseRequested)) { break; }
    }
}
