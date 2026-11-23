use std::{fs, process::Command};

#[test]
fn inventory_names_each_construct() {
    let dir = std::env::temp_dir().join(format!("bmake-inventory-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("fixture.mk");
    fs::write(&file, "ifdef FLAG\nvalue := $(patsubst %.c,%.o,$(sources))\nelse\nvalue += other\nendif\n.PHONY: all\n%.o: %.c\n\t$(CC) -c $<\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_bmake"))
        .arg("--inventory").arg("-f").arg(&file).output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    for (line, kind, name) in [(1,"directive","ifdef"), (2,"assignment",":="), (2,"function","patsubst"), (3,"directive","else"), (4,"assignment","+="), (5,"directive","endif"), (6,"special-target",".PHONY"), (7,"pattern-rule","%") ] {
        assert!(text.lines().any(|row| row == format!("{}:{line}\t{kind}\t{name}", file.display())), "missing {line} {kind} {name}: {text}");
    }
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn inventory_respects_make_syntax_context() {
    let dir = std::env::temp_dir().join(format!("bmake-inventory-context-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("fixture.mk"), include_str!("inventory-context.mk")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_bmake"))
        .current_dir(&dir)
        .env_remove("MAKEFLAGS")
        .args(["--inventory", "-f", "fixture.mk"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(!dir.join("marker").exists());
    fs::remove_dir_all(dir).unwrap();

    let actual = String::from_utf8(output.stdout).unwrap();
    let expected = [
        (1, "assignment", ":="),
        (4, "directive", "include"),
        (5, "assignment", "="),
        (6, "directive", "ifdef"),
        (7, "directive", "else"),
        (7, "directive", "ifneq"),
        (8, "assignment", "+="),
        (8, "directive", "define"),
        (8, "directive", "override"),
        (10, "function", "subst"),
        (12, "directive", "endef"),
        (13, "directive", "endif"),
        (14, "pattern-rule", "static"),
        (14, "special-target", ".WAIT"),
        (15, "assignment", "+="),
        (15, "directive", "export"),
        (15, "directive", "private"),
        (15, "function", "strip"),
        (16, "function", "filter"),
        (17, "function", "file"),
        (17, "function", "info"),
        (19, "function", "warning"),
        (21, "special-target", ".PHONY"),
        (22, "assignment", ":::="),
    ]
    .map(|(line, kind, name)| format!("fixture.mk:{line}\t{kind}\t{name}"));
    assert_eq!(actual.lines().collect::<Vec<_>>(), expected);
}
