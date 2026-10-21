//! The server's host key file, `/etc/ssh/ssh_host_ed25519_key`.
//!
//! The first start that finds no key generates one from the caller's random
//! source and writes it in the OpenSSH private-key format, readable by root
//! only, beside its `.pub` line. Later starts read it back. A key file that is
//! a symbolic link, is not root's, can be read by anyone else, or does not
//! parse is refused, not replaced: bsshd does not start with it.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use super::account::Refusal;
use super::keys::HostKey;

/// The host key directory, under the root.
pub const DIRECTORY: &str = "etc/ssh";
/// The private key's file name; the public key line is in `<name>.pub`.
pub const PRIVATE: &str = "ssh_host_ed25519_key";
const COMMENT: &str = "root@breenix";

extern "C" {
    fn geteuid() -> u32;
}

fn refuse<T>(text: impl Into<String>) -> Result<T, Refusal> {
    Err(Refusal(text.into()))
}

/// Owned by root (or this process, which is root in bsshd).
fn system_owned(meta: &fs::Metadata) -> bool {
    meta.uid() == 0 || meta.uid() == unsafe { geteuid() }
}

fn write_new(path: &Path, text: &str, mode: u32) -> Result<(), Refusal> {
    let mut staging = path.as_os_str().to_owned();
    staging.push(".new");
    let staging = std::path::PathBuf::from(staging);
    match fs::remove_file(&staging) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return refuse(format!("remove {}: {}", staging.display(), e)),
        _ => {}
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&staging)
        .or_else(|e| refuse(format!("create {}: {}", staging.display(), e)))?;
    file.write_all(text.as_bytes())
        .and_then(|_| file.sync_all())
        .or_else(|e| refuse(format!("write {}: {}", staging.display(), e)))?;
    fs::rename(&staging, path).or_else(|e| refuse(format!("rename {}: {}", path.display(), e)))
}

/// Read the host key under `root`, or generate and store one when there is
/// no key. Returns the key and whether it was generated now.
pub fn load_or_generate(
    root: &Path,
    random: impl FnOnce(&mut [u8]) -> Result<(), &'static str>,
) -> Result<(HostKey, bool), Refusal> {
    let directory = root.join(DIRECTORY);
    if let Err(e) = fs::symlink_metadata(&directory) {
        if e.kind() != std::io::ErrorKind::NotFound {
            return refuse(format!("{}: {}", directory.display(), e));
        }
        fs::create_dir(&directory).or_else(|e| refuse(format!("mkdir {}: {}", directory.display(), e)))?;
        fs::set_permissions(&directory, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .or_else(|e| refuse(format!("chmod {}: {}", directory.display(), e)))?;
    }
    let meta = fs::symlink_metadata(&directory).or_else(|e| refuse(format!("{}: {}", directory.display(), e)))?;
    if !meta.file_type().is_dir() || !system_owned(&meta) || meta.mode() & 0o022 != 0 {
        return refuse(format!("{} must be a directory owned by root and writable by root only", directory.display()));
    }

    let private = directory.join(PRIVATE);
    let public = directory.join(format!("{}.pub", PRIVATE));
    match fs::symlink_metadata(&private) {
        Ok(meta) => {
            if !meta.file_type().is_file() || !system_owned(&meta) || meta.mode() & 0o077 != 0 {
                return refuse(format!("{} must be a regular file readable by root only", private.display()));
            }
            let text = fs::read_to_string(&private).or_else(|e| refuse(format!("{}: {}", private.display(), e)))?;
            let key = HostKey::from_openssh(&text).or_else(|e| refuse(format!("{}: {}", private.display(), e)))?;
            if fs::symlink_metadata(&public).is_err() {
                write_new(&public, &format!("{}\n", key.public_line(COMMENT)), 0o644)?;
            }
            Ok((key, false))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let key = HostKey::generate(random).or_else(|e| refuse(format!("host key generation: {}", e)))?;
            let check = u32::from_le_bytes(key.fingerprint()[..4].try_into().unwrap_or([0; 4]));
            write_new(&private, &key.to_openssh(COMMENT, check), 0o600)?;
            write_new(&public, &format!("{}\n", key.public_line(COMMENT)), 0o644)?;
            File::open(&directory)
                .and_then(|handle| handle.sync_all())
                .or_else(|e| refuse(format!("sync {}: {}", directory.display(), e)))?;
            Ok((key, true))
        }
        Err(e) => refuse(format!("{}: {}", private.display(), e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("bsshd-hostkey-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("etc")).unwrap();
        root
    }

    fn seed(byte: u8) -> impl FnOnce(&mut [u8]) -> Result<(), &'static str> {
        move |buffer: &mut [u8]| {
            buffer.fill(byte);
            Ok(())
        }
    }

    #[test]
    fn first_start_generates_and_later_starts_read_the_same_key() {
        let root = scratch("generate");
        let (first, generated) = load_or_generate(&root, seed(3)).unwrap();
        assert!(generated);
        let private = root.join(DIRECTORY).join(PRIVATE);
        assert_eq!(fs::metadata(&private).unwrap().mode() & 0o777, 0o600);
        assert_eq!(fs::metadata(root.join(DIRECTORY)).unwrap().mode() & 0o777, 0o755);
        let (second, generated) = load_or_generate(&root, |_: &mut [u8]| Err("must not be asked")).unwrap();
        assert!(!generated);
        assert_eq!(first.public_key_blob(), second.public_key_blob());
        let line = fs::read_to_string(root.join(DIRECTORY).join(format!("{}.pub", PRIVATE))).unwrap();
        assert_eq!(super::super::keys::parse_public_line(line.trim()), Some(first.public_key_blob()));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn readable_linked_or_corrupt_keys_are_refused_not_replaced() {
        let root = scratch("refuse");
        load_or_generate(&root, seed(4)).unwrap();
        let private = root.join(DIRECTORY).join(PRIVATE);
        let original = fs::read(&private).unwrap();
        fs::set_permissions(&private, fs::Permissions::from_mode(0o640)).unwrap();
        assert!(load_or_generate(&root, seed(5)).is_err());
        fs::set_permissions(&private, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&private, b"not a key").unwrap();
        assert!(load_or_generate(&root, seed(5)).is_err());
        assert_eq!(fs::read(&private).unwrap(), b"not a key");
        fs::remove_file(&private).unwrap();
        fs::write(root.join("elsewhere"), &original).unwrap();
        std::os::unix::fs::symlink(root.join("elsewhere"), &private).unwrap();
        assert!(load_or_generate(&root, seed(5)).is_err());
        fs::set_permissions(root.join(DIRECTORY), fs::Permissions::from_mode(0o777)).unwrap();
        fs::remove_file(&private).unwrap();
        assert!(load_or_generate(&root, seed(5)).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
