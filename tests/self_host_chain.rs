//! Run with `rustc --edition=2021 --test tests/self_host_chain.rs`.
#[path = "../libs/libbreenix/src/crypto/sha256.rs"]
mod sha256;
#[path = "../userspace/programs/src/suite_support/self_host_chain.rs"]
mod self_host_chain;

use std::collections::BTreeMap;
use std::fs;
use std::time::{Duration, SystemTime};

const DEP_INFO: &str = "x86_64-breenix/selfhost/libbcc.d";

#[test]
fn noop_exempts_only_aarch64_tool_dep_info() {
    use self_host_chain::{verify_noop, Artifact};
    let original = Artifact {
        sha256: "same".into(),
        modified: SystemTime::UNIX_EPOCH,
    };
    let touched = Artifact {
        modified: original.modified + Duration::from_secs(1),
        ..original.clone()
    };
    let check = |path: &str| {
        verify_noop(
            &BTreeMap::from([(path.to_owned(), original.clone())]),
            &BTreeMap::from([(path.to_owned(), touched.clone())]),
        )
    };
    check("aarch64-breenix/selfhost/libbcc.d").unwrap();
    for path in [
        "aarch64-breenix/selfhost/libbcc.rlib",
        "x86_64-breenix/selfhost/libbcc.d",
    ] {
        assert!(check(path).unwrap_err().contains(path));
    }
}

#[test]
fn rejects_changed_file_set_or_contents() {
    let target = std::env::temp_dir().join(format!("breenix-self-host-chain-{}", std::process::id()));
    fs::create_dir(&target).unwrap();
    let file = target.join("selfhost/.fingerprint/bobj");
    let extra = target.join("selfhost/.fingerprint/extra");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, b"hello").unwrap();
    let recorded = BTreeMap::from([(
        "selfhost/.fingerprint/bobj".to_owned(),
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824".to_owned(),
    )]);
    let verify = || {
        self_host_chain::verify(&recorded, &self_host_chain::snapshot(&target).unwrap())
    };
    let matching = verify();
    fs::write(&file, b"jello").unwrap();
    let tampered = verify();
    fs::remove_file(&file).unwrap();
    let missing = verify();
    fs::write(&file, b"hello").unwrap();
    fs::write(&extra, b"hello").unwrap();
    let unexpected = verify();
    fs::remove_dir_all(&target).unwrap();

    matching.unwrap();
    assert!(tampered.unwrap_err().contains("bobj"));
    assert!(missing.unwrap_err().contains("bobj"));
    assert!(unexpected.unwrap_err().contains("extra"));
}

#[test]
fn noop_rejects_mtime_changes_outside_top_level_tool_dep_info() {
    use self_host_chain::{verify_noop, Artifact};

    let original = Artifact {
        sha256: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824".into(),
        modified: SystemTime::UNIX_EPOCH,
    };
    let touched = Artifact {
        modified: original.modified + Duration::from_secs(1),
        ..original.clone()
    };
    let before = BTreeMap::from([(DEP_INFO.to_owned(), original.clone())]);
    let after = BTreeMap::from([(DEP_INFO.to_owned(), touched.clone())]);
    verify_noop(&before, &after).unwrap();

    for path in [
        "x86_64-breenix/selfhost/libbcc.rlib",
        "x86_64-breenix/selfhost/bcc",
        "x86_64-breenix/selfhost/.fingerprint/bcc-build/bin-bcc",
        "x86_64-breenix/selfhost/deps/libbcc.d",
        "x86_64-breenix/selfhost/libbobj.d",
        "selfhost/libbcc.d",
    ] {
        let mut before = before.clone();
        let mut after = after.clone();
        before.insert(path.to_owned(), original.clone());
        after.insert(path.to_owned(), touched.clone());
        assert!(verify_noop(&before, &after).unwrap_err().contains(path));
    }
}

#[test]
fn noop_rejects_changed_contents_in_exempt_dep_info() {
    let target = std::env::temp_dir().join(format!("breenix-self-host-noop-{}", std::process::id()));
    let dep_info = target.join(DEP_INFO);
    fs::create_dir_all(dep_info.parent().unwrap()).unwrap();
    fs::write(&dep_info, b"bcc: input.rs\n").unwrap();
    let before = self_host_chain::snapshot(&target).unwrap();
    fs::write(&dep_info, b"bcc: other.rs\n").unwrap();
    let after = self_host_chain::snapshot(&target).unwrap();
    fs::remove_dir_all(&target).unwrap();

    assert!(self_host_chain::verify_noop(&before, &after)
        .unwrap_err()
        .contains(DEP_INFO));
}
