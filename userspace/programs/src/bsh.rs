//! Breenish Shell (bsh) - ECMAScript-powered shell for Breenix OS
//!
//! A shell with a full ECMAScript scripting language. Processes and
//! subprocesses are managed through async/await and Promises.
//!
//! Usage:
//!   bsh              # Interactive REPL
//!   bsh script.js    # Execute a script file
//!   bsh -e 'code'    # Evaluate a string

use std::io::{self, Read, Write};

use breenish_js::error::{JsError, JsResult};
use breenish_js::object::{JsObject, ObjectHeap};
use breenish_js::string::StringPool;
use breenish_js::value::JsValue;
use breenish_js::Context;

// ---------------------------------------------------------------------------
// Native function implementations
// ---------------------------------------------------------------------------

/// exec(cmd, arg1, arg2, ...) -> { exitCode, stdout, stderr }
///
/// Forks a child process, executes the command, waits for it to finish,
/// and returns an object with the exit code and captured stdout/stderr.
fn native_exec(
    args: &[JsValue],
    strings: &mut StringPool,
    heap: &mut ObjectHeap,
) -> JsResult<JsValue> {
    if args.is_empty() {
        return Err(JsError::type_error("exec: expected at least one argument"));
    }

    // Collect command and arguments as strings
    let mut cmd_args: Vec<String> = Vec::new();
    for arg in args {
        if arg.is_string() {
            cmd_args.push(String::from(strings.get(arg.as_string_id())));
        } else if arg.is_number() {
            cmd_args.push(format!("{}", arg.to_number()));
        } else {
            cmd_args.push(String::from("undefined"));
        }
    }

    let cmd = &cmd_args[0];

    // Resolve command path
    let resolved = resolve_command(cmd);
    let exec_path = resolved.as_deref().unwrap_or(cmd.as_str());

    // Create pipes for stdout and stderr capture
    let (stdout_r, stdout_w) = match libbreenix::io::pipe() {
        Ok(p) => p,
        Err(_) => return Err(JsError::runtime("exec: pipe() failed")),
    };
    let (stderr_r, stderr_w) = match libbreenix::io::pipe() {
        Ok(p) => p,
        Err(_) => return Err(JsError::runtime("exec: pipe() failed")),
    };

    // Fork
    let fork_result = match libbreenix::process::fork() {
        Ok(r) => r,
        Err(_) => return Err(JsError::runtime("exec: fork() failed")),
    };

    match fork_result {
        libbreenix::process::ForkResult::Child => {
            // Child: redirect stdout/stderr to pipes, close read ends
            let _ = libbreenix::io::close(stdout_r);
            let _ = libbreenix::io::close(stderr_r);
            let _ = libbreenix::io::dup2(stdout_w, libbreenix::types::Fd::STDOUT);
            let _ = libbreenix::io::dup2(stderr_w, libbreenix::types::Fd::STDERR);
            let _ = libbreenix::io::close(stdout_w);
            let _ = libbreenix::io::close(stderr_w);

            // Build null-terminated argv
            let mut c_args: Vec<Vec<u8>> = Vec::new();
            for a in &cmd_args {
                let mut v: Vec<u8> = a.as_bytes().to_vec();
                v.push(0);
                c_args.push(v);
            }
            let argv_ptrs: Vec<*const u8> = c_args.iter().map(|a| a.as_ptr()).collect();

            // Build null-terminated path
            let mut path_bytes: Vec<u8> = exec_path.as_bytes().to_vec();
            path_bytes.push(0);

            // execv
            let mut argv_with_null: Vec<*const u8> = argv_ptrs;
            argv_with_null.push(core::ptr::null());
            let _ = libbreenix::process::execv(&path_bytes, argv_with_null.as_ptr());

            // If exec failed, exit with 127
            libbreenix::process::exit(127);
        }
        libbreenix::process::ForkResult::Parent(child_pid) => {
            // Parent: close write ends, read from pipes
            let _ = libbreenix::io::close(stdout_w);
            let _ = libbreenix::io::close(stderr_w);

            // Read stdout
            let stdout_str = read_fd_to_string(stdout_r);
            let _ = libbreenix::io::close(stdout_r);

            // Read stderr
            let stderr_str = read_fd_to_string(stderr_r);
            let _ = libbreenix::io::close(stderr_r);

            // Wait for child
            let mut status: i32 = 0;
            let _ = libbreenix::process::waitpid(
                child_pid.raw() as i32,
                &mut status as *mut i32,
                0,
            );

            let exit_code = if libbreenix::process::wifexited(status) {
                libbreenix::process::wexitstatus(status)
            } else {
                -1
            };

            // Build result object: { exitCode, stdout, stderr, pid }
            let mut obj = JsObject::new();
            let k_exit = strings.intern("exitCode");
            let k_stdout = strings.intern("stdout");
            let k_stderr = strings.intern("stderr");
            let k_pid = strings.intern("pid");

            obj.set(k_exit, JsValue::number(exit_code as f64));

            let stdout_id = strings.intern(&stdout_str);
            obj.set(k_stdout, JsValue::string(stdout_id));

            let stderr_id = strings.intern(&stderr_str);
            obj.set(k_stderr, JsValue::string(stderr_id));

            obj.set(k_pid, JsValue::number(child_pid.raw() as f64));

            let idx = heap.alloc(obj);
            Ok(JsValue::object(idx))
        }
    }
}

