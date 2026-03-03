//! Built-in command implementations

use crate::kernel::WasmKernel;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

pub fn cmd_ls(kernel: &mut WasmKernel, argv: &[&str], _stdin: &[u8]) -> (Vec<u8>, i32) {
    let mut show_all = false;
    let mut show_long = false;
    let mut paths: Vec<&str> = Vec::new();

    for arg in &argv[1..] {
        if *arg == "-a" || *arg == "--all" {
            show_all = true;
        } else if *arg == "-l" {
            show_long = true;
        } else if *arg == "-la" || *arg == "-al" {
            show_all = true;
            show_long = true;
        } else {
            paths.push(arg);
        }
    }

    if paths.is_empty() {
        paths.push(".");
    }

    let mut output = String::new();
    for path in &paths {
        let resolved = kernel.resolve_path(path);
        match kernel.fs.readdir(&resolved) {
            Ok(entries) => {
                let mut names: Vec<String> = Vec::new();
                for entry in &entries {
                    if !show_all && entry.name.starts_with('.') {
                        continue;
                    }
                    if show_long {
                        let type_char = match entry.file_type {
                            breenix_core::fs::vfs::FileType::Directory => 'd',
                            breenix_core::fs::vfs::FileType::SymLink => 'l',
                            breenix_core::fs::vfs::FileType::CharDevice => 'c',
                            breenix_core::fs::vfs::FileType::BlockDevice => 'b',
                            _ => '-',
                        };
                        // Get size from stat
                        let full_path = if resolved == "/" {
                            format!("/{}", entry.name)
                        } else {
                            format!("{}/{}", resolved, entry.name)
                        };
                        let size = kernel.fs.stat(&full_path)
                            .map(|s| s.size)
                            .unwrap_or(0);
                        names.push(format!("{}rw-r--r-- 1 root root {:>8} {}", type_char, size, entry.name));
                    } else {
                        let suffix = if entry.file_type == breenix_core::fs::vfs::FileType::Directory {
                            "/"
                        } else {
                            ""
                        };
                        names.push(format!("{}{}", entry.name, suffix));
                    }
                }
                names.sort();
                if show_long {
                    output.push_str(&format!("total {}\n", names.len()));
                    for name in &names {
                        output.push_str(name);
                        output.push('\n');
                    }
                } else {
                    output.push_str(&names.join("  "));
                    if !names.is_empty() {
                        output.push('\n');
                    }
                }
            }
            Err(_) => {
                // Maybe it's a file, not a directory
                if kernel.fs.exists(&resolved) {
                    output.push_str(path);
                    output.push('\n');
                } else {
                    return (format!("ls: cannot access '{}': No such file or directory\n", path).into_bytes(), 2);
                }
            }
        }
    }
    (output.into_bytes(), 0)
}

pub fn cmd_cat(kernel: &mut WasmKernel, argv: &[&str], stdin: &[u8]) -> (Vec<u8>, i32) {
    if argv.len() <= 1 {
        // Read from stdin
        return (stdin.to_vec(), 0);
    }

    let mut output = Vec::new();
    for path in &argv[1..] {
        let resolved = kernel.resolve_path(path);
        match kernel.fs.read_file(&resolved) {
            Ok(data) => output.extend_from_slice(&data),
            Err(_) => {
                return (format!("cat: {}: No such file or directory\n", path).into_bytes(), 1);
            }
        }
    }
    (output, 0)
}

pub fn cmd_echo(_kernel: &mut WasmKernel, argv: &[&str], _stdin: &[u8]) -> (Vec<u8>, i32) {
    let mut no_newline = false;
    let start = if argv.len() > 1 && argv[1] == "-n" {
        no_newline = true;
        2
    } else {
        1
    };

    let mut output = argv[start..].join(" ");
    if !no_newline {
        output.push('\n');
    }
    (output.into_bytes(), 0)
}

