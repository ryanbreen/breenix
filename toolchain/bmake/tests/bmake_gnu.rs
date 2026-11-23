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
        let path = dir.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
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

#[test]
fn pattern_rule_with_patsubst() {
    // GNU make 4.4.1 §§4.11, 6.3.1, 8.2 and 10.5.
    let output = run_make(
        r#"SOURCES = src/alpha.c
OBJECTS := $(patsubst %.c,%.obj,$(SOURCES))
GENERATED = src/beta.gen
TARGETS := $(GENERATED:%.gen=%.out)

%.obj: %.c missing
	@printf 'unusable pattern\n'
%.obj: %.c
	@printf 'generic pattern\n'
src/%.obj: src/%.c src/common.h src/common.h | stamp
	@printf '%s\n' '$@|$<|$*|$(@D)|$(@F)|$(<D)|$(<F)' '$(^D)|$(^F)|$(+D)|$(+F)|$(?D)|$(?F)|$|'

all: $(OBJECTS) $(TARGETS) posix.o
$(TARGETS): %.out: %.gen
	@printf '%s\n' '$@|$<|$*|$(*D)|$(*F)'
.c.o:
	@printf '%s\n' 'suffix:$<'
%.o: %.c
	@printf 'pattern shadows suffix\n'
"#,
        &[],
        &[
            ("src/alpha.c", ""),
            ("src/common.h", ""),
            ("src/beta.gen", ""),
            ("posix.c", ""),
            ("stamp", ""),
        ],
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        output.stdout,
        b"src/alpha.obj|src/alpha.c|alpha|src|alpha.obj|src|alpha.c\nsrc src|alpha.c common.h|src src src|alpha.c common.h common.h|src src|alpha.c common.h|stamp\nsrc/beta.out|src/beta.gen|src/beta|src|beta\nsuffix:posix.c\n"
    );
    assert!(output.stderr.is_empty(), "{output:?}");
}

#[test]
fn text_and_filename_functions() {
    // GNU make 4.4.1 §§4.4.3, 6.3.1, 8.2 and 8.3.
    let cases = [
        (r"$(subst ee,EE,feet on the street)", "fEEt on the strEEt"),
        (r"$(subst $(space),$(comma),a b c)", "a,b,c"),
        (r"$(subst a,b,a(a),a)", "b(b),b"),
        (r"$(patsubst %.c,%.o, x.c.c   bar.c )", "x.c.o bar.o"),
        (r"$(patsubst \%.c,yes,%.c bar.c)", "yes bar.c"),
        (r"$(patsubst %.c,\%-%,bar.c)", "%-bar"),
        (r"$(objects:%.o=%.c)", "foo.c main1.c bar.c main2.c"),
        (r"$(strip   a  b    c  )", "a b c"),
        (r"$(findstring a,a b c)|$(findstring a,b c)", "a|"),
        (r"$(filter %.c %.s,foo.c bar.c baz.s ugh.h)", "foo.c bar.c baz.s"),
        (r"$(filter-out $(mains),$(objects))", "foo.o bar.o"),
        (r"$(sort foo bar lose foo)", "bar foo lose"),
        (r"$(word 2, foo bar baz)|$(word 4,foo bar baz)", "bar|"),
        (r"$(wordlist 2, 3, foo bar baz)", "bar baz"),
        (r"$(wordlist 3,0,foo bar baz)", ""),
        (r"$(words foo bar baz)", "3"),
        (r"$(firstword foo bar)|$(lastword foo bar)", "foo|bar"),
        (r"$(dir src/foo.c hacks)", "src/ ./"),
        (r"$(notdir src/foo.c hacks)", "foo.c hacks"),
        (r"$(suffix src/foo.c src-1.0/bar.c hacks)", ".c .c"),
        (r"$(basename src/foo.c src-1.0/bar hacks)", "src/foo src-1.0/bar hacks"),
        (r"$(addsuffix .c,foo bar)", "foo.c bar.c"),
        (r"$(addprefix src/,foo bar)", "src/foo src/bar"),
        (r"$(join a b c,.c .o)", "a.c b.o c"),
        (r"$(wildcard ./[ab]?.c *.h none*)", "./a1.c ./b2.c one.h"),
        (r"$(abspath /a//b/../missing /..)", "/a/missing /"),
        (r"$(realpath /./ bmake-no-such-file)", "/"),
    ];
    let mut makefile = String::from(
        "empty :=\nspace := $(empty) $(empty)\ncomma := ,\n\
         objects := foo.o   main1.o bar.o main2.o\nmains := main1.o main2.o\nall:\n",
    );
    for (expression, _) in cases {
        makefile.push_str(&format!("\t@printf '<%s>\\n' '{expression}'\n"));
    }
    let output = run_make(
        &makefile,
        &[],
        &[("b2.c", ""), ("a1.c", ""), ("one.h", ""), (".a0.c", "")],
    );
    assert!(output.status.success(), "{output:?}");
    let stdout = std::str::from_utf8(&output.stdout).unwrap();
    let actual: Vec<_> = stdout.lines().collect();
    assert_eq!(actual.len(), cases.len());
    for ((expression, expected), actual) in cases.into_iter().zip(actual) {
        assert_eq!(actual, format!("<{expected}>"), "{expression}");
    }
    assert!(output.stderr.is_empty(), "{output:?}");
}