/// cd(path) -> undefined
fn native_cd(
    args: &[JsValue],
    strings: &mut StringPool,
    _heap: &mut ObjectHeap,
) -> JsResult<JsValue> {
    let path = if args.is_empty() {
        // cd with no args goes to "/"
        String::from("/")
    } else if args[0].is_string() {
        String::from(strings.get(args[0].as_string_id()))
    } else {
        return Err(JsError::type_error("cd: expected string path"));
    };

    let mut path_bytes: Vec<u8> = path.as_bytes().to_vec();
    path_bytes.push(0);

    match libbreenix::process::chdir(&path_bytes) {
        Ok(()) => Ok(JsValue::undefined()),
        Err(_) => Err(JsError::runtime(format!("cd: {}: No such directory", path))),
    }
}

/// pwd() -> string
fn native_pwd(
    _args: &[JsValue],
    strings: &mut StringPool,
    _heap: &mut ObjectHeap,
) -> JsResult<JsValue> {
    let mut buf = [0u8; 1024];
    match libbreenix::process::getcwd(&mut buf) {
        Ok(len) => {
            let path = core::str::from_utf8(&buf[..len]).unwrap_or("");
            let id = strings.intern(path);
            Ok(JsValue::string(id))
        }
        Err(_) => Err(JsError::runtime("pwd: failed to get working directory")),
    }
}

/// which(cmd) -> string | null
fn native_which(
    args: &[JsValue],
    strings: &mut StringPool,
    _heap: &mut ObjectHeap,
) -> JsResult<JsValue> {
    if args.is_empty() || !args[0].is_string() {
        return Ok(JsValue::null());
    }

    let cmd = String::from(strings.get(args[0].as_string_id()));

    match resolve_command(&cmd) {
        Some(path) => {
            let id = strings.intern(&path);
            Ok(JsValue::string(id))
        }
        None => Ok(JsValue::null()),
    }
}

/// readFile(path) -> string
fn native_read_file(
    args: &[JsValue],
    strings: &mut StringPool,
    _heap: &mut ObjectHeap,
) -> JsResult<JsValue> {
    if args.is_empty() || !args[0].is_string() {
        return Err(JsError::type_error("readFile: expected string path"));
    }

    let path = String::from(strings.get(args[0].as_string_id()));

    match std::fs::read_to_string(&path) {
        Ok(contents) => {
            let id = strings.intern(&contents);
            Ok(JsValue::string(id))
        }
        Err(e) => Err(JsError::runtime(format!("readFile: {}: {}", path, e))),
    }
}

/// writeFile(path, data) -> undefined
fn native_write_file(
    args: &[JsValue],
    strings: &mut StringPool,
    _heap: &mut ObjectHeap,
) -> JsResult<JsValue> {
    if args.len() < 2 || !args[0].is_string() || !args[1].is_string() {
        return Err(JsError::type_error(
            "writeFile: expected (path: string, data: string)",
        ));
    }

    let path = String::from(strings.get(args[0].as_string_id()));
    let data = String::from(strings.get(args[1].as_string_id()));

    match std::fs::write(&path, data.as_bytes()) {
        Ok(()) => Ok(JsValue::undefined()),
        Err(e) => Err(JsError::runtime(format!("writeFile: {}: {}", path, e))),
    }
}

/// exit(code?) -> never returns
fn native_exit(
    args: &[JsValue],
    _strings: &mut StringPool,
    _heap: &mut ObjectHeap,
) -> JsResult<JsValue> {
    let code = if !args.is_empty() {
        args[0].to_number() as i32
    } else {
        0
    };
    std::process::exit(code);
}

