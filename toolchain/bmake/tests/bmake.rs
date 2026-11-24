use std::{
    fs::{self, File},
    io::ErrorKind,
    os::unix::process::CommandExt,
    path::PathBuf,
    process::{self, Command, Output, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, UNIX_EPOCH},
};

struct Project(PathBuf);

impl Project {
    fn new(makefile: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let project = loop {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("bmake-{}-{id}", process::id()));
            match fs::create_dir(&path) {
                Ok(()) => break Self(path),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create {}: {error}", path.display()),
            }
        };
        fs::write(project.0.join("project.mk"), makefile).unwrap();
        project
    }

    fn fixture(&self, name: &str, contents: &str, seconds: u64) {
        let path = self.0.join(name);
        fs::write(&path, contents).unwrap();
        let modified = UNIX_EPOCH + Duration::from_secs(1_700_000_000 + seconds);
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
    }

    fn build(&self) -> Output {
        self.command().arg("project.mk").output().unwrap()
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_bmake"));
        command.current_dir(&self.0);
        command
    }

    fn expect_error(&self, target: &str, reason: &str) {
        let output = self.build();
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains(target), "{stderr}");
        assert!(stderr.contains(reason), "{stderr}");
        assert!(!stderr.contains("panicked"), "{stderr}");
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn rebuilds_when_prerequisite_newer() {
    let project = Project::new(
        "output: older input\n\t@cd subdir\n\t@cat input > output\ninput: source\n\t@cp -p source input\n",
    );
    fs::create_dir(project.0.join("subdir")).unwrap();
    project.fixture("older", "", 0);
    project.fixture("input", "stale input", 0);
    project.fixture("output", "stale output", 10);
    project.fixture("source", "rebuilt input", 20);

    let output = project.build();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(project.0.join("input")).unwrap(),
        "rebuilt input"
    );
    assert_eq!(
        fs::read_to_string(project.0.join("output")).unwrap(),
        "rebuilt input"
    );
}

#[test]
fn equal_mtimes_are_not_out_of_date() {
    let project = Project::new("output: input\n\t@printf rebuilt > output\n");
    project.fixture("output", "keep", 0);
    project.fixture("input", "input", 0);

    let output = project.build();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(project.0.join("output")).unwrap(),
        "keep"
    );
}

#[test]
fn noop_when_current() {
    let project = Project::new("output: input\n\tprintf rebuilt > output\n");
    project.fixture("input", "input", 0);
    project.fixture("output", "keep", 10);
    let path = project.0.join("output");
    let before = fs::metadata(&path).unwrap().modified().unwrap();

    let output = project.build();
    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
    assert_eq!(fs::read_to_string(&path).unwrap(), "keep");

    fs::remove_file(project.0.join("input")).unwrap();
    project.expect_error("input", "no rule");
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
}

#[test]
fn failed_recipe_exits_2() {
    let project = Project::new(concat!(
        "broken:\n",
        "\t  @-exit 19\n",
        "\tprintf 'ran\\n'\n",
        "\t@exit 37\n",
        "\t@printf unexpected > later\n",
    ));

    let output = project.build();
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert_eq!(output.stdout, b"printf 'ran\\n'\nran\n");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("broken"), "{stderr}");
    assert!(stderr.contains("status 37"), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert!(!project.0.join("later").exists());
}

#[test]
fn signalled_recipe_reported() {
    let project = Project::new("interrupted:\n\t@exec /bin/sh signal-self.sh\n");
    project.fixture("signal-self.sh", "kill -TERM \"$$\"\n", 0);

    project.expect_error("interrupted", "signal 15");
}

#[test]
fn cycle_exits_2() {
    let project = Project::new("first: second\nsecond: first\n");
    project.expect_error("project.mk:2:", "first -> second -> first");
}

fn expect_stdout(output: Output, expected: &str) {
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
    assert!(output.stderr.is_empty(), "{:?}", output.stderr);
}

