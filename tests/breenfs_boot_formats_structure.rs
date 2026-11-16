//! Breenfs mounts explicitly; boot probing remains ext2-only in step 24.10.
#[test]
fn boot_formats_name_only_ext2() {
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
    assert_eq!(names, ["ext2"], "breenfs must stay outside filesystems!");
    assert_eq!(code.matches("pub static FORMATS:").count(), 1);
    assert!(
        code.contains("&[$($backend::FORMAT),+];"),
        "FORMATS comes from that registry"
    );
}
