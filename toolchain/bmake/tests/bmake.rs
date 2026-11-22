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
        Command::new(env!("CARGO_BIN_EXE_bmake"))
            .arg("project.mk")
            .current_dir(&self.0)
            .output()
            .unwrap()
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
