use std::{
    fs::{self, File},
    io::ErrorKind,
    path::PathBuf,
    process::{self, Command, Output},
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
        ("environment", &["BMAKE_TEST_ORIGIN=command"][..], "<command>\n"),
        ("environment", &["-e", "BMAKE_TEST_ORIGIN=command"][..], "<command>\n"),
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
        "all.obj: older equal newer sub/item.obj lib.a(member.o)\n",
        "\t@printf 'explicit %s|%s|%s|%s\\n' '$@' '$?' '$*' '$(?F)'\n",
        "lib.a(member.o):\n",
        "\t@printf 'archive %s|%s\\n' '$@' '$%'\n",
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
            "archive lib.a|member.o\n",
            "explicit all.obj|newer sub/item.obj lib.a(member.o)|all|newer item.obj lib.a(member.o)\n",
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
        let project = Project::new(&format!(
            "{definitions}all:\n\t@printf '%s' '$(A)'\n"
        ));
        let line = definitions.lines().count();
        project.expect_error(&format!("project.mk:{line}:"), cycle);
    }
}
