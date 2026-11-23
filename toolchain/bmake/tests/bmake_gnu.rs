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

fn run_make(makefile: &str, args: &[&str], files: &[(&str, &str)]) -> std::process::Output {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = loop {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("bmake-gnu-{}-{id}", std::process::id()));
        match fs::create_dir(&dir) {
            Ok(()) => break dir,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("create {}: {error}", dir.display()),
        }
    };
    fs::write(dir.join("fixture.mk"), makefile).unwrap();
    for (name, contents) in files {
        fs::write(dir.join(name), contents).unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_bmake"))
        .current_dir(&dir)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HIDDEN", "from environment")
        .args(["-f", "fixture.mk"])
        .args(args)
        .output()
        .unwrap();
    fs::remove_dir_all(dir).unwrap();
    output
}

#[test]
fn conditional_directives_nest() {
    let output = run_make(
        r#"MODE = active
EMPTY_REF = $(ABSENT)
NAME = EMPTY_REF
ifeq ($(MODE), broken)
not makefile syntax
ifdef EMPTY_REF
include also-absent.mk
endif
else ifneq "$(MODE)" 'disabled'
  ifdef $(NAME)
    ifndef ABSENT
      -include absent.mk present.mk
      sinclude also-absent.mk
    else
      include must-not-open.mk
    endif
  endif
else
include must-not-open.mk
endif
.PHONY: visit
all: | visit
ifeq ($(MODE),active)
	@printf '%s\n' 'rebuilt'
endif
visit:
	@printf '%s\n' '$(PRESENT)'
"#,
        &["all"],
        &[
            ("all", "existing"),
            ("visit", "existing"),
            ("present.mk", "PRESENT = included\n"),
        ],
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"included\n");
    assert!(output.stderr.is_empty(), "{output:?}");

    let output = run_make("ifdef ENABLED\nall: ; @:\n", &["all"], &[]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("fixture.mk:1: missing endif"),
        "{output:?}"
    );
}

#[test]
fn define_export_override() {
    let output = run_make(
        r#"PHASE = early
export NOW := $(PHASE)
override define TEXT =
# keep this text
$(PHASE)
define INNER
literal
endef
endef
export TEXT
PHASE = late
TEXT = ignored
override FLAGS += forced
export FLAGS
unexport HIDDEN
VAR = global
all: parent sibling
parent: VAR := $(VAR)-parent
parent: export PRIVATE = parent-only
parent: child
	@printf '%s:%s\n' '$(VAR)' "$$PRIVATE"
child:
	@printf '%s:%s\n' '$(VAR)' "$$PRIVATE"
sibling:
	@printf '%s:%s:%s\n' '$(VAR)' "$$NOW" "$${HIDDEN-unset}"
	@printf '%s\n' "$$FLAGS" "$$TEXT"
"#,
        &["all", "FLAGS=command", "TEXT=command"],
        &[],
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        output.stdout,
        b"global-parent:parent-only\nglobal-parent:parent-only\nglobal:early:unset\ncommand forced\n# keep this text\nlate\ndefine INNER\nliteral\nendef\n"
    );
    assert!(output.stderr.is_empty(), "{output:?}");
}