pub fn cmd_pwd(kernel: &mut WasmKernel, _argv: &[&str], _stdin: &[u8]) -> (Vec<u8>, i32) {
    let mut output = kernel.sys_getcwd();
    output.push('\n');
    (output.into_bytes(), 0)
}

pub fn cmd_cd(kernel: &mut WasmKernel, argv: &[&str], _stdin: &[u8]) -> (Vec<u8>, i32) {
    let path = if argv.len() > 1 {
        argv[1]
    } else {
        kernel.env.get("HOME").map(|s| s.as_str()).unwrap_or("/")
    };
    // Need to clone to avoid borrow conflict
    let path_owned = String::from(path);
    match kernel.sys_chdir(&path_owned) {
        Ok(()) => (Vec::new(), 0),
        Err(_) => (format!("cd: {}: No such file or directory\n", path_owned).into_bytes(), 1),
    }
}

pub fn cmd_mkdir(kernel: &mut WasmKernel, argv: &[&str], _stdin: &[u8]) -> (Vec<u8>, i32) {
    let mut parents = false;
    let mut paths: Vec<&str> = Vec::new();
    for arg in &argv[1..] {
        if *arg == "-p" {
            parents = true;
        } else {
            paths.push(arg);
        }
    }

    if paths.is_empty() {
        return (b"mkdir: missing operand\n".to_vec(), 1);
    }

    for path in paths {
        let resolved = kernel.resolve_path(path);
        let result = if parents {
            kernel.fs.mkdir_p(&resolved, 0o755).map(|_| ())
        } else {
            kernel.fs.mkdir(&resolved, 0o755).map(|_| ())
        };
        if result.is_err() {
            return (format!("mkdir: cannot create directory '{}'\n", path).into_bytes(), 1);
        }
    }
    (Vec::new(), 0)
}

pub fn cmd_rmdir(kernel: &mut WasmKernel, argv: &[&str], _stdin: &[u8]) -> (Vec<u8>, i32) {
    if argv.len() <= 1 {
        return (b"rmdir: missing operand\n".to_vec(), 1);
    }
    for path in &argv[1..] {
        let resolved = kernel.resolve_path(path);
        if kernel.fs.rmdir(&resolved).is_err() {
            return (format!("rmdir: failed to remove '{}'\n", path).into_bytes(), 1);
        }
    }
    (Vec::new(), 0)
}

pub fn cmd_touch(kernel: &mut WasmKernel, argv: &[&str], _stdin: &[u8]) -> (Vec<u8>, i32) {
    if argv.len() <= 1 {
        return (b"touch: missing operand\n".to_vec(), 1);
    }
    for path in &argv[1..] {
        let resolved = kernel.resolve_path(path);
        let _ = kernel.fs.create_or_get(&resolved, 0o644);
    }
    (Vec::new(), 0)
}

pub fn cmd_rm(kernel: &mut WasmKernel, argv: &[&str], _stdin: &[u8]) -> (Vec<u8>, i32) {
    let mut recursive = false;
    let mut paths: Vec<&str> = Vec::new();
    for arg in &argv[1..] {
        if *arg == "-r" || *arg == "-rf" || *arg == "-fr" {
            recursive = true;
        } else if *arg == "-f" {
            // force - ignore nonexistent
        } else {
            paths.push(arg);
        }
    }

    if paths.is_empty() {
        return (b"rm: missing operand\n".to_vec(), 1);
    }

    for path in paths {
        let resolved = kernel.resolve_path(path);
        if kernel.fs.is_dir(&resolved) {
            if recursive {
                // Simple recursive delete
                if rm_recursive(kernel, &resolved).is_err() {
                    return (format!("rm: cannot remove '{}'\n", path).into_bytes(), 1);
                }
            } else {
                return (format!("rm: cannot remove '{}': Is a directory\n", path).into_bytes(), 1);
            }
        } else if kernel.fs.unlink(&resolved).is_err() {
            return (format!("rm: cannot remove '{}': No such file or directory\n", path).into_bytes(), 1);
        }
    }
    (Vec::new(), 0)
}