/// pipe("cmd1 arg1", "cmd2 arg2", ...) -> { exitCode, stdout, stderr }
///
/// Creates a Unix pipeline connecting N commands via pipes. Each argument is
/// a string containing the command and its arguments, separated by whitespace.
/// Returns the result of the last command in the pipeline.
fn native_pipe(
    args: &[JsValue],
    strings: &mut StringPool,
    heap: &mut ObjectHeap,
) -> JsResult<JsValue> {
    if args.is_empty() {
        return Err(JsError::type_error("pipe: expected at least one command string"));
    }

    // Parse each argument into a command string
    let mut commands: Vec<Vec<String>> = Vec::new();
    for arg in args {
        if !arg.is_string() {
            return Err(JsError::type_error("pipe: each argument must be a command string"));
        }
        let cmd_str = String::from(strings.get(arg.as_string_id()));
        let parts: Vec<String> = cmd_str.split_whitespace().map(String::from).collect();
        if parts.is_empty() {
            return Err(JsError::type_error("pipe: empty command string"));
        }
        commands.push(parts);
    }

    let n = commands.len();

    // If only one command, just execute it directly (no pipes needed)
    if n == 1 {
        let mut exec_args = Vec::new();
        for part in &commands[0] {
            let id = strings.intern(part);
            exec_args.push(JsValue::string(id));
        }
        return native_exec(&exec_args, strings, heap);
    }

    // Create N-1 pipes for inter-process communication
    let mut pipes: Vec<(libbreenix::types::Fd, libbreenix::types::Fd)> = Vec::new();
    for _ in 0..(n - 1) {
        match libbreenix::io::pipe() {
            Ok(p) => pipes.push(p),
            Err(_) => {
                for (r, w) in &pipes {
                    let _ = libbreenix::io::close(*r);
                    let _ = libbreenix::io::close(*w);
                }
                return Err(JsError::runtime("pipe: pipe() syscall failed"));
            }
        }
    }

    // Create pipes to capture stdout and stderr of the last command
    let (last_stdout_r, last_stdout_w) = match libbreenix::io::pipe() {
        Ok(p) => p,
        Err(_) => {
            for (r, w) in &pipes {
                let _ = libbreenix::io::close(*r);
                let _ = libbreenix::io::close(*w);
            }
            return Err(JsError::runtime("pipe: pipe() syscall failed"));
        }
    };

    let (last_stderr_r, last_stderr_w) = match libbreenix::io::pipe() {
        Ok(p) => p,
        Err(_) => {
            for (r, w) in &pipes {
                let _ = libbreenix::io::close(*r);
                let _ = libbreenix::io::close(*w);
            }
            let _ = libbreenix::io::close(last_stdout_r);
            let _ = libbreenix::io::close(last_stdout_w);
            return Err(JsError::runtime("pipe: pipe() syscall failed"));
        }
    };

    // Fork each child in the pipeline
    let mut child_pids: Vec<i32> = Vec::new();
    for i in 0..n {
        let fork_result = match libbreenix::process::fork() {
            Ok(r) => r,
            Err(_) => {
                for (r, w) in &pipes {
                    let _ = libbreenix::io::close(*r);
                    let _ = libbreenix::io::close(*w);
                }
                let _ = libbreenix::io::close(last_stdout_r);
                let _ = libbreenix::io::close(last_stdout_w);
                let _ = libbreenix::io::close(last_stderr_r);
                let _ = libbreenix::io::close(last_stderr_w);
                for pid in &child_pids {
                    let mut status: i32 = 0;
                    let _ = libbreenix::process::waitpid(*pid, &mut status as *mut i32, 0);
                }
                return Err(JsError::runtime("pipe: fork() failed"));
            }
        };

        match fork_result {
            libbreenix::process::ForkResult::Child => {
                // Set up stdin: first child keeps original stdin,
                // others read from previous pipe
                if i > 0 {
                    let _ = libbreenix::io::dup2(pipes[i - 1].0, libbreenix::types::Fd::STDIN);
                }

                // Set up stdout: last child writes to capture pipe,
                // others write to next pipe
                if i < n - 1 {
                    let _ = libbreenix::io::dup2(pipes[i].1, libbreenix::types::Fd::STDOUT);
                } else {
                    let _ = libbreenix::io::dup2(last_stdout_w, libbreenix::types::Fd::STDOUT);
                    let _ = libbreenix::io::dup2(last_stderr_w, libbreenix::types::Fd::STDERR);
                }

                // Close all pipe fds in the child
                for (r, w) in &pipes {
                    let _ = libbreenix::io::close(*r);
                    let _ = libbreenix::io::close(*w);
                }
                let _ = libbreenix::io::close(last_stdout_r);
                let _ = libbreenix::io::close(last_stdout_w);
                let _ = libbreenix::io::close(last_stderr_r);
                let _ = libbreenix::io::close(last_stderr_w);

                // Resolve and exec the command
                let cmd = &commands[i][0];
                let resolved = resolve_command(cmd);
                let exec_path = resolved.as_deref().unwrap_or(cmd.as_str());

                let mut c_args: Vec<Vec<u8>> = Vec::new();
                for a in &commands[i] {
                    let mut v: Vec<u8> = a.as_bytes().to_vec();
                    v.push(0);
                    c_args.push(v);
                }
                let mut argv_ptrs: Vec<*const u8> = c_args.iter().map(|a| a.as_ptr()).collect();
                argv_ptrs.push(core::ptr::null());

                let mut path_bytes: Vec<u8> = exec_path.as_bytes().to_vec();
                path_bytes.push(0);

                let _ = libbreenix::process::execv(&path_bytes, argv_ptrs.as_ptr());
                libbreenix::process::exit(127);
            }
            libbreenix::process::ForkResult::Parent(child_pid) => {
                child_pids.push(child_pid.raw() as i32);
            }
        }
    }

    // Parent: close all inter-process pipe ends
    for (r, w) in &pipes {
        let _ = libbreenix::io::close(*r);
        let _ = libbreenix::io::close(*w);
    }
    let _ = libbreenix::io::close(last_stdout_w);
    let _ = libbreenix::io::close(last_stderr_w);

    // Read stdout and stderr of the last command
    let stdout_str = read_fd_to_string(last_stdout_r);
    let _ = libbreenix::io::close(last_stdout_r);

    let stderr_str = read_fd_to_string(last_stderr_r);
    let _ = libbreenix::io::close(last_stderr_r);

    // Wait for all children, capturing the exit code of the last one
    let last_pid = *child_pids.last().unwrap();
    let mut last_exit_code: i32 = -1;
    for pid in &child_pids {
        let mut status: i32 = 0;
        let _ = libbreenix::process::waitpid(*pid, &mut status as *mut i32, 0);
        if *pid == last_pid {
            last_exit_code = if libbreenix::process::wifexited(status) {
                libbreenix::process::wexitstatus(status)
            } else {
                -1
            };
        }
    }

    // Build result object: { exitCode, stdout, stderr }
    let mut obj = JsObject::new();
    let k_exit = strings.intern("exitCode");
    let k_stdout = strings.intern("stdout");
    let k_stderr = strings.intern("stderr");

    obj.set(k_exit, JsValue::number(last_exit_code as f64));

    let stdout_id = strings.intern(&stdout_str);
    obj.set(k_stdout, JsValue::string(stdout_id));

    let stderr_id = strings.intern(&stderr_str);
    obj.set(k_stderr, JsValue::string(stderr_id));

    let idx = heap.alloc(obj);
    Ok(JsValue::object(idx))
}