#[test]
fn macro_precedence_env_e() {
    let project = Project::new(
        "BMAKE_TEST_ORIGIN = makefile\nall:\n\t@printf '<%s>\\n' '$(BMAKE_TEST_ORIGIN)'\n",
    );
    for (environment, options, expected) in [
        ("environment", &[][..], "<makefile>\n"),
        ("environment", &["-e"][..], "<environment>\n"),
        ("", &["-e"][..], "<>\n"),
        (
            "environment",
            &["BMAKE_TEST_ORIGIN=command"][..],
            "<command>\n",
        ),
        (
            "environment",
            &["-e", "BMAKE_TEST_ORIGIN=command"][..],
            "<command>\n",
        ),
    ] {
        let output = project
            .command()
            .args(["-f", "project.mk"])
            .args(options)
            .env("BMAKE_TEST_ORIGIN", environment)
            .output()
            .unwrap();
        expect_stdout(output, expected);
    }
}

#[test]
fn assignment_operators_posix_2024() {
    let immediate = Project::new(concat!(
        "INPUT = early\n",
        "VALUE ::= $(INPUT) $$(INPUT)\n",
        "INPUT = late\n",
        "all:\n\t@printf '<%s>\\n' '$(VALUE)'\n",
    ));
    let expanded_delayed = Project::new(concat!(
        "INPUT = early\n",
        "ESCAPED = $$(INPUT)\n",
        "VALUE :::= $(INPUT) $(ESCAPED)\n",
        "INPUT = late\n",
        "all:\n\t@printf '<%s>\\n' '$(VALUE)'\n",
    ));
    let append = Project::new(concat!(
        "INPUT = early\n",
        "IMMEDIATE ::= start\n",
        "DELAYED = start\n",
        "IMMEDIATE += $(INPUT)\n",
        "DELAYED += $(INPUT)\n",
        "BMAKE_TEST_NEW_APPEND += $(INPUT)\n",
        "INPUT = late\n",
        "all:\n",
        "\t@printf '<%s>\\n' '$(IMMEDIATE)' '$(DELAYED)' '$(BMAKE_TEST_NEW_APPEND)'\n",
    ));
    let conditional = Project::new(concat!(
        "INPUT = early\n",
        "EMPTY =\n",
        "EMPTY ?= ignored\n",
        "BMAKE_TEST_NEW_CONDITIONAL ?= $(INPUT)\n",
        "BMAKE_TEST_NEW_CONDITIONAL ?= ignored\n",
        "INPUT = late\n",
        "all:\n\t@printf '<%s>\\n' '$(EMPTY)' '$(BMAKE_TEST_NEW_CONDITIONAL)'\n",
    ));
    let shell = Project::new(concat!(
        "INPUT = early\n",
        "VALUE != v=$(INPUT); printf ' \\t\\n%s\\n%s\\n\\n' \"$$v\" second; printf x >> calls\n",
        "FAILED != printf '\\044(INPUT)\\n'; exit 7\n",
        "INPUT = late\n",
        "all:\n\t@printf '<%s>\\n' '$(VALUE)' '$(VALUE)' '$(FAILED)'\n",
    ));

    expect_stdout(immediate.build(), "<early $(INPUT)>\n");
    expect_stdout(expanded_delayed.build(), "<early $(INPUT)>\n");
    expect_stdout(
        append
            .command()
            .arg("project.mk")
            .env_remove("BMAKE_TEST_NEW_APPEND")
            .output()
            .unwrap(),
        "<start early>\n<start late>\n<late>\n",
    );
    expect_stdout(
        conditional
            .command()
            .arg("project.mk")
            .env_remove("BMAKE_TEST_NEW_CONDITIONAL")
            .output()
            .unwrap(),
        "<>\n<late>\n",
    );

    let output = shell.build();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"<early second >\n<early second >\n<late>\n");
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostic.contains("project.mk:3:"), "{diagnostic}");
    assert!(diagnostic.contains("exited with status 7"), "{diagnostic}");
    assert_eq!(fs::read(shell.0.join("calls")).unwrap(), b"x");
}