fn rm_recursive(kernel: &mut WasmKernel, path: &str) -> Result<(), ()> {
    let entries = kernel.fs.readdir(path).map_err(|_| ())?;
    for entry in entries {
        if entry.name == "." || entry.name == ".." {
            continue;
        }
        let child = if path == "/" {
            format!("/{}", entry.name)
        } else {
            format!("{}/{}", path, entry.name)
        };
        if entry.file_type == breenix_core::fs::vfs::FileType::Directory {
            rm_recursive(kernel, &child)?;
        } else {
            kernel.fs.unlink(&child).map_err(|_| ())?;
        }
    }
    kernel.fs.rmdir(path).map_err(|_| ())
}

pub fn cmd_cp(kernel: &mut WasmKernel, argv: &[&str], _stdin: &[u8]) -> (Vec<u8>, i32) {
    if argv.len() < 3 {
        return (b"cp: missing operand\n".to_vec(), 1);
    }
    let src = kernel.resolve_path(argv[1]);
    let dst = kernel.resolve_path(argv[2]);

    match kernel.fs.read_file(&src) {
        Ok(data) => {
            let _ = kernel.fs.create_or_get(&dst, 0o644);
            if kernel.fs.write_file(&dst, &data).is_err() {
                return (format!("cp: cannot write to '{}'\n", argv[2]).into_bytes(), 1);
            }
            (Vec::new(), 0)
        }
        Err(_) => (format!("cp: cannot stat '{}': No such file or directory\n", argv[1]).into_bytes(), 1),
    }
}

pub fn cmd_mv(kernel: &mut WasmKernel, argv: &[&str], _stdin: &[u8]) -> (Vec<u8>, i32) {
    if argv.len() < 3 {
        return (b"mv: missing operand\n".to_vec(), 1);
    }
    let src = kernel.resolve_path(argv[1]);
    let dst = kernel.resolve_path(argv[2]);

    match kernel.fs.rename(&src, &dst) {
        Ok(()) => (Vec::new(), 0),
        Err(_) => (format!("mv: cannot move '{}' to '{}'\n", argv[1], argv[2]).into_bytes(), 1),
    }
}

pub fn cmd_head(kernel: &mut WasmKernel, argv: &[&str], stdin: &[u8]) -> (Vec<u8>, i32) {
    let mut n_lines: usize = 10;
    let mut file_path: Option<&str> = None;

    let mut i = 1;
    while i < argv.len() {
        if argv[i] == "-n" && i + 1 < argv.len() {
            n_lines = argv[i + 1].parse().unwrap_or(10);
            i += 2;
        } else {
            file_path = Some(argv[i]);
            i += 1;
        }
    }

    let data = if let Some(path) = file_path {
        let resolved = kernel.resolve_path(path);
        match kernel.fs.read_file(&resolved) {
            Ok(d) => d,
            Err(_) => return (format!("head: cannot open '{}'\n", path).into_bytes(), 1),
        }
    } else {
        stdin.to_vec()
    };

    let text = core::str::from_utf8(&data).unwrap_or("");
    let lines: Vec<&str> = text.lines().take(n_lines).collect();
    let mut output = lines.join("\n");
    if !output.is_empty() {
        output.push('\n');
    }
    (output.into_bytes(), 0)
}

pub fn cmd_tail(kernel: &mut WasmKernel, argv: &[&str], stdin: &[u8]) -> (Vec<u8>, i32) {
    let mut n_lines: usize = 10;
    let mut file_path: Option<&str> = None;

    let mut i = 1;
    while i < argv.len() {
        if argv[i] == "-n" && i + 1 < argv.len() {
            n_lines = argv[i + 1].parse().unwrap_or(10);
            i += 2;
        } else {
            file_path = Some(argv[i]);
            i += 1;
        }
    }

    let data = if let Some(path) = file_path {
        let resolved = kernel.resolve_path(path);
        match kernel.fs.read_file(&resolved) {
            Ok(d) => d,
            Err(_) => return (format!("tail: cannot open '{}'\n", path).into_bytes(), 1),
        }
    } else {
        stdin.to_vec()
    };

    let text = core::str::from_utf8(&data).unwrap_or("");
    let all_lines: Vec<&str> = text.lines().collect();
    let start = if all_lines.len() > n_lines { all_lines.len() - n_lines } else { 0 };
    let lines = &all_lines[start..];
    let mut output = lines.join("\n");
    if !output.is_empty() {
        output.push('\n');
    }
    (output.into_bytes(), 0)
}

