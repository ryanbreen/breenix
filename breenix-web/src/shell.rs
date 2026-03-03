//! Breenix Shell — command line interpreter
//!
//! Supports:
//! - Single commands: `ls -la /etc`
//! - Pipelines: `cat /etc/passwd | grep root | wc`
//! - Output redirects: `echo hello > file.txt`, `echo world >> file.txt`
//! - Input redirects: `wc < file.txt`

use crate::commands;
use crate::kernel::WasmKernel;
use alloc::string::String;
use alloc::vec::Vec;

/// A single command in a pipeline
struct Command {
    /// The command name and arguments
    argv: Vec<String>,
    /// Input redirect: `< file`
    stdin_file: Option<String>,
    /// Output redirect: `> file` or `>> file`
    stdout_file: Option<String>,
    /// Whether to append (>>) vs overwrite (>)
    append: bool,
}

/// Parse a command line into pipeline stages
fn parse_pipeline(line: &str) -> Vec<Command> {
    let stages: Vec<&str> = line.split('|').collect();
    let mut commands = Vec::new();

    for stage in stages {
        let trimmed = stage.trim();
        if trimmed.is_empty() {
            continue;
        }

        let mut argv: Vec<String> = Vec::new();
        let mut stdin_file = None;
        let mut stdout_file = None;
        let mut append = false;

        let tokens = tokenize(trimmed);
        let mut i = 0;
        while i < tokens.len() {
            let token = &tokens[i];
            if token == ">" {
                if i + 1 < tokens.len() {
                    stdout_file = Some(tokens[i + 1].clone());
                    append = false;
                    i += 2;
                    continue;
                }
            } else if token == ">>" {
                if i + 1 < tokens.len() {
                    stdout_file = Some(tokens[i + 1].clone());
                    append = true;
                    i += 2;
                    continue;
                }
            } else if token == "<" {
                if i + 1 < tokens.len() {
                    stdin_file = Some(tokens[i + 1].clone());
                    i += 2;
                    continue;
                }
            } else {
                argv.push(token.clone());
            }
            i += 1;
        }

        if !argv.is_empty() {
            commands.push(Command {
                argv,
                stdin_file,
                stdout_file,
                append,
            });
        }
    }
    commands
}

/// Simple tokenizer: splits on whitespace, respects double quotes
fn tokenize(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '"' {
            in_quotes = !in_quotes;
        } else if ch == '>' && !in_quotes {
            if !current.is_empty() {
                tokens.push(core::mem::take(&mut current));
            }
            if chars.peek() == Some(&'>') {
                chars.next();
                tokens.push(String::from(">>"));
            } else {
                tokens.push(String::from(">"));
            }
        } else if ch == '<' && !in_quotes {
            if !current.is_empty() {
                tokens.push(core::mem::take(&mut current));
            }
            tokens.push(String::from("<"));
        } else if ch.is_whitespace() && !in_quotes {
            if !current.is_empty() {
                tokens.push(core::mem::take(&mut current));
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Execute a command line (may be a pipeline)
///
/// Returns (output_string, exit_code)
pub fn execute(kernel: &mut WasmKernel, line: &str) -> (String, i32) {
    let line = line.trim();
    if line.is_empty() {
        return (String::new(), 0);
    }

    // Handle variable expansion
    let expanded = expand_variables(kernel, line);
    let commands = parse_pipeline(&expanded);

    if commands.is_empty() {
        return (String::new(), 0);
    }

    let mut stdin_data: Vec<u8> = Vec::new();
    let mut last_exit_code = 0;

    for (i, cmd) in commands.iter().enumerate() {
        let cmd_name = cmd.argv[0].as_str();
        let argv_refs: Vec<&str> = cmd.argv.iter().map(|s| s.as_str()).collect();

        // Handle input redirect
        if let Some(ref file) = cmd.stdin_file {
            let resolved = kernel.resolve_path(file);
            match kernel.fs.read_file(&resolved) {
                Ok(data) => stdin_data = data,
                Err(_) => {
                    let err = alloc::format!("-bsh: {}: No such file or directory\n", file);
                    return (err, 1);
                }
            }
        }

        // Execute the command
        let (output, exit_code) = if let Some(cmd_fn) = commands::lookup(cmd_name) {
            cmd_fn(kernel, &argv_refs, &stdin_data)
        } else {
            (alloc::format!("-bsh: {}: command not found\n", cmd_name).into_bytes(), 127)
        };

        last_exit_code = exit_code;

        // Handle output redirect
        if let Some(ref file) = cmd.stdout_file {
            let resolved = kernel.resolve_path(file);
            let _ = kernel.fs.create_or_get(&resolved, 0o644);
            if cmd.append {
                let _ = kernel.fs.append_file(&resolved, &output);
            } else {
                let _ = kernel.fs.write_file(&resolved, &output);
            }
            stdin_data = Vec::new();
        } else if i < commands.len() - 1 {
            // Pipe: output becomes input for next command
            stdin_data = output;
        } else {
            // Last command: output goes to terminal
            stdin_data = output;
        }
    }

    kernel.last_exit_code = last_exit_code;

    let output = String::from_utf8(stdin_data).unwrap_or_default();
    (output, last_exit_code)
}

/// Expand environment variables ($VAR and $?)
fn expand_variables(kernel: &WasmKernel, line: &str) -> String {
    let mut result = String::new();
    let mut chars = line.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '$' {
            if chars.peek() == Some(&'?') {
                chars.next();
                result.push_str(&alloc::format!("{}", kernel.last_exit_code));
            } else {
                let mut var_name = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_alphanumeric() || c == '_' {
                        var_name.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if let Some(value) = kernel.env.get(&var_name) {
                    result.push_str(value);
                }
                // If variable not found, expand to empty string (POSIX behavior)
            }
        } else {
            result.push(ch);
        }
    }
    result
}
