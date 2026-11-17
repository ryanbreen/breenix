//! Boot formats ordered by coordinator ruling E24 ledger line 265.
#[test]
fn boot_formats_probe_ext2_then_breenfs() {
    let source = include_str!("../kernel/src/fs/mod.rs");
    let code = source
        .lines()
        .map(|line| line.split("//").next().unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    let invocations: Vec<_> = code.match_indices("filesystems!").collect();
    assert_eq!(invocations.len(), 1, "one boot-format registry invocation");
    let tail = code[invocations[0].0 + "filesystems!".len()..].trim_start();
    let body = tail
        .strip_prefix('{')
        .expect("registry uses braces")
        .split_once('}')
        .expect("registry closes")
        .0;
    let names: Vec<_> = body
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|name| !name.is_empty())
        .collect();
    assert_eq!(names, ["ext2", "breenfs"], "ext2 must be probed first");
    assert_eq!(code.matches("pub static FORMATS:").count(), 1);
    assert!(
        code.contains("&[$($backend::FORMAT),+];"),
        "FORMATS comes from that registry"
    );
}