/// glob(pattern) -> array of matching file paths
///
/// Performs basic glob expansion on the given pattern string.
/// Supports `*` (match any sequence of chars) and `?` (match single char).
/// If the pattern has no wildcards, returns the pattern as-is in an array.
/// For patterns with a directory prefix (e.g., `/bin/*.rs`), splits into
/// directory + filename pattern.
fn native_glob(
    args: &[JsValue],
    strings: &mut StringPool,
    heap: &mut ObjectHeap,
) -> JsResult<JsValue> {
    if args.is_empty() || !args[0].is_string() {
        return Err(JsError::type_error("glob: expected string pattern"));
    }

    let pattern = String::from(strings.get(args[0].as_string_id()));

    // If no wildcards, return the pattern as a single-element array
    if !pattern.contains('*') && !pattern.contains('?') {
        let mut arr = JsObject::new_array();
        let id = strings.intern(&pattern);
        arr.push(JsValue::string(id));
        let idx = heap.alloc(arr);
        return Ok(JsValue::object(idx));
    }

    // Split pattern into directory and filename pattern
    let (dir, file_pattern) = if let Some(pos) = pattern.rfind('/') {
        let d = &pattern[..pos];
        let f = &pattern[pos + 1..];
        (if d.is_empty() { "/" } else { d }, f.to_string())
    } else {
        (".", pattern.clone())
    };

    let mut arr = JsObject::new_array();

    // Read directory entries and filter by pattern
    match std::fs::read_dir(dir) {
        Ok(entries) => {
            let mut names: Vec<String> = Vec::new();
            for entry in entries {
                if let Ok(entry) = entry {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy().to_string();
                    if glob_match(&file_pattern, &name_str) {
                        // Build full path
                        let full = if dir == "." {
                            name_str
                        } else if dir == "/" {
                            format!("/{}", name_str)
                        } else {
                            format!("{}/{}", dir, name_str)
                        };
                        names.push(full);
                    }
                }
            }
            names.sort();
            for name in &names {
                let id = strings.intern(name);
                arr.push(JsValue::string(id));
            }
        }
        Err(_) => {
            // Directory not readable: return empty array
        }
    }

    let idx = heap.alloc(arr);
    Ok(JsValue::object(idx))
}

/// Simple glob pattern matching supporting `*` and `?`.
fn glob_match(pattern: &str, text: &str) -> bool {
    let pat: Vec<char> = pattern.chars().collect();
    let txt: Vec<char> = text.chars().collect();
    glob_match_inner(&pat, &txt)
}

