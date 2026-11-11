use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
fn run() -> Result<(), String> {
    let a: Vec<_> = std::env::args().skip(1).collect();
    if a.len() != 6 || a[0] != "-t" || a[1] != "smb" || a[4] != "-o" {
        return Err(
            "usage: mount -t smb //HOST/SHARE DIR -o credfile=PATH[,port=N][,vers=3.0.2]".into(),
        );
    }
    smbfs::parse_mount_options(&a[2], &a[5]).map_err(str::to_owned)?;
    let mut child = Command::new("/sbin/smbfsd")
        .args([&a[2], &a[3], &a[5]])
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("start smbfsd: {e}"))?;
    let mut line = String::new();
    BufReader::new(child.stdout.take().ok_or("missing daemon output")?)
        .read_line(&mut line)
        .map_err(|e| format!("daemon output: {e}"))?;
    if !line.starts_with("READY dialect=") || !line.ends_with('\n') {
        let _ = child.wait();
        return Err("smbfsd failed before mounting".into());
    }
    print!("{line}");
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("mount: {e}");
        std::process::exit(1);
    }
}
