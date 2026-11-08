//! Exercise the input stream's production modifier fold on the host.
use std::{fs, process::Command};

#[test]
fn modifier_fold_respects_the_grab_selection() {
    let source = include_str!("../kernel/src/input/stream.rs");
    let fold = if let Some(start) = source.find("fn fold_modifiers(") {
        let brace = start + source[start..].find('{').unwrap();
        let mut depth = 0;
        let end = source[brace..].bytes().position(|byte| {
            if byte == b'{' { depth += 1; }
            if byte == b'}' { depth -= 1; }
            depth == 0
        }).unwrap();
        source[start..=brace + end].to_owned()
    } else {
        // Adapt main's actual expression to the extracted function's interface.
        assert!(source.contains("self.devices.values().fold(0, |all, d| all | d.modifiers)"));
        "fn fold_modifiers(devices: impl Iterator<Item = (u32, u8)>, _selection: Option<&[u32]>) -> u8 { devices.fold(0, |all, (_, modifiers)| all | modifiers) }".to_owned()
    };
    let dir = std::env::temp_dir().join(format!("input-modifier-fold-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("fold.rs");
    fs::write(&path, format!(r#"
{fold}
fn main() {{
    let devices = [(1, 0u8), (2, 1u8)]; // selected K, unselected Q holding Shift
    assert_eq!(fold_modifiers(devices.into_iter(), Some(&[1])), 0, "Q must not shift K's event");
    assert_eq!(fold_modifiers(devices.into_iter(), None), 1);
    assert_eq!(fold_modifiers(devices.into_iter(), Some(&[])), 1);
    assert_eq!(fold_modifiers(devices.into_iter(), Some(&[1, 2])), 1);
}}
"#)).unwrap();
    let compile = Command::new("rustc").args(["--edition=2021", "-Dwarnings"]).arg(&path).arg("-o").arg(dir.join("fold")).output().unwrap();
    assert!(compile.status.success(), "{}", String::from_utf8_lossy(&compile.stderr));
    let result = Command::new(dir.join("fold")).output().unwrap();
    fs::remove_dir_all(dir).unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
}
