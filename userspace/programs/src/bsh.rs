//! Breenish Shell (bsh) - ECMAScript-powered shell for Breenix OS
//!
//! A shell with a full ECMAScript scripting language. Processes and
//! subprocesses are managed through async/await and Promises.
//!
//! Phase 1: Minimal interpreter that evaluates JavaScript expressions
//! via REPL or script file.
//!
//! Usage:
//!   bsh              # Interactive REPL
//!   bsh script.js    # Execute a script file
//!   bsh -e 'code'    # Evaluate a string

use std::io::{self, Read, Write};

fn print_fn(s: &str) {
    let _ = io::stdout().write_all(s.as_bytes());
    let _ = io::stdout().flush();
}

fn run_repl() {
    let mut ctx = breenish_js::Context::new();
    ctx.set_print_fn(print_fn);

    let _ = io::stdout().write_all(b"breenish v0.1.0 -- ECMAScript shell for Breenix\n");
    let _ = io::stdout().flush();

    let mut line_buf = Vec::new();

    loop {
        let _ = io::stdout().write_all(b"bsh> ");
        let _ = io::stdout().flush();

        // Read one line from stdin
        line_buf.clear();
        loop {
            let mut byte = [0u8; 1];
            match io::stdin().read(&mut byte) {
                Ok(0) => return, // EOF
                Ok(_) => {
                    if byte[0] == b'\n' {
                        break;
                    }
                    line_buf.push(byte[0]);
                }
                Err(_) => return,
            }
        }

        let line = match std::str::from_utf8(&line_buf) {
            Ok(s) => s.trim(),
            Err(_) => continue,
        };

        if line.is_empty() {
            continue;
        }

        if line == "exit" || line == "quit" {
            break;
        }

        match ctx.eval(line) {
            Ok(_) => {}
            Err(e) => {
                let msg = format!("{}\n", e);
                let _ = io::stderr().write_all(msg.as_bytes());
            }
        }
    }
}

fn run_string(code: &str) {
    let mut ctx = breenish_js::Context::new();
    ctx.set_print_fn(print_fn);

    match ctx.eval(code) {
        Ok(_) => {}
        Err(e) => {
            let msg = format!("{}\n", e);
            let _ = io::stderr().write_all(msg.as_bytes());
            std::process::exit(1);
        }
    }
}

fn run_file(path: &str) {
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            let msg = format!("bsh: cannot open '{}': {}\n", path, e);
            let _ = io::stderr().write_all(msg.as_bytes());
            std::process::exit(1);
        }
    };

    let mut source = String::new();
    if let Err(e) = file.read_to_string(&mut source) {
        let msg = format!("bsh: cannot read '{}': {}\n", path, e);
        let _ = io::stderr().write_all(msg.as_bytes());
        std::process::exit(1);
    }

    let mut ctx = breenish_js::Context::new();
    ctx.set_print_fn(print_fn);

    match ctx.eval(&source) {
        Ok(_) => {}
        Err(e) => {
            let msg = format!("{}\n", e);
            let _ = io::stderr().write_all(msg.as_bytes());
            std::process::exit(1);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() == 1 {
        // No arguments: interactive REPL
        run_repl();
    } else if args.len() == 3 && args[1] == "-e" {
        // -e 'code': evaluate string
        run_string(&args[2]);
    } else if args.len() == 2 {
        // script file
        run_file(&args[1]);
    } else {
        let _ = io::stderr().write_all(b"Usage: bsh [script.js | -e 'code']\n");
        std::process::exit(1);
    }
}
