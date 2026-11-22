//! Disposable, peer-restricted SSH account and listener for test boots.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader};
use std::net::Ipv4Addr;
use std::os::unix::fs::{chown, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use breenix_suite::suite::{case_diagnostic, case_ms_left, CaseResult};
use libbreenix::accounts::{self, Account, UserAdd};

const SETTINGS: &str = "/etc/breenix/test-ssh/transfer";

fn field<'a>(settings: &'a str, prefix: &str) -> Result<&'a str, String> {
    settings
        .lines()
        .find_map(|line| line.strip_prefix(prefix))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("missing {prefix}in transfer fixture settings"))
}

fn nonce() -> Result<String, String> {
    let mut random = [0u8; 16];
    let mut filled = 0;
    while filled < random.len() {
        match libbreenix::crypto::rand::getrandom_bytes(&mut random[filled..]) {
            Ok(n) if n > 0 => filled += n,
            _ => return Err("cannot get a transfer nonce".into()),
        }
    }
    Ok(random.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn authorize(account: &Account, key: &str) -> std::io::Result<()> {
    let ssh = Path::new(&account.home).join(".ssh");
    fs::create_dir(&ssh)?;
    fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700))?;
    let keys = ssh.join("authorized_keys");
    use std::io::Write;
    writeln!(
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&keys)?,
        "{key}"
    )?;
    for path in [&ssh, &keys] {
        chown(path, Some(account.uid), Some(account.gid))?;
    }
    Ok(())
}

fn observe(
    child: &mut Child,
    log: File,
    nonce: &str,
    account: &Account,
    peer: Ipv4Addr,
    ready_record: &str,
    end_record: &str,
) -> Result<(), String> {
    let mut log = BufReader::new(log);
    let mut fingerprint = None;
    let mut end = None;
    loop {
        let exited = child.try_wait().map_err(|error| error.to_string())?;
        loop {
            let mut line = String::new();
            if log
                .read_line(&mut line)
                .map_err(|error| error.to_string())?
                == 0
            {
                break;
            }
            if !line.ends_with('\n') {
                log.seek_relative(-(line.len() as i64))
                    .map_err(|error| error.to_string())?;
                break;
            }
            if let Some(record) = line.strip_prefix("bsshd: host key ED25519 ") {
                fingerprint = record.split_whitespace().next().map(str::to_owned);
            } else if line.starts_with("bsshd: listening on ") {
                let fp = fingerprint
                    .as_deref()
                    .ok_or("bsshd did not report its host key")?;
                case_diagnostic(&format!(
                    "{ready_record} nonce={nonce} fp={fp} peer={peer} user={} uid={} gid={}",
                    account.name, account.uid, account.gid
                ));
            } else if let Some(record) = line.strip_prefix("bsshd: lease-end ") {
                end = Some(record.trim_end().to_owned());
            }
        }
        if let Some(status) = exited {
            let end = end.ok_or("bsshd exited without its lease-end record")?;
            case_diagnostic(&format!("{end_record} nonce={nonce} {end}"));
            if status.success() && end == "reason=done refused=0" {
                return Ok(());
            }
            return Err(format!("SSH lease failed: {end}"));
        }
        if case_ms_left() < 1_500 {
            return Err("bsshd did not end the SSH lease".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn lease(
    account: &Account,
    peer: Ipv4Addr,
    nonce: &str,
    ready_record: &str,
    end_record: &str,
) -> Result<(), String> {
    let done = Path::new(&account.home).join(format!(".transfer-done-{nonce}"));
    let log_path = format!("/tmp/transfer-{nonce}.log");
    let output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&log_path)
        .map_err(|error| error.to_string())?;
    let result = (|| {
        let log = File::open(&log_path).map_err(|error| error.to_string())?;
        let mut child = Command::new("/usr/sbin/bsshd")
            .args(["--allow-peer", &peer.to_string(), "--lease-done"])
            .arg(done)
            .stdin(Stdio::null())
            .stdout(output)
            .spawn()
            .map_err(|error| error.to_string())?;
        let result = observe(
            &mut child,
            log,
            nonce,
            account,
            peer,
            ready_record,
            end_record,
        );
        if result.is_err() && child.try_wait().ok().flatten().is_none() {
            let _ = child.kill();
            let _ = child.wait();
        }
        result
    })();
    let cleanup = fs::remove_file(&log_path).map_err(|error| error.to_string());
    result.and(cleanup)
}

pub fn with_listener(
    ready_record: &str,
    end_record: &str,
    verify: impl FnOnce(&Account, &str) -> CaseResult,
) -> CaseResult {
    let root = Path::new("/");
    let settings = fs::read_to_string(SETTINGS).map_err(|error| format!("{SETTINGS}: {error}"))?;
    let user = field(&settings, "user ")?;
    let key = field(&settings, "key ")?;
    let peer = field(&settings, "peer ")?
        .parse()
        .map_err(|_| "invalid transfer peer".to_owned())?;
    let uid = field(&settings, "uid ")?
        .parse()
        .ok()
        .filter(|uid| (1000..=60000).contains(uid))
        .ok_or("invalid transfer uid")?;
    let nonce = nonce()?;
    let account = accounts::useradd(
        root,
        user,
        &UserAdd {
            uid: Some(uid),
            shell: Some("/bin/bsh".into()),
            create_home: true,
            ..UserAdd::default()
        },
    )?;
    let result = (|| {
        authorize(&account, key).map_err(|error| error.to_string())?;
        lease(&account, peer, &nonce, ready_record, end_record)?;
        verify(&account, &nonce)
    })();
    // Revoke the key before account removal, which can refuse a live process.
    let revoked = fs::remove_file(Path::new(&account.home).join(".ssh/authorized_keys"))
        .map_err(|error| format!("revoke transfer key: {error}"));
    let cleanup = accounts::userdel(root, user, true);
    result
        .and(revoked.map_err(Into::into))
        .and(cleanup.map_err(Into::into))
}