fn glob_match_inner(pat: &[char], txt: &[char]) -> bool {
    let mut pi = 0;
    let mut ti = 0;
    let mut star_pi = usize::MAX;
    let mut star_ti = 0;

    while ti < txt.len() {
        if pi < pat.len() && (pat[pi] == '?' || pat[pi] == txt[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < pat.len() && pat[pi] == '*' {
            star_pi = pi;
            star_ti = ti;
            pi += 1;
        } else if star_pi != usize::MAX {
            pi = star_pi + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }

    while pi < pat.len() && pat[pi] == '*' {
        pi += 1;
    }

    pi == pat.len()
}

/// env() -> object with all env vars
/// env(name) -> get environment variable value
/// env(name, value) -> set environment variable, returns undefined
fn native_env(
    args: &[JsValue],
    strings: &mut StringPool,
    heap: &mut ObjectHeap,
) -> JsResult<JsValue> {
    match args.len() {
        0 => {
            // Return an object with all environment variables
            let mut obj = JsObject::new();
            for (key, value) in std::env::vars() {
                let k = strings.intern(&key);
                let v_id = strings.intern(&value);
                obj.set(k, JsValue::string(v_id));
            }
            let idx = heap.alloc(obj);
            Ok(JsValue::object(idx))
        }
        1 => {
            // Get environment variable
            if !args[0].is_string() {
                return Err(JsError::type_error("env: expected string name"));
            }
            let name = String::from(strings.get(args[0].as_string_id()));
            match std::env::var(&name) {
                Ok(val) => {
                    let id = strings.intern(&val);
                    Ok(JsValue::string(id))
                }
                Err(_) => Ok(JsValue::undefined()),
            }
        }
        _ => {
            // Set environment variable
            if !args[0].is_string() {
                return Err(JsError::type_error("env: expected string name"));
            }
            let name = String::from(strings.get(args[0].as_string_id()));
            if args[1].is_string() {
                let value = String::from(strings.get(args[1].as_string_id()));
                std::env::set_var(&name, &value);
            } else if args[1].is_undefined() || args[1].is_null() {
                std::env::remove_var(&name);
            } else {
                let value = format!("{}", args[1].to_number());
                std::env::set_var(&name, &value);
            }
            Ok(JsValue::undefined())
        }
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Read all data from a file descriptor into a String.
fn read_fd_to_string(fd: libbreenix::types::Fd) -> String {
    let mut buf = [0u8; 4096];
    let mut result = Vec::new();
    loop {
        match libbreenix::io::read(fd, &mut buf) {
            Ok(0) => break,
            Ok(n) => result.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&result).into_owned()
}

/// Resolve a command name to a full path by searching PATH directories.
fn resolve_command(cmd: &str) -> Option<String> {
    // If cmd contains '/', use it directly
    if cmd.contains('/') {
        return Some(cmd.to_string());
    }

    // Search PATH
    let path_dirs = std::env::var("PATH").unwrap_or_else(|_| String::from("/bin:/usr/bin"));
    for dir in path_dirs.split(':') {
        let full_path = format!("{}/{}", dir, cmd);
        // Check if file exists and is executable
        let mut path_bytes: Vec<u8> = full_path.as_bytes().to_vec();
        path_bytes.push(0);
        let path_str = std::str::from_utf8(&path_bytes[..path_bytes.len() - 1]).unwrap_or("");
        if libbreenix::fs::access(path_str, libbreenix::fs::X_OK).is_ok() {
            return Some(full_path);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Context setup
// ---------------------------------------------------------------------------

/// Create a new breenish-js Context with all shell builtins registered.
fn create_shell_context() -> Context {
    let mut ctx = Context::new();
    ctx.set_print_fn(print_fn);

    // Register native shell functions
    ctx.register_native("exec", native_exec);
    ctx.register_native("cd", native_cd);
    ctx.register_native("pwd", native_pwd);
    ctx.register_native("which", native_which);
    ctx.register_native("readFile", native_read_file);
    ctx.register_native("writeFile", native_write_file);
    ctx.register_native("exit", native_exit);
    ctx.register_native("pipe", native_pipe);
    ctx.register_native("glob", native_glob);
    ctx.register_native("env", native_env);

    // Register built-in objects
    ctx.register_promise_builtins();
    ctx.register_json_builtins();
    ctx.register_math_builtins();

    ctx
}

fn print_fn(s: &str) {
    let _ = io::stdout().write_all(s.as_bytes());
    let _ = io::stdout().flush();
}

// ---------------------------------------------------------------------------
// Line editor with history and cursor movement
// ---------------------------------------------------------------------------

/// Result of reading a single key press from the terminal.
enum Key {
    /// A printable ASCII character
    Char(u8),
    /// Enter / Return
    Enter,
    /// Backspace (0x7F or 0x08)
    Backspace,
    /// Escape sequence: arrow up
    Up,
    /// Escape sequence: arrow down
    Down,
    /// Escape sequence: arrow left
    Left,
    /// Escape sequence: arrow right
    Right,
    /// Home key (ESC [ H)
    Home,
    /// End key (ESC [ F)
    End,
    /// Delete key (ESC [ 3 ~)
    Delete,
    /// Ctrl+A - move to start of line
    CtrlA,
    /// Ctrl+C - cancel current line
    CtrlC,
    /// Ctrl+D - EOF on empty line
    CtrlD,
    /// Ctrl+E - move to end of line
    CtrlE,
    /// Ctrl+K - kill to end of line
    CtrlK,
    /// Ctrl+U - kill to start of line
    CtrlU,
    /// Ctrl+W - delete word before cursor
    CtrlW,
    /// End of file (read returned 0 bytes)
    Eof,
    /// Unknown or unhandled key
    Unknown,
}

/// Interactive line editor with history support, cursor movement, and editing.
///
/// Handles raw terminal I/O, ANSI escape sequences, and maintains command
/// history across invocations.
struct LineEditor {
    /// Current line buffer (ASCII bytes)
    buffer: Vec<u8>,
    /// Cursor position within the buffer (byte offset)
    cursor: usize,
    /// Command history (oldest first)
    history: Vec<String>,
    /// Current position in history during navigation.
    /// `history.len()` means we are editing a new line (not in history).
    history_pos: usize,
    /// Saved line content when the user navigates into history so we can
    /// restore it when they come back to the bottom.
    saved_line: String,
    /// Original terminal attributes, saved on entry to raw mode.
    orig_termios: Option<libbreenix::termios::Termios>,
}

impl LineEditor {
    fn new() -> Self {
        LineEditor {
            buffer: Vec::new(),
            cursor: 0,
            history: Vec::new(),
            history_pos: 0,
            saved_line: String::new(),
            orig_termios: None,
        }
    }

    /// Enter raw mode: disable canonical mode and echo so we get individual
    /// key presses and handle display ourselves.
    fn enable_raw_mode(&mut self) {
        let fd = libbreenix::types::Fd::STDIN;
        let mut termios = libbreenix::termios::Termios::default();
        if libbreenix::termios::tcgetattr(fd, &mut termios).is_ok() {
            self.orig_termios = Some(termios);
            let mut raw = termios;
            // Disable canonical mode (line buffering) and echo
            raw.c_lflag &= !(libbreenix::termios::lflag::ICANON
                | libbreenix::termios::lflag::ECHO
                | libbreenix::termios::lflag::ECHOE
                | libbreenix::termios::lflag::ECHOK
                | libbreenix::termios::lflag::ECHONL);
            // Disable signal generation for Ctrl+C/Ctrl+Z so we handle them
            raw.c_lflag &= !libbreenix::termios::lflag::ISIG;
            // Read one byte at a time, no timeout
            raw.c_cc[libbreenix::termios::cc::VMIN] = 1;
            raw.c_cc[libbreenix::termios::cc::VTIME] = 0;
            let _ = libbreenix::termios::tcsetattr(
                fd,
                libbreenix::termios::TCSAFLUSH,
                &raw,
            );
        }
    }

    /// Restore the original terminal attributes saved by `enable_raw_mode`.
    fn disable_raw_mode(&mut self) {
        if let Some(ref orig) = self.orig_termios {
            let fd = libbreenix::types::Fd::STDIN;
            let _ = libbreenix::termios::tcsetattr(
                fd,
                libbreenix::termios::TCSAFLUSH,
                orig,
            );
        }
    }

    /// Read a single byte from stdin. Returns `None` on EOF or error.
    fn read_byte() -> Option<u8> {
        let mut buf = [0u8; 1];
        match io::stdin().read(&mut buf) {
            Ok(1) => Some(buf[0]),
            _ => None,
        }
    }

    /// Read a single key press, decoding multi-byte escape sequences.
    fn read_key() -> Key {
        let byte = match Self::read_byte() {
            Some(b) => b,
            None => return Key::Eof,
        };

        match byte {
            // Ctrl+A
            0x01 => Key::CtrlA,
            // Ctrl+C
            0x03 => Key::CtrlC,
            // Ctrl+D
            0x04 => Key::CtrlD,
            // Ctrl+E
            0x05 => Key::CtrlE,
            // Ctrl+K
            0x0B => Key::CtrlK,
            // Ctrl+U
            0x15 => Key::CtrlU,
            // Ctrl+W
            0x17 => Key::CtrlW,
            // Enter (carriage return)
            b'\r' | b'\n' => Key::Enter,
            // Backspace (DEL or BS)
            0x7F | 0x08 => Key::Backspace,
            // Escape - start of escape sequence
            0x1B => Self::read_escape_sequence(),
            // Printable ASCII
            0x20..=0x7E => Key::Char(byte),
            _ => Key::Unknown,
        }
    }

    /// Parse an escape sequence after the initial ESC byte.
    fn read_escape_sequence() -> Key {
        let b1 = match Self::read_byte() {
            Some(b) => b,
            None => return Key::Unknown,
        };

        if b1 != b'[' {
            return Key::Unknown;
        }

        let b2 = match Self::read_byte() {
            Some(b) => b,
            None => return Key::Unknown,
        };

        match b2 {
            b'A' => Key::Up,
            b'B' => Key::Down,
            b'C' => Key::Right,
            b'D' => Key::Left,
            b'H' => Key::Home,
            b'F' => Key::End,
            // ESC [ 1 ~ (Home alternate) or ESC [ 3 ~ (Delete) or ESC [ 4 ~ (End alternate)
            b'1' | b'3' | b'4' => {
                let b3 = match Self::read_byte() {
                    Some(b) => b,
                    None => return Key::Unknown,
                };
                if b3 == b'~' {
                    match b2 {
                        b'1' => Key::Home,
                        b'3' => Key::Delete,
                        b'4' => Key::End,
                        _ => Key::Unknown,
                    }
                } else {
                    Key::Unknown
                }
            }
            _ => Key::Unknown,
        }
    }

    /// Write raw bytes to stdout (used for terminal control).
    fn write_out(data: &[u8]) {
        let _ = io::stdout().write_all(data);
    }

    /// Flush stdout.
    fn flush_out() {
        let _ = io::stdout().flush();
    }

    /// Redraw the current line: prompt + buffer, then position the cursor
    /// correctly.
    fn refresh_line(&self, prompt: &str) {
        // Move to start of line
        Self::write_out(b"\r");
        // Print prompt and buffer
        Self::write_out(prompt.as_bytes());
        Self::write_out(&self.buffer);
        // Clear from cursor to end of line (in case text was deleted)
        Self::write_out(b"\x1b[K");
        // Move cursor back to the correct position if not at end of buffer
        let chars_after_cursor = self.buffer.len() - self.cursor;
        if chars_after_cursor > 0 {
            let seq = format!("\x1b[{}D", chars_after_cursor);
            Self::write_out(seq.as_bytes());
        }
        Self::flush_out();
    }

    /// Insert a character at the current cursor position.
    fn insert_char(&mut self, ch: u8) {
        self.buffer.insert(self.cursor, ch);
        self.cursor += 1;
    }

    /// Delete the character before the cursor (backspace).
    fn delete_back(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.buffer.remove(self.cursor);
        }
    }

    /// Delete the character at the cursor (delete key).
    fn delete_at(&mut self) {
        if self.cursor < self.buffer.len() {
            self.buffer.remove(self.cursor);
        }
    }

    /// Move cursor one position to the left.
    fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    /// Move cursor one position to the right.
    fn move_right(&mut self) {
        if self.cursor < self.buffer.len() {
            self.cursor += 1;
        }
    }

    /// Move cursor to the start of the line.
    fn move_home(&mut self) {
        self.cursor = 0;
    }

    /// Move cursor to the end of the line.
    fn move_end(&mut self) {
        self.cursor = self.buffer.len();
    }

    /// Clear everything before the cursor (Ctrl+U).
    fn kill_line_before(&mut self) {
        if self.cursor > 0 {
            self.buffer.drain(..self.cursor);
            self.cursor = 0;
        }
    }

    /// Clear everything from the cursor to end of line (Ctrl+K).
    fn kill_line_after(&mut self) {
        self.buffer.truncate(self.cursor);
    }

    /// Delete the word before the cursor (Ctrl+W).
    /// Skips trailing whitespace, then deletes back to the previous whitespace
    /// boundary.
    fn delete_word_back(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let orig = self.cursor;
        // Skip whitespace before cursor
        while self.cursor > 0 && self.buffer[self.cursor - 1] == b' ' {
            self.cursor -= 1;
        }
        // Delete back to the next whitespace or start of line
        while self.cursor > 0 && self.buffer[self.cursor - 1] != b' ' {
            self.cursor -= 1;
        }
        self.buffer.drain(self.cursor..orig);
    }

    /// Replace the buffer with a history entry or the saved line.
    fn set_buffer_from_str(&mut self, s: &str) {
        self.buffer.clear();
        self.buffer.extend_from_slice(s.as_bytes());
        self.cursor = self.buffer.len();
    }

    /// Navigate up in history (older entries).
    fn history_up(&mut self) {
        if self.history.is_empty() {
            return;
        }
        if self.history_pos == self.history.len() {
            // Save the current line before entering history
            self.saved_line = String::from_utf8_lossy(&self.buffer).into_owned();
        }
        if self.history_pos > 0 {
            self.history_pos -= 1;
            let entry = self.history[self.history_pos].clone();
            self.set_buffer_from_str(&entry);
        }
    }

    /// Navigate down in history (newer entries).
    fn history_down(&mut self) {
        if self.history_pos >= self.history.len() {
            return;
        }
        self.history_pos += 1;
        if self.history_pos == self.history.len() {
            // Back to the line being edited
            let saved = self.saved_line.clone();
            self.set_buffer_from_str(&saved);
        } else {
            let entry = self.history[self.history_pos].clone();
            self.set_buffer_from_str(&entry);
        }
    }

    /// Add a completed line to the history, avoiding consecutive duplicates.
    fn add_to_history(&mut self, line: &str) {
        if line.is_empty() {
            return;
        }
        // Don't add if it's the same as the last entry
        if let Some(last) = self.history.last() {
            if last == line {
                return;
            }
        }
        self.history.push(line.to_string());
    }

    /// Read a complete line from the user with full editing support.
    ///
    /// Returns `Some(line)` when the user presses Enter, or `None` on
    /// EOF (Ctrl+D on an empty line).
    fn read_line(&mut self, prompt: &str) -> Option<String> {
        self.buffer.clear();
        self.cursor = 0;
        self.history_pos = self.history.len();
        self.saved_line.clear();

        self.enable_raw_mode();

        // Print the prompt
        Self::write_out(prompt.as_bytes());
        Self::flush_out();

        let result = loop {
            match Self::read_key() {
                Key::Enter => {
                    // Print newline and return the line
                    Self::write_out(b"\r\n");
                    Self::flush_out();
                    let line = String::from_utf8_lossy(&self.buffer).into_owned();
                    break Some(line);
                }
                Key::Eof => {
                    Self::write_out(b"\r\n");
                    Self::flush_out();
                    break None;
                }
                Key::CtrlD => {
                    if self.buffer.is_empty() {
                        Self::write_out(b"\r\n");
                        Self::flush_out();
                        break None;
                    }
                    // Non-empty line: Ctrl+D does nothing (or could delete-at)
                }
                Key::CtrlC => {
                    // Clear current line, print ^C and a new prompt
                    Self::write_out(b"^C\r\n");
                    Self::flush_out();
                    self.buffer.clear();
                    self.cursor = 0;
                    Self::write_out(prompt.as_bytes());
                    Self::flush_out();
                }
                Key::Backspace => {
                    self.delete_back();
                    self.refresh_line(prompt);
                }
                Key::Delete => {
                    self.delete_at();
                    self.refresh_line(prompt);
                }
                Key::Left => {
                    self.move_left();
                    self.refresh_line(prompt);
                }
                Key::Right => {
                    self.move_right();
                    self.refresh_line(prompt);
                }
                Key::Home | Key::CtrlA => {
                    self.move_home();
                    self.refresh_line(prompt);
                }
                Key::End | Key::CtrlE => {
                    self.move_end();
                    self.refresh_line(prompt);
                }
                Key::Up => {
                    self.history_up();
                    self.refresh_line(prompt);
                }
                Key::Down => {
                    self.history_down();
                    self.refresh_line(prompt);
                }
                Key::CtrlU => {
                    self.kill_line_before();
                    self.refresh_line(prompt);
                }
                Key::CtrlK => {
                    self.kill_line_after();
                    self.refresh_line(prompt);
                }
                Key::CtrlW => {
                    self.delete_word_back();
                    self.refresh_line(prompt);
                }
                Key::Char(ch) => {
                    self.insert_char(ch);
                    self.refresh_line(prompt);
                }
                Key::Unknown => {
                    // Ignore unrecognized keys
                }
            }
        };

        self.disable_raw_mode();
        result
    }
}

// ---------------------------------------------------------------------------
// Shell modes
// ---------------------------------------------------------------------------

fn run_repl() {
    let mut ctx = create_shell_context();

    // Load startup scripts
    load_rc_file(&mut ctx, "/etc/bshrc");

    let _ = io::stdout().write_all(b"breenish v0.5.0 -- ECMAScript shell for Breenix\n");
    let _ = io::stdout().flush();

    let mut editor = LineEditor::new();

    loop {
        // Show current directory in prompt
        let prompt = match get_short_cwd() {
            Some(cwd) => format!("bsh {}> ", cwd),
            None => String::from("bsh> "),
        };

        let line = match editor.read_line(&prompt) {
            Some(line) => line,
            None => return, // EOF / Ctrl+D
        };

        let line = line.trim();

        if line.is_empty() {
            continue;
        }

        editor.add_to_history(line);

        // Handle `source <path>` as a shell builtin
        if let Some(path) = parse_source_command(line) {
            source_file(&mut ctx, &path);
            continue;
        }

        // Handle bare command shorthand: if it looks like a command (starts with
        // a letter, no JS operators), wrap it in exec() automatically
        let code = if should_auto_exec(line) {
            auto_exec_wrap(line)
        } else {
            line.to_string()
        };

        match ctx.eval(&code) {
            Ok(_) => {}
            Err(e) => {
                let msg = format!("{}\n", e);
                let _ = io::stderr().write_all(msg.as_bytes());
            }
        }
    }
}

/// Check if a line looks like a shell command rather than JavaScript.
/// Simple heuristic: starts with a command name (no JS keywords or operators).
fn should_auto_exec(line: &str) -> bool {
    let line = line.trim();
    if line.is_empty() {
        return false;
    }

    // Explicit JS constructs - don't auto-exec
    let js_starts = [
        "let ", "const ", "var ", "function ", "if ", "if(", "while ", "while(",
        "for ", "for(", "switch ", "switch(", "try ", "try{", "return ",
        "throw ", "class ", "import ", "export ", "async ", "await ",
        "print(", "exec(", "cd(", "pwd(", "which(", "readFile(", "writeFile(",
        "exit(", "pipe(", "glob(", "env(", "source ", "source(",
        "{", "[", "(", "//", "/*",
    ];
    for prefix in &js_starts {
        if line.starts_with(prefix) {
            return false;
        }
    }

    // Lines with assignment or declaration syntax
    if line.contains("=>") || line.contains("= ") || line.starts_with("//") {
        return false;
    }

    // Lines that start with a valid identifier and look like commands
    let first_char = line.chars().next().unwrap();
    if first_char.is_ascii_alphabetic() || first_char == '/' || first_char == '.' {
        return true;
    }

    false
}

/// Wrap a bare command line in exec() and print the result.
fn auto_exec_wrap(line: &str) -> String {
    // Split the line into command and args by whitespace
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.is_empty() {
        return line.to_string();
    }

    // Build exec call: exec("cmd", "arg1", "arg2")
    let mut call = String::from("let __r = exec(");
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            call.push_str(", ");
        }
        call.push('"');
        // Escape quotes in args
        for c in part.chars() {
            if c == '"' {
                call.push_str("\\\"");
            } else {
                call.push(c);
            }
        }
        call.push('"');
    }
    call.push_str("); if (__r.stdout.length > 0) print(__r.stdout);");
    call
}

/// Load and evaluate an RC (startup) file, silently ignoring missing files.
/// Errors during evaluation are printed to stderr but do not abort the shell.
fn load_rc_file(ctx: &mut Context, path: &str) {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            if let Err(e) = ctx.eval(&contents) {
                let msg = format!("bsh: error in {}: {}\n", path, e);
                let _ = io::stderr().write_all(msg.as_bytes());
            }
        }
        Err(_) => {
            // File doesn't exist or can't be read -- silently ignore
        }
    }
}

/// Parse a `source <path>` command line. Returns the path if the line
/// is a source command, or None otherwise.
///
/// Accepted forms:
///   source path/to/file
///   source "path/to/file"
///   source 'path/to/file'
///   source("path/to/file")
fn parse_source_command(line: &str) -> Option<String> {
    let trimmed = line.trim();

    if let Some(rest) = trimmed.strip_prefix("source(") {
        // source("path") or source('path')
        let rest = rest.trim_end_matches(')').trim();
        let path = rest.trim_matches('"').trim_matches('\'');
        if !path.is_empty() {
            return Some(path.to_string());
        }
    } else if let Some(rest) = trimmed.strip_prefix("source ") {
        // source path or source "path" or source 'path'
        let path = rest.trim().trim_matches('"').trim_matches('\'');
        if !path.is_empty() {
            return Some(path.to_string());
        }
    }

    None
}

/// Read a file and evaluate its contents in the given context.
fn source_file(ctx: &mut Context, path: &str) {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            if let Err(e) = ctx.eval(&contents) {
                let msg = format!("{}\n", e);
                let _ = io::stderr().write_all(msg.as_bytes());
            }
        }
        Err(e) => {
            let msg = format!("source: {}: {}\n", path, e);
            let _ = io::stderr().write_all(msg.as_bytes());
        }
    }
}

/// Get a shortened version of the current working directory.
fn get_short_cwd() -> Option<String> {
    let mut buf = [0u8; 1024];
    match libbreenix::process::getcwd(&mut buf) {
        Ok(len) => {
            let cwd = std::str::from_utf8(&buf[..len]).ok()?;
            if cwd == "/" {
                Some(String::from("/"))
            } else {
                // Show only the last component
                cwd.rsplit('/').next().map(String::from)
            }
        }
        Err(_) => None,
    }
}

fn run_string(code: &str) {
    let mut ctx = create_shell_context();

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

    let mut ctx = create_shell_context();

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
