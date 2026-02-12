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
// Shell modes
// ---------------------------------------------------------------------------

fn run_repl() {
    let mut ctx = create_shell_context();

    // Load startup scripts
    load_rc_file(&mut ctx, "/etc/bshrc");

    let _ = io::stdout().write_all(b"breenish v0.5.0 -- ECMAScript shell for Breenix\n");
    let _ = io::stdout().flush();

    let mut line_buf = Vec::new();

    loop {
        // Show current directory in prompt
        let prompt = match get_short_cwd() {
            Some(cwd) => format!("bsh {}> ", cwd),
            None => String::from("bsh> "),
        };
        let _ = io::stdout().write_all(prompt.as_bytes());
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
