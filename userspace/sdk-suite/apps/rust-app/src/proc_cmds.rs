use std::io::{Read, Write};
use std::process::{Command, Stdio};

use crate::CmdResult;

pub fn args(rest: &[String]) -> CmdResult {
    println!("argc={}", rest.len() + 2);
    for arg in rest {
        println!("[{arg}]");
    }
    Ok(())
}

pub fn env() -> CmdResult {
    let value = std::env::var("SDK_SUITE_VAR").map_err(|e| format!("SDK_SUITE_VAR: {e}"))?;
    println!("SDK_SUITE_VAR={value}");
    let unset = std::env::var_os("SDK_SUITE_UNSET");
    println!("SDK_SUITE_UNSET={}", if unset.is_some() { "set" } else { "absent" });
    let count = std::env::vars_os().count();
    println!("vars={count}");
    Ok(())
}

pub fn exit(rest: &[String]) -> CmdResult {
    let code: i32 = rest.first().and_then(|c| c.parse().ok()).ok_or("exit needs a status")?;
    std::process::exit(code)
}

/// Copy stdin to stdout.
pub fn cat() -> CmdResult {
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input).map_err(|e| format!("read stdin: {e}"))?;
    std::io::stdout().write_all(&input).map_err(|e| format!("write stdout: {e}"))
}

pub fn status(me: &str) -> CmdResult {
    let status = Command::new(me).args(["exit", "5"]).status().map_err(|e| format!("spawn {me}: {e}"))?;
    println!("status={:?}", status.code());
    Ok(())
}

pub fn output(me: &str) -> CmdResult {
    let mut child = Command::new(me)
        .arg("cat")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn {me}: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("no stdin pipe")?;
    stdin.write_all(b"piped through a child\n").map_err(|e| format!("write to the child: {e}"))?;
    drop(stdin);
    let output = child.wait_with_output().map_err(|e| format!("wait for the child: {e}"))?;
    print!("child said: {}", String::from_utf8_lossy(&output.stdout));
    println!("child status={:?}", output.status.code());
    Ok(())
}

pub fn cwd() -> CmdResult {
    let start = std::env::current_dir().map_err(|e| format!("current_dir: {e}"))?;
    println!("cwd={}", start.display());
    std::fs::create_dir("sub").map_err(|e| format!("create_dir sub: {e}"))?;
    std::env::set_current_dir("sub").map_err(|e| format!("set_current_dir sub: {e}"))?;
    let now = std::env::current_dir().map_err(|e| format!("current_dir: {e}"))?;
    println!("cwd after={}", now.display());
    std::env::set_current_dir(&start).map_err(|e| format!("set_current_dir back: {e}"))?;
    std::fs::remove_dir("sub").map_err(|e| format!("remove_dir sub: {e}"))
}