#[test]
fn substitution_reference() {
    let project = Project::new(concat!(
        "FILES = dir/input.c  untouched  not.c.bak\n",
        "NAME = FILES\n",
        "SUFFIX = .o\n",
        "all: ${$(NAME):.c=$(SUFFIX)}\n",
        "\t@printf '%s|%s|%s\\n' '$(FILES:.c=$(SUFFIX))' '${FILES:.bak=}' '$$HOME'\n",
        "SUFFIX = .obj\n",
    ));
    fs::create_dir(project.0.join("dir")).unwrap();
    project.fixture("untouched", "", 0);
    project.fixture("not.c.bak", "", 0);
    project.fixture("dir/input.o", "", 0);
    expect_stdout(
        project.build(),
        "dir/input.obj  untouched  not.c.bak|dir/input.c  untouched  not.c|$HOME\n",
    );
}

#[test]
fn automatic_variables() {
    let project = Project::new(concat!(
        ".POSIX:\n",
        ".SUFFIXES: .src .obj\n",
        ".src.obj:\n",
        "\t@printf 'inferred %s|%s|%s|%s\\n' '$@' '$<' '$?' '$*'\n",
        "\t@cp -p '$<' '$@'\n",
        "all.obj: older equal newer sub/item.obj\n",
        "\t@printf 'explicit %s|%s|%s|%s\\n' '$@' '$?' '$*' '$(?F)'\n",
    ));
    fs::create_dir(project.0.join("sub")).unwrap();
    project.fixture("all.obj", "", 10);
    project.fixture("older", "", 0);
    project.fixture("equal", "", 10);
    project.fixture("newer", "", 20);
    project.fixture("sub/item.src", "", 20);
    expect_stdout(
        project.build(),
        concat!(
            "inferred sub/item.obj|sub/item.src|sub/item.src|sub/item\n",
            "explicit all.obj|newer sub/item.obj|all|newer item.obj\n",
        ),
    );
}

#[test]
fn include_reads_in_order() {
    let project = Project::new(concat!(
        "all:\n",
        "\t@printf '%s|%s\\n' '$(VALUE)' '$(LAST)'\n",
        "VALUE = outer\n",
        "INCLUDES = sub/first.mk tail.mk\n",
        "include $(INCLUDES) # read in this order\n",
        "-include missing.mk good.mk\n",
        "LAST = final\n",
    ));
    fs::create_dir(project.0.join("sub")).unwrap();
    project.fixture("sub/first.mk", "include nested.mk\n", 0);
    project.fixture("nested.mk", "VALUE = nested\n", 0);
    project.fixture("tail.mk", "VALUE = tail-$(LAST)\n", 0);
    project.fixture(
        "good.mk",
        "all: included\nincluded:\n\t@printf 'included\\n'\n",
        0,
    );
    expect_stdout(project.build(), "included\ntail-final|final\n");

    fs::remove_file(project.0.join("nested.mk")).unwrap();
    project.expect_error("sub/first.mk:1:", "nested.mk");
}

#[test]
#[allow(non_snake_case, reason = "The check name preserves the case of -C")]
fn option_C_and_f() {
    let project = Project::new("all:\n\t@printf wrong\n");
    fs::create_dir(project.0.join("sub")).unwrap();
    project.fixture("sub/Makefile", "all:\n\t@printf wrong\n", 0);
    project.fixture("sub/settings.mk", "TEXT = first\n", 0);
    project.fixture(
        "sub/rules.mk",
        "unused:\n\t@printf wrong\nall:\n\t@printf '%s:' '$(TEXT)'\n\t@cat input\n",
        0,
    );
    project.fixture("sub/input", "changed directory\n", 0);
    let output = project
        .command()
        .args(["-f", "settings.mk", "-C", "sub", "-f", "rules.mk", "all"])
        .output()
        .unwrap();
    expect_stdout(output, "first:changed directory\n");
}

