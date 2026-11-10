use std::{
    fs::OpenOptions,
    io::{self, Read, Write},
    os::fd::AsRawFd,
    time::Instant,
};
use ufs_abi::{Reply, Request, MAX_MESSAGE};
fn run() -> Result<(), String> {
    let a: Vec<_> = std::env::args().skip(1).collect();
    if a.len() != 3 {
        return Err("usage: smbfsd //HOST/SHARE DIR OPTIONS".into());
    }
    let o = smbfs::parse_mount_options(&a[0], &a[2]).map_err(str::to_owned)?;
    let c = smbfs::read_credentials(&o.credfile).map_err(str::to_owned)?;
    let creds = libsmb::auth::ntlm::Credentials {
        user: &c.username,
        password: &c.password,
        domain: &c.domain,
        workstation: "BREENIX",
    };
    let cap = if o.smb302 {
        libsmb::DialectCap::Smb302
    } else {
        libsmb::DialectCap::Default
    };
    let connection = smbfs::Connection::connect(&o.host, o.port, &o.share, &creds, cap)
        .map_err(|_| "SMB connection or authentication failed")?;
    let dialect = connection.dialect();
    let encrypted = connection.encrypted();
    let mut fs = smbfs::Namespace::new(connection);
    let mut dev = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/ufs")
        .map_err(|e| format!("open /dev/ufs: {e}"))?;
    let fd = dev.as_raw_fd() as u32;
    unsafe { libbreenix::fs::mount("smb", &a[1], "ufs", 0, (&fd as *const u32).cast()) }
        .map_err(|e| format!("mount: {e}"))?;
    let mut stdout = io::stdout();
    writeln!(
        stdout,
        "READY dialect=0x{dialect:04x} encrypted={}",
        u8::from(encrypted)
    )
    .and_then(|_| stdout.flush())
    .map_err(|e| e.to_string())?;
    let start = Instant::now();
    let mut input = vec![0; MAX_MESSAGE];
    let mut output = vec![0; MAX_MESSAGE];
    loop {
        let n = match dev.read(&mut input) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(format!("read /dev/ufs: {e}")),
        };
        let req = Request::decode(&input[..n]).map_err(|_| "invalid ufs request")?;
        let now = start.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        let (errno, payload) = match fs.serve(&req, now) {
            Ok(p) => (0, p),
            Err(e) => (-e, vec![]),
        };
        let n = Reply {
            id: req.id,
            errno,
            payload: &payload,
        }
        .encode(&mut output)
        .map_err(|_| "invalid ufs reply")?;
        let written = loop {
            match dev.write(&output[..n]) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                x => break x.map_err(|e| format!("write /dev/ufs: {e}"))?,
            }
        };
        if written != n {
            return Err("short /dev/ufs reply".into());
        }
    }
}
fn main() {
    if let Err(e) = run() {
        eprintln!("smbfsd: {e}");
        std::process::exit(1);
    }
}
