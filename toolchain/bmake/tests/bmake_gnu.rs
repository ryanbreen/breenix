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
    with_makefile(makefile, files, |make, _| make.args(args).output().unwrap())
}

fn with_makefile<T>(
    makefile: &str,
    files: &[(&str, &str)],
    run: impl FnOnce(&mut Command, &std::path::Path) -> T,
) -> T {
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
    let mut make = Command::new(env!("CARGO_BIN_EXE_bmake"));
    make.current_dir(&dir)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HIDDEN", "from environment")
        .args(["-f", "fixture.mk"]);
    let output = run(&mut make, &dir);
    fs::remove_dir_all(dir).unwrap();
    output
}

#[test]
fn call_eval_foreach() {
    // GNU make 4.4.1 §§8.4–8.6 and 8.8–8.14, with explicit goals from §9.2.
    let output = run_make(
        r#"item = outside
raw = $(item)
simple := initial
override forced = yes
define rule
$(1): ; @printf '%s\n' '$$@|$$(item)|$$(origin @)'
endef
all: $(if $(filter all,$(MAKECMDGOALS)),alpha beta,$(error missing goal))
$(foreach item,alpha beta,$(eval $(call rule,$(item))))
define raw_definition
FROM_EVAL := $(item)
endef
$(eval $(value raw_definition))
before := $(FROM_EVAL)
change := $(eval FROM_EVAL := good)$(FROM_EVAL)
short := $(or ,chosen,$(error dead or))|$(and yes,,$(error dead and))|$(if ,$(error dead if),safe)
captured := $(shell printf 'a\r\nb\n\n'; exit 7)
status := $(.SHELLSTATUS)
$(info $(before)|$(change)|$(short)|$(captured)|$(status))
$(warning continuing)
all:
	@printf '%s\n' '$(value raw)|$(flavor raw)|$(flavor simple)|$(flavor missing)'
	@printf '%s\n' '$(call origin,CC)|$(origin HIDDEN)|$(origin CLI)|$(origin forced)'
"#,
        &["CLI=argument", "all"],
        &[],
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        output.stdout,
        b"outside|good|chosen||safe|a b|7\nalpha|outside|automatic\nbeta|outside|automatic\n$(item)|recursive|simple|undefined\ndefault|environment|command line|override\n"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(stderr, "fixture.mk:20: continuing\n");

    let fatal = run_make("$(error stop here)\nall: ; @:\n", &[], &[]);
    assert_eq!(fatal.status.code(), Some(2), "{fatal:?}");
    assert_eq!(fatal.stderr, b"fixture.mk:1: stop here\n");

    let fatal = run_make(
        "all: broken later\nbroken: ; $(error stop here)\nlater: ; @echo unexpected\n",
        &["-k"],
        &[],
    );
    assert_eq!(fatal.status.code(), Some(2), "{fatal:?}");
    assert_eq!(fatal.stderr, b"fixture.mk:2: stop here\n");
    assert!(fatal.stdout.is_empty(), "{fatal:?}");
}

#[test]
fn command_change_rebuilds() {
    // GNU make 4.4.1 §§4.7, 8.4 and 8.7. Compare the recorded command on
    // each invocation, and write its replacement only when the recipe runs.
    let makefile = r#"TEXT = first
command = printf '%s\n' $(TEXT) > $@
out: FORCE
	$(if $(findstring |$(command)|,|$(file <$@.cmd)|),,$(command))
	@$(if $(findstring |$(command)|,|$(file <$@.cmd)|),,$(file >$@.cmd,$(command)))
.PHONY: FORCE
"#;
    with_makefile(makefile, &[], |make, dir| {
        let first = make.output().unwrap();
        assert!(first.status.success(), "{first:?}");
        assert_eq!(first.stdout, b"printf '%s\\n' first > out\n");
        assert_eq!(fs::read(dir.join("out")).unwrap(), b"first\n");
        assert_eq!(fs::read(dir.join("out.cmd")).unwrap(), first.stdout);

        let unchanged = make.output().unwrap();
        assert!(unchanged.status.success(), "{unchanged:?}");
        assert!(unchanged.stdout.is_empty(), "{unchanged:?}");

        let changed = make.arg("TEXT=second").output().unwrap();
        assert!(changed.status.success(), "{changed:?}");
        assert_eq!(changed.stdout, b"printf '%s\\n' second > out\n");
        assert_eq!(fs::read(dir.join("out")).unwrap(), b"second\n");
        assert_eq!(fs::read(dir.join("out.cmd")).unwrap(), changed.stdout);
    });
}

#[test]
fn recursive_kbuild_flags() {
    // GNU make 4.4.1 §§3.3, 5.7.3 and 9.8.
    let output = run_make(
        r#"MAKEFLAGS += --no-print-directory --include-dir=includes -rR
all:
	@$(MAKE) -f child.mk check
"#,
        &["-n"],
        &[
            ("child.mk", "include values.mk\ncheck: ; @echo $(VALUE) $(origin CC)\n"),
            ("includes/values.mk", "VALUE = inherited\n"),
        ],
    );
    assert!(output.status.success(), "{output:?}");
    let stdout = std::str::from_utf8(&output.stdout).unwrap();
    assert_eq!(stdout.lines().last(), Some("echo inherited undefined"));
    assert!(output.stderr.is_empty(), "{output:?}");
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
    // GNU make 4.4.1 §§4.3.3, 6.3.1, 8.2 and 8.3.
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
