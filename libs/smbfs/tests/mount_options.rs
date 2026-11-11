use smbfs::{parse_credentials, parse_mount_options};

#[test]
fn mount_options_validate_unc_and_options() {
    let o =
        parse_mount_options("//server/share", "credfile=/etc/smb,port=10445,vers=3.0.2").unwrap();
    assert_eq!(
        (&*o.host, &*o.share, &*o.credfile, o.port, o.smb302),
        ("server", "share", "/etc/smb", 10445, true)
    );
    for unc in [
        "server/share",
        "//server",
        "//server/",
        "///share",
        "//server/share/extra",
    ] {
        assert!(parse_mount_options(unc, "credfile=/x").is_err(), "{unc}");
    }
    for options in [
        "credfile=/x,other=1",
        "credfile=/x,port=0",
        "credfile=/x,port=65536",
        "credfile=/x,port=no",
        "credfile=/x,vers=2.0",
        "port=445",
    ] {
        assert!(
            parse_mount_options("//server/share", options).is_err(),
            "{options}"
        );
    }
}

#[test]
fn credentials_require_root_and_private_mode_without_leaking_contents() {
    let text = "username=alice\npassword=secret-value\ndomain=example\n";
    let c = parse_credentials(text, 0, 0o600).unwrap();
    assert_eq!(
        (&*c.username, &*c.password, &*c.domain),
        ("alice", "secret-value", "example")
    );
    for (uid, mode) in [(1, 0o600), (0, 0o644), (0, 0o400)] {
        let e = parse_credentials(text, uid, mode).err().unwrap();
        assert!(!e.contains("secret-value"));
    }
    let e = parse_credentials(
        "username=alice\npassword=secret-value\nbad=secret-value\n",
        0,
        0o600,
    )
    .err()
    .unwrap();
    assert!(!e.contains("secret-value"));
}

#[test]
fn credential_file_read_rejects_insecure_file_without_disclosing_contents() {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::temp_dir().join(format!("smbfs-credentials-{}", std::process::id()));
    std::fs::write(&path, "username=alice\npassword=secret-value\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let error = smbfs::read_credentials(path.to_str().unwrap())
        .err()
        .unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(!error.contains("secret-value"));
}
