//! Host checks. Run with rustc --edition=2021 --test from the checkout root.

#[path = "../kernel/src/fs/breenfs/commit_faults.rs"]
mod faults;

use faults::{Injection, Window};
use std::process::Command;

#[test]
fn production_resolves_neither_feature() {
    for (package, target, profile_flags) in [
        ("breenix", "x86_64-unknown-none", &[][..]),
        ("kernel", "./aarch64-breenix-kernel.json", &[][..]),
        (
            "kernel",
            "./aarch64-breenix-kernel.json",
            &["--no-default-features"][..],
        ),
    ] {
        let output = Command::new("cargo")
            .args([
                "tree",
                "--locked",
                "-e",
                "features,no-dev",
                "-i",
                "kernel",
                "-p",
                package,
                "--target",
                target,
            ])
            .args(profile_flags)
            .output()
            .expect("run cargo tree");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let features = String::from_utf8(output.stdout).unwrap();
        for name in ["breenfs_commit_window", "breenfs_corrupt_inject"] {
            assert!(
                !features.contains(name),
                "{package} {target} enables {name}:\n{features}"
            );
        }
    }
}

#[test]
fn windows_match_the_metal_cut_protocol() {
    for (window, name, marker) in [
        (
            Window::NodeWrites,
            "node-writes",
            "[breenfs] commit-window node-writes",
        ),
        (
            Window::FirstFlush,
            "first-flush",
            "[breenfs] commit-window first-flush",
        ),
        (
            Window::BetweenRings,
            "between-rings",
            "[breenfs] commit-window between-rings",
        ),
    ] {
        assert_eq!(
            Window::from_command_line(&format!("other=1 breenfs_commit_window={name}")),
            Some(window)
        );
        assert_eq!(window.marker(), marker);
    }
    assert_eq!(
        Window::from_command_line("breenfs_commit_window=final-flush"),
        None
    );
    assert_eq!(
        Window::from_command_line(
            "breenfs_commit_window=node-writes breenfs_commit_window=first-flush"
        ),
        None
    );
}

#[test]
fn selected_bit_changes_only_once_in_the_named_filesystem_block() {
    let injection =
        Injection::from_command_line("other=1 breenfs_corrupt_inject=81:513:7").unwrap();
    let sector = 81 * 8 + 1;
    assert!(injection.take(sector - 1, 512, false).is_none());
    assert!(injection.take(sector + 8, 512, false).is_none());
    let flip = injection.take(sector, 512, false).unwrap();
    assert_eq!((flip.block, flip.offset, flip.bit), (81, 513, 7));
    let mut bytes = [0x3c; 512];
    flip.apply(&mut bytes);
    let mut want = [0x3c; 512];
    want[1] ^= 0x80;
    assert_eq!(bytes, want);
    assert!(injection.take(sector, 512, false).is_none());
}

#[test]
fn first_data_skips_nodes_and_invalid_requests_stay_disarmed() {
    let injection =
        Injection::from_command_line("breenfs_corrupt_inject=first-data:4095:0").unwrap();
    assert!(injection.take(20, 4096, false).is_none());
    let flip = injection.take(24, 4096, true).unwrap();
    assert_eq!((flip.block, flip.offset, flip.bit), (24, 4095, 0));
    assert!(injection.take(25, 4096, true).is_none());
    for cmdline in [
        "",
        "other_breenfs_corrupt_inject=21:0:0",
        "breenfs_corrupt_inject=19:0:0",
        "breenfs_corrupt_inject=21:4096:0",
        "breenfs_corrupt_inject=21:0:8",
        "breenfs_corrupt_inject=21:0:0:1",
        "breenfs_corrupt_inject=21:0:0 breenfs_corrupt_inject=22:1:1",
    ] {
        assert!(Injection::from_command_line(cmdline).is_none(), "{cmdline}");
    }
}


#[test]
fn publication_fault_arms_once_for_the_writers_dirty_inode() {
    let armed = faults::PublicationArm::new();
    assert_eq!(armed.take_if(|_| true), None);
    assert!(armed.arm(42));
    assert!(!armed.arm(51));
    assert_eq!(armed.take_if(|ino| ino == 51), None);
    assert_eq!(armed.take_if(|ino| ino == 42), Some(42));
    assert_eq!(armed.take_if(|_| true), None);
    assert!(!armed.arm(42));
}

#[test]
fn corruption_skips_other_data_in_the_same_publication() {
    let injection = Injection::from_command_line("breenfs_corrupt_inject=first-data:64:0").unwrap();
    assert!(injection.take_for(41, 40 * 8, 512).is_none());
    assert!(injection.take_for(41, 41 * 8 + 1, 512).is_none());
    assert_eq!(injection.take_for(41, 41 * 8, 512).unwrap().block, 41);
}

#[test]
fn selected_sector_is_reported_before_a_later_write_fails() {
    use std::cell::RefCell;

    const SECTOR: usize = 512;
    const START: u64 = 81 * 8;
    let injection = Injection::from_command_line("breenfs_corrupt_inject=81:513:7").unwrap();
    let bytes = [0x3c; 4096];
    let mut altered = [0x3c; SECTOR];
    altered[1] ^= 0x80;
    let events = RefCell::new(Vec::new());
    let result = injection.write_selected(
        81,
        START,
        SECTOR,
        &bytes,
        |block, data| {
            events.borrow_mut().push(("write", block));
            match block - START {
                0 => assert_eq!(data, &bytes[..SECTOR]),
                1 => assert_eq!(data, &altered),
                2 => {
                    assert_eq!(data, &bytes[2 * SECTOR..]);
                    return Err("later write failed");
                }
                _ => panic!("unexpected sector {block}"),
            }
            Ok(())
        },
        |flip| {
            assert_eq!((flip.block, flip.offset, flip.bit), (81, 513, 7));
            events.borrow_mut().push(("receipt", START + 1));
        },
    );
    assert_eq!(result, Err("later write failed"));
    assert_eq!(
        events.into_inner(),
        [("write", START), ("write", START + 1), ("receipt", START + 1), ("write", START + 2)]
    );
}
