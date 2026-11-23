#[path = "../userspace/programs/src/suite_support/busybox_record.rs"]
mod busybox_record;
#[path = "../userspace/programs/src/suite_support/under_test.rs"]
mod under_test;

use std::{fs, path::Path};
use std::os::unix::fs::{MetadataExt, PermissionsExt};

#[test]
fn busybox_chain_writes_under_test_record() {
    let root = std::env::temp_dir().join(format!("suite-toolchain-record-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let result = (|| {
        let record = root.join(std::path::Path::new(under_test::RECORD).file_name().unwrap());
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

#[test]
fn staged_consumers_require_record_and_prefer_new_applets() {
    let root = std::env::temp_dir().join(format!("suite-toolchain-consumers-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let result = (|| {
        let record = root.join("busybox-under-test");
        let fixture = root.join("busybox.sh");
        let output = root.join("run");
        fs::create_dir(&output).unwrap();
        let binary = output.join("busybox.elf");
        fs::write(&binary, b"fresh link").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        for suite in [under_test::Suite::Libc, under_test::Suite::Utilities] {
            assert!(!under_test::Paths::load_for_fixture(&record, &fixture, suite)?.selected);
        }
        fs::write(&fixture, b"staged driver").unwrap();
        for suite in [under_test::Suite::Libc, under_test::Suite::Utilities] {
            let error = under_test::Paths::load_for_fixture(&record, &fixture, suite).unwrap_err();
            assert!(error.contains(record.to_str().unwrap()), "{error}");
        }
        busybox_record::publish(&record, &binary, &["ash", "echo", "sh"])?;
        let libc = under_test::Paths::load_for_fixture(&record, &fixture, under_test::Suite::Libc)?;
        let utilities = under_test::Paths::load_for_fixture(&record, &fixture, under_test::Suite::Utilities)?;
        let bin = output.join("bin");
        assert_eq!(Path::new(libc.path.as_ref()), bin);
        assert_eq!(utilities.path.split(':').collect::<Vec<_>>(), [bin.to_str().unwrap(), "/bin", "/usr/bin"]);
        assert_eq!(fs::metadata(bin.join("echo")).unwrap().ino(), fs::metadata(binary).unwrap().ino());
        Ok::<_, String>(())
    })();
    fs::remove_dir_all(root).unwrap();
    result.unwrap();
}
