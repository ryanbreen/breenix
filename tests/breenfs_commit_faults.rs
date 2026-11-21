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