pub fn cmd_wc(_kernel: &mut WasmKernel, _argv: &[&str], stdin: &[u8]) -> (Vec<u8>, i32) {
    let text = core::str::from_utf8(stdin).unwrap_or("");
    let lines = text.lines().count();
    let words = text.split_whitespace().count();
    let bytes = stdin.len();

    let output = format!("  {}  {}  {}\n", lines, words, bytes);
    (output.into_bytes(), 0)
}

pub fn cmd_uname(_kernel: &mut WasmKernel, argv: &[&str], _stdin: &[u8]) -> (Vec<u8>, i32) {
    let all = argv.contains(&"-a");
    if all {
        (b"Breenix breenix 0.1.0 #1 SMP wasm32 Breenix\n".to_vec(), 0)
    } else {
        (b"Breenix\n".to_vec(), 0)
    }
}

pub fn cmd_env(kernel: &mut WasmKernel, _argv: &[&str], _stdin: &[u8]) -> (Vec<u8>, i32) {
    let mut output = String::new();
    for (key, value) in &kernel.env {
        output.push_str(key);
        output.push('=');
        output.push_str(value);
        output.push('\n');
    }
    (output.into_bytes(), 0)
}

pub fn cmd_export(kernel: &mut WasmKernel, argv: &[&str], _stdin: &[u8]) -> (Vec<u8>, i32) {
    for arg in &argv[1..] {
        if let Some(eq_pos) = arg.find('=') {
            let key = String::from(&arg[..eq_pos]);
            let value = String::from(&arg[eq_pos + 1..]);
            kernel.env.insert(key, value);
        }
    }
    (Vec::new(), 0)
}

pub fn cmd_unset(kernel: &mut WasmKernel, argv: &[&str], _stdin: &[u8]) -> (Vec<u8>, i32) {
    for arg in &argv[1..] {
        kernel.env.remove(*arg);
    }
    (Vec::new(), 0)
}

pub fn cmd_help(_kernel: &mut WasmKernel, _argv: &[&str], _stdin: &[u8]) -> (Vec<u8>, i32) {
    let output = "\
Breenix Shell — built-in commands:

  ls [-la] [path]     List directory contents
  cat [file...]       Concatenate and print files
  echo [-n] [text]    Print text
  pwd                 Print working directory
  cd [dir]            Change directory
  mkdir [-p] dir      Create directory
  rmdir dir           Remove empty directory
  touch file          Create empty file
  rm [-rf] file       Remove file or directory
  cp src dst          Copy file
  mv src dst          Move/rename file
  head [-n N] [file]  Show first N lines (default 10)
  tail [-n N] [file]  Show last N lines (default 10)
  wc                  Count lines, words, bytes
  grep pattern        Search for pattern in input
  tee [file]          Copy stdin to stdout and file
  stat file           Show file information
  uname [-a]          System information
  env                 Show environment variables
  export KEY=VALUE    Set environment variable
  unset KEY           Remove environment variable
  whoami              Print current user
  hostname            Print hostname
  date                Print current date/time
  true                Exit with status 0
  false               Exit with status 1
  clear               Clear terminal
  help                Show this help

Supports pipes: cmd1 | cmd2
Supports redirects: cmd > file, cmd >> file, cmd < file
";
    (output.as_bytes().to_vec(), 0)
}

