//! Host-only tests: rustc --edition=2021 --test (no kernel build or QEMU).
#[path = "../src/bin/qemu-uefi/profile.rs"]
mod profile;

use profile::Profile;
use std::{collections::BTreeSet, path::PathBuf, process::Command};

fn root() -> PathBuf {
    PathBuf::from(option_env!("CARGO_MANIFEST_DIR").unwrap_or("."))
}

#[test]
fn catalog_and_launcher_implement_exactly_the_same_profiles() {
    // Parse real JSON and enforce Vigil's four-string-field contract without
    // adding a kernel-workspace dependency for this host-only test.
    let output = Command::new("python3")
        .args([
            "-c",
            r#"
import json, sys
rows = json.load(open(sys.argv[1]))
assert isinstance(rows, list) and rows[0]['name'] == 'default'
for row in rows:
    assert set(row) == {'name', 'title', 'summary', 'devices'}
    assert all(isinstance(value, str) and value for value in row.values())
    print(row['name'])
"#,
        ])
        .arg(root().join("docs/x86-profiles.json"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let names = String::from_utf8(output.stdout).unwrap();
    let documented: BTreeSet<_> = names.lines().collect();
    assert_eq!(
        documented.len(),
        names.lines().count(),
        "duplicate catalog names"
    );
    let implemented: BTreeSet<_> = Profile::ALL.into_iter().map(Profile::name).collect();
    assert_eq!(documented, implemented);
    for name in documented {
        let profile = Profile::parse(name).unwrap();
        assert_eq!(profile.name(), name);
        // Exercise each implementation's command construction as well.
        let mut command = Command::new("qemu-system-x86_64");
        profile.add_controllers(&mut command);
        assert!(!profile.machine().is_empty());
        assert!(!profile.cpus().is_empty());
        assert!(!profile.nic_device().is_empty());
        for (index, id) in ["hd", "testdisk", "ext2disk"].into_iter().enumerate() {
            assert!(profile
                .disk_device(id, index, index == 0)
                .contains(&format!("drive={id}")));
        }
        assert_eq!(profile.census("virtio").1, 1);
    }
    let error = Profile::parse("unknown").unwrap_err();
    for profile in Profile::ALL {
        assert!(error.contains(profile.name()));
    }
}

#[test]
fn default_hardware_components_match_expected_values() {
    let default = Profile::Default;
    let mut command = Command::new("qemu-system-x86_64");
    default.add_controllers(&mut command);
    assert_eq!(command.get_args().count(), 0);
    assert_eq!(default.machine(), "pc");
    assert_eq!(default.cpus(), "4");
    assert_eq!(default.nic_device(), "e1000");
    assert_eq!(
        default.disk_device("hd", 0, true),
        "virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off"
    );
    assert_eq!(
        default.disk_device("testdisk", 1, false),
        "virtio-blk-pci,drive=testdisk,disable-modern=on,disable-legacy=off"
    );
    assert_eq!(
        default.disk_device("ext2disk", 2, false),
        "virtio-blk-pci,drive=ext2disk,disable-modern=on,disable-legacy=off"
    );
    assert_eq!(default.census("virtio"), (3, 1));
    assert_eq!(default.census("ide"), (0, 1));
}

#[test]
fn storage_census_matches_the_devices_constructed_for_each_profile() {
    for profile in Profile::ALL {
        let blocks = (0..3)
            .filter(|index| {
                profile
                    .disk_device("disk", *index, *index == 0)
                    .starts_with("virtio-blk-pci,")
            })
            .count();
        assert_eq!(profile.census("virtio").0, blocks, "{}", profile.name());
    }
    assert!(Profile::VirtioModern
        .disk_device("hd", 0, true)
        .ends_with("disable-legacy=on"));
    assert!(Profile::Ahci
        .disk_device("hd", 0, true)
        .contains("bus=profile-ahci.0"));
    assert!(Profile::Nvme
        .disk_device("hd", 0, true)
        .starts_with("nvme,"));
}

#[test]
fn preflight_output_and_none_network_use_production_construction() {
    for profile in Profile::ALL {
        let (blocks, network) = profile.census("virtio");
        assert_eq!(profile.census_line("virtio"), format!("{blocks} {network}"));
        let pci = profile.pci_census_lines();
        assert!(!pci.is_empty());
        for line in pci.lines() {
            let mut fields = line.split(' ');
            let id = fields.next().unwrap();
            let count: usize = fields.next().unwrap().parse().unwrap();
            assert_eq!(id.len(), 9);
            assert!(count > 0);
            assert!(fields.next().is_none());
        }
        let mut command = Command::new("qemu-system-x86_64");
        profile.add_none_network(&mut command);
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_str().unwrap())
            .collect();
        if profile == Profile::Default {
            assert!(args.is_empty());
        } else {
            assert_eq!(
                args,
                vec![
                    "-netdev",
                    "user,id=net0",
                    "-device",
                    &format!("{},netdev=net0,mac=52:54:00:12:34:56", profile.nic_device())
                ]
            );
        }
    }
    assert!(Profile::Ahci.pci_census().contains(&("8086:2922", 1)));
    assert!(Profile::Nvme.pci_census().contains(&("1b36:0010", 3)));
    assert_eq!(Profile::Default.census_line("virtio"), "3 1");
    assert_eq!(Profile::Ahci.census_line("virtio"), "0 1");
}