#[test]
fn self_referencing_macro_is_error_not_panic() {
    for (definitions, cycle) in [
        ("A = $(A)\n", "A -> A"),
        ("A = ${B}\nB = $(A)\n", "A -> B -> A"),
    ] {
        let project = Project::new(&format!("{definitions}all:\n\t@printf '%s' '$(A)'\n"));
        let line = definitions.lines().count();
        project.expect_error(&format!("project.mk:{line}:"), cycle);
    }
}

#[test]
fn default_c_inference_uses_bundle_compiler_options() {
    let project = Project::new("all: app item.o\n");
    project.fixture("app.c", "int main(void) { return 0; }\n", 0);
    project.fixture("item.c", "int item;\n", 0);
    let output = project
        .command()
        .env_remove("CC")
        .env_remove("CFLAGS")
        .args(["-n", "-f", "project.mk"])
        .output()
        .unwrap();
    let recipes: Vec<_> = std::str::from_utf8(&output.stdout)
        .unwrap()
        .lines()
        .map(|line| line.split_ascii_whitespace().collect::<Vec<_>>())
        .collect();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        recipes,
        [vec!["cc", "-o", "app", "app.c"], vec!["cc", "-c", "item.c"]]
    );
}

#[test]
fn suffix_inference_builds_object() {
    let project = Project::new(concat!(
        ".POSIX:\n",
        "CC = /bin/sh record-cc.sh\n",
        "AS = /bin/sh record-as.sh\n",
        ".c.o:\n",
        "\t@$(CC) -c $< -o $@\n",
        "all: item.o app helper asm.o\n",
    ));
    project.fixture(
        "record-cc.sh",
        "printf '%s\\n' \"$*\" >> cc.log\n: > \"$4\"\n",
        0,
    );
    project.fixture(
        "record-as.sh",
        "printf '%s\\n' \"$*\" > as.log\n: > \"$2\"\n",
        0,
    );
    project.fixture("item.c", "int item;\n", 0);
    project.fixture("app.c", "int main(void) { return 0; }\n", 0);
    project.fixture("helper.sh", "echo helper\n", 0);
    project.fixture("asm.s", "# assembly input\n", 0);
    let output = project.build();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(project.0.join("cc.log")).unwrap(),
        "-c item.c -o item.o\n-O 1 -o app app.c\n"
    );
    assert_eq!(
        fs::read_to_string(project.0.join("as.log")).unwrap(),
        "-o asm.o asm.s\n"
    );
    for name in ["item.o", "app", "helper", "asm.o"] {
        assert!(project.0.join(name).is_file(), "{name}");
    }
    assert_eq!(
        fs::read_to_string(project.0.join("helper")).unwrap(),
        "echo helper\n"
    );
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        fs::metadata(project.0.join("helper"))
            .unwrap()
            .permissions()
            .mode()
            & 0o111,
        0o111
    );

    let project = Project::new(concat!(
        ".SUFFIXES:\n",
        ".SUFFIXES: .two .one .out\n",
        ".one.out: ; @printf one > $@\n",
        ".two.out: ; @printf two > $@\n",
        "all: preferred.out\n",
    ));
    project.fixture("preferred.one", "", 0);
    project.fixture("preferred.two", "", 0);
    expect_stdout(project.build(), "");
    assert_eq!(
        fs::read_to_string(project.0.join("preferred.out")).unwrap(),
        "two"
    );
    project.fixture("missing.c", "", 0);
    let output = project
        .command()
        .args(["-f", "project.mk", "missing.o"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(!project.0.join("missing.o").exists());
}

#[test]
fn phony_always_runs() {
    let project = Project::new(concat!(
        ".PHONY: all visit\n",
        "all: visit fallback\n",
        "\t@printf 'all\\n'\n",
        "visit:\n",
        "\t@printf 'visit\\n'\n",
        "fallback: future\n",
        ".DEFAULT: ; @printf 'default %s|%s\\n' '$@' '$<'\n",
    ));
    project.fixture("all", "", 0);
    project.fixture("visit", "", 0);
    for _ in 0..2 {
        expect_stdout(project.build(), "visit\ndefault future|future\nall\n");
    }
    assert_eq!(fs::read_to_string(project.0.join("visit")).unwrap(), "");
}

#[test]
fn dry_run_prints_without_running() {
    let project = Project::new(
        "all: input\n\t@printf 'new\\n' > all\ninput:\n\t@printf 'input\\n' > input\n",
    );
    project.fixture("all", "current\n", 10);
    let output = project
        .command()
        .args(["-n", "-f", "project.mk"])
        .output()
        .unwrap();
    expect_stdout(output, "printf 'input\\n' > input\nprintf 'new\\n' > all\n");
    assert!(!project.0.join("input").exists());
    assert_eq!(
        fs::read_to_string(project.0.join("all")).unwrap(),
        "current\n"
    );
}

#[test]
fn keep_going_builds_independent_targets_and_exits_2() {
    let project = Project::new(concat!(
        "all: dependent independent also\n",
        "dependent: broken\n",
        "\t@printf wrong > dependent\n",
        "broken:\n",
        "\t@exit 17\n",
        "\t@printf wrong > broken\n",
        "independent:\n",
        "\t@printf built > independent\n",
        "also: missing\n",
        "\t@printf wrong > also\n",
    ));
    let output = project
        .command()
        .args(["-k", "-f", "project.mk"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("target 'broken': recipe exited with status 17"),
        "{stderr}"
    );
    assert!(
        stderr.contains("no rule to make prerequisite 'missing'"),
        "{stderr}"
    );
    assert_eq!(
        fs::read_to_string(project.0.join("independent")).unwrap(),
        "built"
    );
    for name in ["all", "dependent", "broken", "also"] {
        assert!(!project.0.join(name).exists(), "{name}");
    }
}

#[test]
fn ignore_errors_continues() {
    for (special, flags) in [
        ("", &["-i"][..]),
        (".IGNORE: all\n", &[]),
        (".IGNORE:\n", &[]),
    ] {
        let project = Project::new(&format!(
            "{special}all:\n\t@exit 17\n\t@printf continued > all\n"
        ));
        let output = project
            .command()
            .args(flags)
            .args(["-f", "project.mk"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(String::from_utf8(output.stderr)
            .unwrap()
            .contains("status 17"));
        assert_eq!(
            fs::read_to_string(project.0.join("all")).unwrap(),
            "continued"
        );
    }
}

#[test]
fn silent_suppresses_echo() {
    for (special, flags) in [
        ("", &["-s"][..]),
        (".SILENT: all\n", &[]),
        (".SILENT:\n", &[]),
    ] {
        let project = Project::new(&format!("{special}all:\n\tprintf 'ran\\n'\n"));
        let output = project
            .command()
            .args(flags)
            .args(["-f", "project.mk"])
            .output()
            .unwrap();
        expect_stdout(output, "ran\n");
    }
}

#[test]
fn precious_preserves_an_interrupted_output() {
    let project = Project::new(concat!(
        ".PRECIOUS: keep\n",
        "keep discard:\n",
        "\t@printf partial > $@; kill -TERM $$$$\n",
    ));
    let output = project
        .command()
        .args(["-k", "-f", "project.mk", "keep", "discard"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    for target in ["keep", "discard"] {
        assert!(
            stderr.contains(&format!(
                "target '{target}': recipe terminated by signal 15"
            )),
            "{stderr}"
        );
    }
    assert_eq!(
        fs::read_to_string(project.0.join("keep")).unwrap(),
        "partial"
    );
    assert!(!project.0.join("discard").exists());
}

#[test]
fn recursive_makeflags_passes_options_and_macros() {
    let project = Project::new(concat!(
        "MAKEFLAGS += -s FROM_FILE='file value'\n",
        "all:\n",
        "\t@test \"$(MAKEFLAGS)\" = \"$$MAKEFLAGS\"\n",
        "\t@$(MAKE) -f child.mk\n",
    ));
    project.fixture(
        "child.mk",
        concat!(
            "TOKEN = wrong\n",
            "all: broken independent\n",
            "broken:\n",
            "\t@exit 17\n",
            "independent:\n",
            "\tprintf '%s\\n' \"$(TOKEN)|$(FROM_FILE)|$$TOKEN\"\n",
        ),
        0,
    );
    let output = project
        .command()
        .env("MAKEFLAGS", "-k TOKEN=environment")
        .args(["-f", "project.mk", "TOKEN=two words 'quoted'"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("child.mk:4: target 'broken': recipe exited with status 17"),
        "{stderr}"
    );
    assert_eq!(
        output.stdout,
        b"two words 'quoted'|file value|two words 'quoted'\n"
    );

    project.fixture(
        "project.mk",
        "all:\n\t@printf wrong > parent-ordinary\n\t@printf started > parent-recursive; $(MAKE) -f child.mk\n",
        0,
    );
    project.fixture("child.mk", "all:\n\t@printf wrong > child-ordinary\n", 0);
    let output = project
        .command()
        .env("MAKEFLAGS", "ks")
        .args(["-n", "-f", "project.mk"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(project.0.join("parent-recursive")).unwrap(),
        "started"
    );
    for name in ["parent-ordinary", "child-ordinary"] {
        assert!(!project.0.join(name).exists(), "{name}");
    }
}

#[test]
fn include_remade_then_reread() {
    let project = Project::new(concat!(
        "include generated.mk\n",
        "$(GOAL):\n\t@printf '%s|%s' '$(VALUE)' '$(STALE)' > $@\n",
        "generated.mk: source\n\t@cp -p source $@\n",
    ));
    project.fixture("generated.mk", "GOAL = old\nSTALE = wrong\n", 0);
    project.fixture("source", "GOAL = fresh\nVALUE = updated\n", 10);
    let output = project.build();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(project.0.join("fresh")).unwrap(),
        "updated|"
    );
    assert!(!project.0.join("old").exists());
    assert_eq!(
        fs::read_to_string(project.0.join("generated.mk")).unwrap(),
        "GOAL = fresh\nVALUE = updated\n"
    );

    let project = Project::new(concat!(
        "all:\n\t@printf '%s' '$(VALUE)' > all\n",
        "include generated.mk\n",
        "generated.mk:\n\t@cp source $@\n",
    ));
    project.fixture("source", "VALUE = created\n", 0);
    let output = project
        .command()
        .args(["-n", "-f", "project.mk"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("printf '%s' 'created' > all"));
    assert_eq!(
        fs::read_to_string(project.0.join("generated.mk")).unwrap(),
        "VALUE = created\n"
    );
    assert!(!project.0.join("all").exists());
}

#[test]
fn archive_member_rebuilt_when_source_newer() {
    let project = Project::new(concat!(
        "CC = sh compile.sh\n",
        "CFLAGS = -DARCHIVE\n",
        "AR = sh archive.sh\n",
        "ARFLAGS = -rv\n",
        "lib.a: lib.a(member.o)\n",
    ));
    project.fixture("member.c", "updated source", 10);
    project.fixture("lib.a", &member_archive(0), 30);
    project.fixture("rebuilt.a", &member_archive(20), 30);
    project.fixture(
        "compile.sh",
        "test \"$*\" = '-c -DARCHIVE member.c' && cp member.c member.o",
        0,
    );
    project.fixture(
        "archive.sh",
        "test \"$*\" = '-rv lib.a member.o' && cmp member.o member.c && cp -p rebuilt.a lib.a",
        0,
    );

    expect_stdout(
        project.build(),
        "sh compile.sh -c -DARCHIVE member.c\nsh archive.sh -rv lib.a member.o\nrm -f member.o\n",
    );
    assert_eq!(
        fs::read_to_string(project.0.join("lib.a")).unwrap(),
        member_archive(20)
    );
    assert!(!project.0.join("member.o").exists());
    expect_stdout(project.build(), "");
}

#[test]
fn archive_member_time_from_header() {
    let project = Project::new(concat!(
        "output: lib.a(member.o)\n",
        "\t@printf '%s\\n' '$?' > output\n",
        "lib.a(member.o): trigger\n",
        "\t@printf '%s\\n' '$@|$%' > names\n",
    ));
    project.fixture("output", "old", 10);
    project.fixture("trigger", "", 0);
    project.fixture("lib.a", &member_archive(20), 0);

    expect_stdout(project.build(), "");
    assert_eq!(
        fs::read_to_string(project.0.join("output")).unwrap(),
        "lib.a(member.o)\n"
    );
    assert!(!project.0.join("names").exists());
    assert!(!project.0.join("member.o").exists());

    project.fixture("trigger", "", 30);
    let goal = || {
        project
            .command()
            .args(["-f", "project.mk", "lib.a(member.o)"])
            .output()
            .unwrap()
    };
    expect_stdout(goal(), "");
    assert_eq!(
        fs::read_to_string(project.0.join("names")).unwrap(),
        "lib.a|member.o\n"
    );

    let mut corrupt = member_archive(20);
    corrupt.replace_range(24..36, "invalid     ");
    project.fixture("lib.a", &corrupt, 0);
    let output = project.build();
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("project.mk:1:"), "{stderr}");
    assert!(stderr.contains("lib.a(member.o)"), "{stderr}");
    assert!(stderr.contains("invalid archive member timestamp"), "{stderr}");
}

// Common ar header from Solaris ar(3HEAD), with a fixed date and two data bytes.
fn member_archive(seconds: u64) -> String {
    let date = 1_700_000_000 + seconds;
    format!("!<arch>\n{:<16}{date:<12}{:<6}{:<6}{:<8}{:<10}`\nxx", "member.o/", 0, 0, 644, 2)
}

// Each append is one short record. Files gate the recipes; the clock only
// sets the deadline for the handshake.
const RECORD_JOB: &str = r#"
set -eu
printf '+ %s\n' "$1" >> jobs.log
: > "$1.entered"
deadline=$(($(date +%s) + 5))
for marker in "$@"; do
    while [ ! -f "$marker.entered" ]; do
        [ "$(date +%s)" -lt "$deadline" ] || exit 44
    done
done
printf -- '- %s\n' "$1" >> jobs.log
: > "$1.done"
"#;

fn check_jobs(project: &Project, jobs: &[&str], expected_peak: usize) {
    use std::collections::HashSet;

    let log = fs::read_to_string(project.0.join("jobs.log")).unwrap();
    let mut active = HashSet::new();
    let mut seen = HashSet::new();
    let mut peak = 0;
    for event in log.lines() {
        let (kind, job) = event.split_once(' ').unwrap();
        match kind {
            "+" => {
                assert!(seen.insert(job) && active.insert(job), "{log}");
                peak = peak.max(active.len());
            }
            "-" => assert!(active.remove(job), "{log}"),
            _ => panic!("invalid job event {event}"),
        }
    }
    assert_eq!(seen, jobs.iter().copied().collect(), "{log}");
    assert!(active.is_empty(), "{log}");
    assert_eq!(peak, expected_peak, "{log}");
}

#[test]
fn j_never_exceeds_tokens_across_submake() {
    let project = Project::new(concat!(
        "all: outer recurse\n",
        "outer:\n\t@/bin/sh record outer nested last\n",
        "recurse:\n\t@$(MAKE) -f child.mk\n",
    ));
    project.fixture("record", RECORD_JOB, 0);
    project.fixture(
        "child.mk",
        concat!(
            "all: nested more last\n",
            "nested:\n\t@/bin/sh record nested outer\n",
            "more:\n\t@/bin/sh record more\n",
            "last:\n\t@/bin/sh record last\n",
        ),
        0,
    );
    expect_stdout(
        project
            .command()
            .args(["-j", "2", "-f", "project.mk"])
            .output()
            .unwrap(),
        "",
    );
    check_jobs(&project, &["outer", "nested", "more", "last"], 2);
}

#[test]
fn wait_orders_prerequisites() {
    let project = Project::new(concat!(
        ".WAIT:\n",
        "all: a b .WAIT right\n",
        "a:\n\t@/bin/sh record a b\n",
        "b:\n\t@/bin/sh record b a\n",
        "right: right-input\n",
        "right-input:\n\t@test -f a.done && test -f b.done && touch right-input\n",
    ));
    project.fixture("record", RECORD_JOB, 0);
    expect_stdout(
        project
            .command()
            .args(["-j4", "-f", "project.mk"])
            .output()
            .unwrap(),
        "",
    );
    assert!(project.0.join("right-input").is_file());
    check_jobs(&project, &["a", "b"], 2);

    fs::remove_file(project.0.join("a.done")).unwrap();
    fs::remove_file(project.0.join("b.done")).unwrap();
    fs::remove_file(project.0.join("jobs.log")).unwrap();
    // Asking for the right-hand target alone does not select the left-hand group.
    expect_stdout(
        project
            .command()
            .args(["-j4", "-f", "project.mk", "right"])
            .output()
            .unwrap(),
        "",
    );
    assert!(!project.0.join("jobs.log").exists());
}

#[test]
fn notparallel_serializes() {
    let project = Project::new(concat!(
        ".NOTPARALLEL:\n",
        "all: first second recurse\n",
        "first:\n\t@/bin/sh record first\n",
        "second:\n\t@test -f first.done && /bin/sh record second\n",
        "recurse:\n\t@test -f second.done && $(MAKE) -f child.mk\n",
    ));
    project.fixture("record", RECORD_JOB, 0);
    project.fixture(
        "child.mk",
        concat!(
            "all: child-a child-b\n",
            "child-a:\n\t@/bin/sh record child-a child-b\n",
            "child-b:\n\t@/bin/sh record child-b child-a\n",
        ),
        0,
    );
    expect_stdout(
        project
            .command()
            .args(["-j2", "-f", "project.mk"])
            .output()
            .unwrap(),
        "",
    );
    check_jobs(&project, &["first", "second", "child-a", "child-b"], 2);
}

#[test]
fn signal_removes_partial_target() {
    let project = Project::new(concat!(
        ".PRECIOUS: keep\n",
        "partial keep:\n\t@exec /bin/sh interrupt '$@'\n",
    ));
    project.fixture(
        "interrupt",
        "printf partial > \"$1\"\nkill -TERM \"$PPID\"\n",
        0,
    );

    for (target, removed) in [("partial", true), ("keep", false)] {
        let output = project
            .command()
            .args(["-f", "project.mk", target])
            .output()
            .unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(!output.status.success(), "{target}: {stderr}");
        assert!(stderr.contains("signal 15"), "{target}: {stderr}");
        assert_eq!(
            stderr.contains(&format!("removed interrupted target '{target}'")),
            removed,
            "{stderr}",
        );
        assert_eq!(project.0.join(target).exists(), !removed);
        if !removed {
            assert_eq!(
                fs::read_to_string(project.0.join(target)).unwrap(),
                "partial"
            );
        }
    }
}

#[test]
fn managed_group_contains_recursive_recipes() {
    let project = Project::new("all:\n\t@ps -o pgid= -p $$$$\n\t@$(MAKE) -f child.mk\n");
    project.fixture("child.mk", "all:\n\t@ps -o pgid= -p $$$$\n", 0);
    let child = project.command()
        .args(["-j2", "-f", "project.mk"])
        .env("BMAKE_INHERIT_PROCESS_GROUP", "1")
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn().unwrap();
    let group = child.id().to_string();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.split_whitespace().collect::<Vec<_>>(), [group.as_str(); 2]);
}
