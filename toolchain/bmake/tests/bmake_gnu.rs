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
