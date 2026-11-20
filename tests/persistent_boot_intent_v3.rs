//! Format 3 mount descriptions for migration, cutover and rescue.
#[allow(dead_code)] // This host test imports the complete boot module but exercises its format-3 API.
#[path = "../kernel/src/boot/intent.rs"]
mod intent;

const A: &str = "11111111-1111-1111-1111-111111111111";
const B: &str = "22222222-2222-2222-2222-222222222222";

fn payload(lines: &[(&str, &str, bool)]) -> String {
    let mut s = String::from("BREENIX-BOOT 3\ntarget persistent\n");
    for (guid, path, writable) in lines {
        s.push_str(&format!("mount {guid} {path} {}\n", if *writable { "rw" } else { "ro" }));
    }
    s
}

#[test]
fn accepts_migration_cutover_and_rescue() {
    for mounts in [
        vec![(A, "/", false), (A, "/old", false), (B, "/new", true), (B, "/shared", true), (A, "/home", false)],
        vec![(A, "/", true), (B, "/home", true)],
        vec![(A, "/", false), (B, "/tmp", true), (B, "/run", true)],
    ] {
        let text = payload(&mounts);
        let parsed = intent::parse(text.as_bytes()).expect("recognized");
        assert!(intent::root_userland(Some(&parsed)).is_ok());
        assert_eq!(parsed.mounts().collect::<Vec<_>>(), mounts);
    }
}

#[test]
fn refuses_invalid_mount_sets() {
    let good = payload(&[(A, "/", true), (B, "/home", true)]);
    for text in [
        payload(&[(A, "/", true), (B, "/home", true), (A, "/tmp", true)]),
        payload(&[(A, "/home", false)]),
        payload(&[(A, "/", false), (B, "/a/../b", false)]),
        payload(&[(A, "/", false), (B, "/", false)]),
        format!("{good}root {A}\n"),
        format!("{good}home {B}\n"),
        payload(&[]),
        payload(&[(A, "/", false), (B, &format!("/{}", "x".repeat(64)), false)]),
        format!("{good}{}", "\n".repeat(1024)),
    ] {
        let parsed = intent::parse(text.as_bytes()).expect("recognized refusal");
        assert!(intent::root_userland(Some(&parsed)).is_err(), "accepted {text:?}");
    }
}
