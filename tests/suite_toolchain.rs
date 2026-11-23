#[path = "../userspace/programs/src/suite_support/busybox_record.rs"]
mod busybox_record;
#[path = "../userspace/programs/src/suite_support/under_test.rs"]
mod under_test;

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

#[test]
fn busybox_chain_writes_under_test_record() {
    let root = std::env::temp_dir().join(format!("suite-toolchain-record-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let result = (|| {
        let record = root.join("busybox-under-test");
        let output = root.join("run");
        fs::create_dir(&output).unwrap();
        let binary = output.join("busybox.elf");
        assert!(busybox_record::publish(&record, &binary, &["ash", "echo", "sh"]).is_err());
        assert!(!record.exists());
        fs::write(&binary, b"fresh link").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        busybox_record::publish(&record, &binary, &["ash", "echo", "sh"])?;
        for suite in [under_test::Suite::Libc, under_test::Suite::Utilities] {
            let paths = under_test::Paths::load(&record, suite)?;
            assert!(paths.selected);
            let inode = fs::metadata(&binary).unwrap().ino();
            assert_eq!(fs::metadata(paths.busybox.as_ref()).unwrap().ino(), inode);
            assert_eq!(fs::metadata(paths.sh.as_ref()).unwrap().ino(), inode);
            assert_eq!(fs::metadata(output.join("bin/echo")).unwrap().ino(), inode);
            assert_eq!(paths.path.as_ref(), output.join("bin").to_str().unwrap());
        }
        Ok::<_, String>(())
    })();
    fs::remove_dir_all(root).unwrap();
    result.unwrap();
}
