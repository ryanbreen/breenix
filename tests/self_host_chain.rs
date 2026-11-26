//! Run with `rustc --edition=2021 --test tests/self_host_chain.rs`.
#[path = "../libs/libbreenix/src/crypto/sha256.rs"]
mod sha256;
#[path = "../userspace/programs/src/suite_support/self_host_chain.rs"]
mod self_host_chain;

use std::collections::BTreeMap;
use std::fs;

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
