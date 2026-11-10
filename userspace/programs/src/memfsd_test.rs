//! Small /dev/ufs server used by the network-shares suite.
#[path = "memfsd/tree.rs"]
mod tree;

use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use ufs_abi::{Op, Reply, Request, MAX_MESSAGE};

fn serve() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.len() > 2 || (args.len() == 2 && !matches!(args[1].as_str(), "--stall-reads" | "--hold-write-until-open")) {
        return Err("usage: memfsd_test DIR [--stall-reads|--hold-write-until-open]".into());
    }
    let stall = args.get(1).is_some_and(|s| s == "--stall-reads");
    let mut hold_write = args.get(1).is_some_and(|s| s == "--hold-write-until-open");
    let mut held_reply = None;
    let mut dev = OpenOptions::new().read(true).write(true).open("/dev/ufs")
        .map_err(|e| format!("open /dev/ufs: {e}"))?;
    let fd = dev.as_raw_fd() as u32;
    // SAFETY: the ufs mount data is a live u32 descriptor, copied during mount.
    unsafe {
        libbreenix::fs::mount("memfs", &args[0], "ufs", 0, (&fd as *const u32).cast())
    }.map_err(|e| format!("mount: {e}"))?;
    let mut stdout = io::stdout();
    writeln!(stdout, "READY").and_then(|_| stdout.flush()).map_err(|e| e.to_string())?;
    let mut tree = tree::Tree::new();
    let mut input = vec![0; MAX_MESSAGE];
    let mut output = vec![0; MAX_MESSAGE];
    loop {
        // /dev/ufs preserves messages: one read obtains one complete request.
        let n = match dev.read(&mut input) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(format!("read /dev/ufs: {e}")),
        };
        let req = Request::decode(&input[..n]).map_err(|_| "invalid ufs request")?;
        if stall && req.op == Op::Read {
            writeln!(stdout, "READ {}", req.id).and_then(|_| stdout.flush()).map_err(|e| e.to_string())?;
            // Consume later requests (including release) while deliberately withholding
            // read replies, so the timeout and daemon-death paths are real.
            continue;
        }
        let (errno, payload) = match tree.serve(&req) {
            Ok(payload) => (0, payload),
            Err(errno) => (-errno, Vec::new()),
        };
        let n = Reply { id: req.id, errno, payload: &payload }.encode(&mut output)
            .map_err(|_| "invalid ufs reply")?;
        if hold_write && req.op == Op::Write {
            hold_write = false;
            held_reply = Some(output[..n].to_vec());
            writeln!(stdout, "WRITE {}", req.id).and_then(|_| stdout.flush()).map_err(|e| e.to_string())?;
            continue;
        }
        let written = loop {
            match dev.write(&output[..n]) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                result => break result.map_err(|e| format!("write /dev/ufs: {e}"))?,
            }
        };
        if written != n { return Err("short /dev/ufs reply write".into()); }
        if req.op == Op::Open {
            if let Some(reply) = held_reply.take() {
                let n = dev.write(&reply).map_err(|e| e.to_string())?;
                if n != reply.len() { return Err("short held write reply".into()); }
            }
        }
    }
}
fn main() {
    if let Err(e) = serve() {
        eprintln!("memfsd_test: {e}");
        std::process::exit(1);
    }
}