pub fn cmd_clear(_kernel: &mut WasmKernel, _argv: &[&str], _stdin: &[u8]) -> (Vec<u8>, i32) {
    (b"\x1b[2J\x1b[H".to_vec(), 0)
}

pub fn cmd_stat(kernel: &mut WasmKernel, argv: &[&str], _stdin: &[u8]) -> (Vec<u8>, i32) {
    if argv.len() <= 1 {
        return (b"stat: missing operand\n".to_vec(), 1);
    }
    let path = argv[1];
    let resolved = kernel.resolve_path(path);
    match kernel.fs.stat(&resolved) {
        Ok(stat) => {
            let type_str = match stat.file_type {
                breenix_core::fs::vfs::FileType::Regular => "regular file",
                breenix_core::fs::vfs::FileType::Directory => "directory",
                breenix_core::fs::vfs::FileType::SymLink => "symbolic link",
                breenix_core::fs::vfs::FileType::CharDevice => "character device",
                breenix_core::fs::vfs::FileType::BlockDevice => "block device",
                breenix_core::fs::vfs::FileType::Fifo => "fifo",
                breenix_core::fs::vfs::FileType::Socket => "socket",
            };
            let output = format!(
                "  File: {}\n  Size: {}\tType: {}\n Inode: {}\tLinks: {}\n",
                path, stat.size, type_str, stat.inode_num, stat.nlink
            );
            (output.into_bytes(), 0)
        }
        Err(_) => (format!("stat: cannot stat '{}': No such file or directory\n", path).into_bytes(), 1),
    }
}

pub fn cmd_whoami(_kernel: &mut WasmKernel, _argv: &[&str], _stdin: &[u8]) -> (Vec<u8>, i32) {
    (b"root\n".to_vec(), 0)
}

pub fn cmd_hostname(kernel: &mut WasmKernel, _argv: &[&str], _stdin: &[u8]) -> (Vec<u8>, i32) {
    match kernel.fs.read_file("/etc/hostname") {
        Ok(data) => (data, 0),
        Err(_) => (b"breenix\n".to_vec(), 0),
    }
}

pub fn cmd_date(_kernel: &mut WasmKernel, _argv: &[&str], _stdin: &[u8]) -> (Vec<u8>, i32) {
    // Use js_sys::Date for actual date
    let now = crate::hal::current_time_ms();
    let secs = (now / 1000.0) as u64;
    let output = format!("Unix timestamp: {} seconds since epoch\n", secs);
    (output.into_bytes(), 0)
}

pub fn cmd_true(_kernel: &mut WasmKernel, _argv: &[&str], _stdin: &[u8]) -> (Vec<u8>, i32) {
    (Vec::new(), 0)
}

pub fn cmd_false(_kernel: &mut WasmKernel, _argv: &[&str], _stdin: &[u8]) -> (Vec<u8>, i32) {
    (Vec::new(), 1)
}

pub fn cmd_tee(kernel: &mut WasmKernel, argv: &[&str], stdin: &[u8]) -> (Vec<u8>, i32) {
    // Write stdin to stdout and optionally to a file
    if argv.len() > 1 {
        let path = kernel.resolve_path(argv[1]);
        let _ = kernel.fs.create_or_get(&path, 0o644);
        let _ = kernel.fs.write_file(&path, stdin);
    }
    (stdin.to_vec(), 0)
}

pub fn cmd_grep(_kernel: &mut WasmKernel, argv: &[&str], stdin: &[u8]) -> (Vec<u8>, i32) {
    if argv.len() < 2 {
        return (b"grep: missing pattern\n".to_vec(), 2);
    }
    let pattern = argv[1];
    let text = core::str::from_utf8(stdin).unwrap_or("");
    let mut output = String::new();
    let mut found = false;
    for line in text.lines() {
        if line.contains(pattern) {
            output.push_str(line);
            output.push('\n');
            found = true;
        }
    }
    (output.into_bytes(), if found { 0 } else { 1 })
}
