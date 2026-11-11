use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
fn run() -> Result<(), String> {
    let a: Vec<_> = std::env::args().skip(1).collect();
    if a.is_empty() {
        let mounts =
            std::fs::read("/proc/mounts").map_err(|e| format!("read /proc/mounts: {e}"))?;
        print!(
            "{}",
            smbfs::shares::render_proc_mounts(&String::from_utf8_lossy(&mounts))
        );
        return Ok(());
    }
    if a == ["-a"] {
        return mount_all();
    }
    if a.len() != 6 || a[0] != "-t" || a[1] != "smb" || a[4] != "-o" {
        return Err(
            "usage: mount -t smb //HOST/SHARE DIR -o credfile=PATH[,port=N][,vers=3.0.2]".into(),
        );
    }
    mount_smb(&a[2], &a[3], &a[5], false)
}

fn mount_all() -> Result<(), String> {
    let table = match std::fs::read_to_string("/etc/shares") {
        Ok(table) => table,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("read /etc/shares: {e}")),
    };
    let (shares, errors) = smbfs::shares::parse_shares(&table);
    let mut failed = !errors.is_empty();
    for error in errors {
        eprintln!("mount: /etc/shares line {}: {}", error.line, error.message);
    }
    let mounts = std::fs::read("/proc/mounts").map_err(|e| format!("read /proc/mounts: {e}"))?;
    let mut mounted_now = Vec::new();
    for share in smbfs::shares::plan_mount_all(&shares, &String::from_utf8_lossy(&mounts)) {
        if mounted_now.contains(&share.directory) { continue; }
        if mount_smb(&share.source, &share.directory, &share.options, true).is_err() {
            eprintln!("mount: failed to mount {}", share.directory);
            failed = true;
        } else {
            mounted_now.push(share.directory.clone());
        }
    }
    if failed { Err(String::new()) } else { Ok(()) }
}

fn mount_smb(source: &str, directory: &str, options: &str, quiet_errors: bool) -> Result<(), String> {
    smbfs::parse_mount_options(source, options).map_err(str::to_owned)?;
    let mut command = Command::new("/sbin/smbfsd");
    command.args([source, directory, options]).stdout(Stdio::piped());
    if quiet_errors { command.stderr(Stdio::null()); }
    let mut child = command.spawn().map_err(|e| format!("start smbfsd: {e}"))?;
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
        if !e.is_empty() { eprintln!("mount: {e}"); }
        std::process::exit(1);
    }
}
