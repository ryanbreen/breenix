//! A ledger snapshot may omit pending serials even when later outcomes exist.
extern crate alloc;
extern crate self as libbui;
#[path = "../libs/libuitest/src/input.rs"]
pub mod input;
#[path = "../libs/libbui/src/tree.rs"]
pub mod tree;

#[test]
fn finds_first_missing_script_disposition() {
    let ledger = "41 delivered 9 none\n43 wm-handled 0 none\n44 rejected 0 not-selected\n";
    assert_eq!(
        input::first_missing_disposition(ledger, &[41, 42, 43, 44]).unwrap(),
        Some(42)
    );
    assert_eq!(
        input::first_missing_disposition(ledger, &[41, 43, 44]).unwrap(),
        None
    );
    assert_eq!(
        input::first_missing_disposition(ledger, &[45, 42]).unwrap(),
        Some(45)
    );
    assert!(input::first_missing_disposition("41 pending 9 none\n", &[41]).is_err());
    assert!(input::first_missing_disposition("broken row\n", &[41]).is_err());
}

#[test]
fn delivery_requires_a_committed_source_watermark() {
    assert!(!input::delivery_ready(0, 41, 41));
    assert!(!input::delivery_ready(9, 40, 41));
    assert!(input::delivery_ready(9, 41, 41));
    assert!(input::delivery_ready(9, 44, 41));
    let old_derived = libbreenix::input::abi::derived_serial(40, 1).unwrap();
    assert!(!input::delivery_ready(9, old_derived, 41));
    assert!(input::delivery_ready(
        9,
        libbreenix::input::abi::derived_serial(41, 1).unwrap(),
        41
    ));
}

#[test]
fn rejects_duplicate_or_malformed_terminal_rows() {
    for ledger in [
        "41 delivered 9 none\n41 rejected 0 not-selected\n",
        "0 delivered 9 none\n",
        "41 delivered wrong none\n",
        "41 delivered 9 none extra\n",
    ] {
        assert!(
            input::first_missing_disposition(ledger, &[41]).is_err(),
            "{ledger}"
        );
    }
    assert_eq!(
        input::first_missing_disposition("41 lost 0 overflow\n", &[41]).unwrap(),
        None
    );
}

#[test]
fn metadata_only_ack_keeps_the_captured_tree_current() {
    let source = include_str!("../libs/libuitest/src/input.rs");
    let live = include_str!("../libs/libuitest/src/pointer_live.rs");
    let function = if let Some(start) = source.find("pub fn tree_capture_current(") {
        let brace = start + source[start..].find('{').unwrap();
        let mut depth = 0;
        let end = source[brace..].bytes().position(|b| {
            if b == b'{' { depth += 1; }
            if b == b'}' { depth -= 1; }
            depth == 0
        }).unwrap();
        assert!(live.contains("tree_capture_current("));
        source[start..=brace + end].to_owned()
    } else {
        assert!(live.contains("&& newest == commit"));
        // Adapt the live guard before the repair to the same fixture inputs.
        "fn tree_capture_current(captured: u64, latest: u64, _before: u64, _after: u64) -> bool { latest == captured }".into()
    };
    let fixture = format!(r#"
{function}
fn main() {{
    // Pixel commit 7 serialized tree 3; no-change 8 acknowledges a release.
    assert!(tree_capture_current(7, 8, 3, 3), "metadata ack does not replace the pixel/tree commit");
    assert!(tree_capture_current(7, 7, 3, 3));
    assert!(!tree_capture_current(7, 8, 3, 4), "a concurrent tree publication must retry");
    assert!(!tree_capture_current(0, 8, 3, 3));
    assert!(!tree_capture_current(9, 8, 3, 3));
    assert!(!tree_capture_current(7, 8, 0, 0));
}}
"#);
    let dir = std::env::temp_dir().join(format!("ui-tree-ack-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("ack.rs"); let binary = dir.join("ack");
    std::fs::write(&path, fixture).unwrap();
    let built = std::process::Command::new("rustc").args(["--edition=2021", "-Dwarnings"])
        .arg(&path).arg("-o").arg(&binary).output().unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let ran = std::process::Command::new(&binary).output().unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
}
#[test]
fn reattaching_reader_rejects_a_previous_readers_tree() {
    let source = include_str!("../libs/libuitest/src/input.rs");
    let function = if let Some(start) = source.find("pub fn tree_ready_after_attach(") {
        let end = start + source[start..].find("\n}").unwrap() + 2;
        source[start..end].to_owned()
    } else {
        // Adapt both production pre-fix attachment guards to the fixture.
        let live = include_str!("../libs/libuitest/src/pointer_live.rs");
        let app = include_str!("../userspace/programs/src/uitest_suite/tree.rs");
        assert!(live.contains(".tree_serializations\n            == 0"));
        assert!(app.contains("if meta.tree_serializations > 0"));
        "fn tree_ready_after_attach(count: u64, baseline: u64) -> bool { let _ = baseline; count > 0 }".into()
    };
    let gate = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("libs/breengel/src/tree_commit.rs");
    let fixture = format!(r#"
#[path = {gate:?}] mod gate;
{function}
fn main() {{
    let mut count = 0;
    let mut tree = 0;
    gate::serialize_for_commit(1, &mut count, || {{ tree = 1; Ok(()) }}).unwrap();
    // The first reader leaves; commits 2..4 copy pixels but skip tree encoding.
    for pixel in 2..=4 {{
        gate::serialize_for_commit(0, &mut count, || {{ tree = pixel; Ok(()) }}).unwrap();
    }}
    assert_eq!((count, tree), (1, 1));
    let baseline = count; // sampled with the replacement mapping held
    assert!(!tree_ready_after_attach(count, baseline), "old tree accepted with commit 4 pixels");
    gate::serialize_for_commit(1, &mut count, || {{ tree = 5; Ok(()) }}).unwrap();
    assert!(tree_ready_after_attach(count, baseline));
    assert_eq!(tree, 5);
    // Metadata-only acknowledgments keep this valid pixel/tree publication.
    assert!(tree_ready_after_attach(count, baseline));
    assert!(!tree_ready_after_attach(0, 0));
}}
"#);
    let dir = std::env::temp_dir().join(format!("ui-tree-reattach-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("reattach.rs"); let binary = dir.join("reattach");
    std::fs::write(&path, fixture).unwrap();
    let built = std::process::Command::new("rustc").args(["--edition=2021", "-Dwarnings"])
        .arg(&path).arg("-o").arg(&binary).output().unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let ran = std::process::Command::new(&binary).output().unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
}
