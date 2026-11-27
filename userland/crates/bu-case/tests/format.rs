use bu_case::{first_difference, Fixture};
use std::fs;
use std::os::unix::fs::PermissionsExt;

#[test]
fn materializes_bytes_mode_time_and_symlink() {
    let fixture = Fixture::parse(
        r#"{"entries":[
        {"path":"dir/link","kind":"symlink","target":"file"},
        {"path":"dir","kind":"directory","mode":"0750"},
        {"path":"dir/file","kind":"file","hex":"00ff","repeat":2,"mode":"0640","mtime":12345}
    ]}"#,
    )
    .unwrap();
    let root = std::env::temp_dir().join(format!("bu-case-format-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    fixture.materialize(&root).unwrap();
    let path = root.join("dir/file");
    let metadata = fs::metadata(&path).unwrap();
    let contents = fs::read(root.join("dir/link")).unwrap();
    let link = fs::read_link(root.join("dir/link")).unwrap();
    fs::remove_dir_all(root).unwrap();

    assert_eq!(contents, [0, 255, 0, 255]);
    assert_eq!(metadata.permissions().mode() & 0o777, 0o640);
    assert_eq!(
        metadata
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        12345
    );
    assert_eq!(link, std::path::Path::new("file"));
}

#[test]
fn rejects_fixture_escape_before_any_files_are_created() {
    for entries in [
        r#"{"path":"../outside","kind":"file","text":"bad"}"#,
        r#"{"path":"link","kind":"symlink","target":"../outside"},
           {"path":"link/file","kind":"file","text":"bad"}"#,
    ] {
        assert!(Fixture::parse(&format!(r#"{{"entries":[{entries}]}}"#)).is_err());
    }
}

#[test]
fn reports_the_first_differing_stream_as_a_unified_diff() {
    assert_eq!(
        first_difference(b"wrong\n", b"right\n", b"old\n", b"new\n", "1", "0").as_deref(),
        Some("--- expected.out\n+++ actual.out\n@@ -1 +1 @@\n-wrong\n+right\n")
    );
}
