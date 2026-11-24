//! Check the sealed reference inputs and the native assembler, linker, C compiler and make.

use breenix_suite::suite::{case, case_diagnostic, case_ms_left, category, running_case, suite, CaseError, CaseResult, Suite};
use libbreenix::crypto::rand::getrandom_bytes;
use libbreenix::crypto::sha256::{sha256, Sha256};
use libbreenix::{io, signal, types::Fd};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

#[path = "fixture/ssh_lease.rs"]
mod ssh_lease;

#[path = "suite_support/busybox_record.rs"]
mod busybox_record;

const DEV: &str = "opt/breenix-dev/25.2";
const CORPUS: &str = "usr/share/breenix/toolchain";
const RUNNER: &str = "sbin/suite-toolchain";
const MANIFEST: &str = "opt/breenix-dev/25.2/manifest.sha256";
const RECORD: &str = "opt/breenix-dev/25.2/build.json";
const LIBC_REFERENCE_DIR: &str = "usr/libexec/breenix";
const LIBC_PREREQUISITES: [&str; 2] = ["abi_check.o", "harness.o"];
static LIBC_NONCE: OnceLock<[u8; 16]> = OnceLock::new();
#[cfg(target_arch = "x86_64")]
const BAS_PROGRAMS: &[&str] = &["sse-exit", "x87-exit"];
#[cfg(target_arch = "aarch64")]
const BAS_PROGRAMS: &[&str] = &["fp-exit", "call-exit"];
const BAS_INPUTS: &[&str] = &["encodings.s", "encodings.hex", "bad-input.s"];
const LINKER_OBJECTS: &[&str] = &[
    "got-main",
    "got-entry",
    "archive-value-with-long-name",
    "tls-template",
];
const ARCHIVE_MEMBERS: &[&str] = &["got-entry", "archive-value-with-long-name"];
const CASES: &str = include_str!("../../../toolchain/corpus/cases");
#[cfg(target_arch = "x86_64")]
const CC_CASES: &str = include_str!("fixture/toolchain-cc-cases"); // 25.9d stages every x86 C11 case.
#[cfg(target_arch = "aarch64")]
const CC_CASES: &str = include_str!("fixture/toolchain-cc-cases-aarch64");

static SUITE: Suite = suite(
    "toolchain",
    "Native C toolchain",
    &[
        category(
            "inputs",
            "The sysroot inventory and the cross-built reference corpus",
            &[
                case(
                    "sysroot-manifest",
                    "Staged toolchain inputs match this image's build record",
                    sysroot_manifest,
                ),
                case(
                    "reference-corpus-runs",
                    "Each reference ELF produces its expected stdout and exit status",
                    reference_corpus_runs,
                ),
            ],
        ),
        category(
            "assembler",
            "Instruction encodings and ELF objects from our assembler",
            &[
                case(
                    "encodings-match",
                    "Assembler output matches the instruction encoding table",
                    assembler_encodings_match,
                ),
                case(
                    "object-runs",
                    "Fresh objects match the recorded link inputs and both staged ELFs run",
                    assembler_object_runs,
                ),
                case(
                    "bad-input-located",
                    "Malformed assembly fails with its source file and line number",
                    assembler_bad_input_located,
                ),
            ],
        ),
        category(
            "linker",
            "Static links of our libc archive: GOT, archives, linker script and TLS",
            &[
                case(
                    "links-reference-objects",
                    "On-box links of the reference objects match their expectations",
                    links_reference_objects,
                ),
                case(
                    "got-and-archives",
                    "A fresh link resolves the GOT and pulls dependent archive members",
                    got_and_archives,
                ),
                case(
                    "tls-template",
                    "A fresh link initializes aligned TLS in the main thread and two new threads",
                    tls_template,
                ),
                case(
                    "archive-tools",
                    "Deterministic archives expose symbols and supply a runnable link",
                    archive_tools,
                ),
                case(
                    "undefined-located",
                    "Undefined references identify their relocation before a repaired link runs",
                    undefined_located,
                ),
                case(
                    "duplicate-located",
                    "Duplicate definitions identify both objects before a repaired link runs",
                    duplicate_located,
                ),
            ],
        ),
        category(
            "c-core",
            "Preprocessor, parser, semantics and code generation for hello and its diagnostics",
            &[
                case(
                    "hello",
                    "The on-box C compiler builds and runs hello",
                    cc_hello,
                ),
                case(
                    "hello-over-ssh",
                    "The leased SSH account builds and runs hello with the staged cc",
                    cc_hello_over_ssh,
                ),
                case(
                    "corpus-three-producers",
                    "Clang, host bcc and guest cc each produce the expected corpus results",
                    cc_corpus_three_producers,
                ),
                case(
                    "stages-agree",
                    "Separate compiler stages agree byte for byte with the driver",
                    cc_stages_agree,
                ),
                case(
                    "diagnostic-located",
                    "Invalid C fails with a source file, line and column",
                    cc_diagnostic_located,
                ),
            ],
        ),
        category(
            "make",
            "POSIX make: graphs, timestamps, inference, macros and exit status",
            &[
                case(
                    "graph-order",
                    "Prerequisites run before their dependents and a shared prerequisite runs once",
                    make_graph_order,
                ),
                case(
                    "noop-when-current",
                    "An equal-time prerequisite leaves the target unchanged and a newer one rebuilds it",
                    make_noop_when_current,
                ),
                case(
                    "suffix-inference",
                    "A suffix rule builds the requested file with the inferred source and stem",
                    make_suffix_inference,
                ),
                case(
                    "macro-precedence",
                    "Makefile, environment and command-line macros follow POSIX precedence",
                    make_macro_precedence,
                ),
                case(
                    "failure-status",
                    "A failing shell recipe reports its status and stops the build",
                    make_failure_status,
                ),
                case("archive-members", "Make builds and names an archive member", make_archive_members),
                case("include-remake", "Make remakes and rereads a missing include", make_include_remake),
                case("recursive-makeflags", "Submake inherits command options and macros", make_recursive_makeflags),
            ],
        ),
        category(
            "project",
            "A multi-file C and assembly project: build, no-op and header-triggered rebuild",
            &[
                case(
                    "builds-and-runs",
                    "Make builds and runs the C and assembly project from clean inputs",
                    project_builds_and_runs,
                ),
                case(
                    "noop-rebuild",
                    "A second make runs no recipe and preserves output bytes and mtimes",
                    project_noop_rebuild,
                ),
                case(
                    "header-rebuild",
                    "A newer header rebuilds only its dependent objects and the executable",
                    project_header_rebuild,
                ),
            ],
        ),
        category(
            "bootstrap",
            "C and Rust in one program",
            &[
                case("c-calls-rust", "C calls Rust with scalars, an aggregate and a callback", c_calls_rust),
                case("rust-calls-c", "Rust calls C with scalars, an aggregate and a callback", rust_calls_c),
            ],
        ),
        category(
            "abi",
            "System V x86-64 calling convention: aggregates, varargs, SSE and x87",
            &[
                case("aggregates-exhaustion", "Aggregate arguments and returns survive register exhaustion in both directions", aggregates_exhaustion),
                case("varargs", "Variadic calls pass arguments correctly in both directions", varargs),
                case("sse-float", "SSE floating-point arguments and returns work in both directions", sse_float),
                case("x87-long-double", "x87 long double arguments and returns work in both directions", x87_long_double),
            ],
        ),
        category(
            "tls",
            "Thread-local storage and per-thread errno",
            &[
                case("template-init-zero-fill-aligned", "TLS initializes aligned templates and zero-filled storage per thread", template_init_zero_fill_aligned),
                case("errno-per-thread", "Errno has independent storage in each thread", errno_per_thread),
                case("tls-across-fork-exec", "TLS survives fork and is reinitialized on exec", tls_across_fork_exec),
            ],
        ),
        category(
            "c11",
            "The declared C11 feature profile and its diagnostics",
            &[
                case(
                    "declarations-initializers",
                    "Designated initialization, compound literals and flexible array members",
                    c11_declarations_initializers,
                ),
                case(
                    "generic-static-assert-alignment",
                    "Generic selection, static assertions and aligned anonymous members",
                    c11_generic_static_assert_alignment,
                ),
                case(
                    "floating",
                    "Rounding modes affect arithmetic and conversions and raise the expected flags",
                    c11_floating,
                ),
                case(
                    "atomics",
                    "Threads exercise release/acquire, sequential consistency and aggregate CAS",
                    c11_atomics,
                ),
                case(
                    "constraint-diagnostics",
                    "The guest constraint subset fails with the expected located diagnostics",
                    c11_constraint_diagnostics,
                ),
                case(
                    "profile-macros",
                    "The compiler advertises its implemented C11 feature profile",
                    c11_profile_macros,
                ),
            ],
        ),
        category(
            "c17",
            "C17 corrections and the small optimization pipeline",
            &[
                case("corrections", "C17 corrections at O0 and O2 match their independent expectation", c17_program),
                case("o2-expect", "Optimized and unoptimized output match the independent expectation", c17_program),
                case("rounding-math", "Both optimization levels honor rounding modes and match the independent expectation", c17_program),
            ],
        ),
        category(
            "libc-suite",
            "The libc suite's C sources compiled on the box, compared case by case",
            &[
                case("dirent-glob", "Build dirent-glob and compare each result with the reference", libc_category),
                case("getopt-env", "Build getopt-env and compare each result with the reference", libc_category),
                case("locale-wchar", "Build locale-wchar and compare each result with the reference", libc_category),
                case("math", "Build math and compare each result with the reference", libc_category),
                case("pthread", "Build pthread and compare each result with the reference", libc_category),
                case("stdio", "Build stdio and compare each result with the reference", libc_category),
                case("string-stdlib", "Build string-stdlib and compare each result with the reference", libc_category),
            ],
        ),
        category(
            "busybox",
            "BusyBox rebuilt offline on the box with its required applets",
            &[
                case("unpack", "Verify and unpack the pinned BusyBox archive", busybox_phase),
                case("patch", "Apply the pinned patch series", busybox_phase),
                case("generators", "Build and run the native Kbuild configuration programs", busybox_phase),
                case("configure", "Apply the config and generate the applet build inputs", busybox_phase),
                case("inventory", "Record the pinned Kbuild object goals in recipe order", busybox_phase),
                case("build-libbb-1", "Build libbb object group 1", busybox_phase),
                case("build-libbb-2", "Build libbb object group 2", busybox_phase),
                case("build-libbb-3", "Build libbb object group 3", busybox_phase),
                case("build-libbb-4", "Build libbb object group 4", busybox_phase),
                case("build-applets-1", "Build applet object group 1", busybox_phase),
                case("build-applets-2", "Build applet object group 2", busybox_phase),
                case("build-applets-3", "Build applet object group 3", busybox_phase),
                case("build-applets-4", "Build applet object group 4", busybox_phase),
                case("link-and-list", "Link BusyBox, compare its applets and publish its paths", busybox_link_and_list),
                case("required-usage", "Run required applets through the newly linked BusyBox", busybox_required_usage),
            ],
        ),
        category(
            "gnu-workloads",
            "The GNU C and make extensions BusyBox's build needs",
            &[
                case("gnu-c-extensions", "Build and run a program using GNU C extensions", gnu_c_extensions),
                case("gnu-make-features", "Build and run recipes using GNU make extensions", gnu_make_features),
                case("kbuild-host-programs", "Run the newly built Kbuild host generators", kbuild_host_programs),
            ],
        ),
        // A6 supplies the ARM64 kernel assembly cases.
        #[cfg(target_arch = "x86_64")]
        category(
            "kernel-asm",
            "The kernel's assembly dialect through our assembler",
            &[
                case(
                    "four-objects-match-record",
                    "Native bas reproduces the four recorded kernel assembly objects",
                    four_objects_match_record,
                ),
                case(
                    "breakpoint-roundtrip",
                    "A user breakpoint delivers SIGTRAP and resumes after INT3",
                    breakpoint_roundtrip,
                ),
                case(
                    "privileged-encodings",
                    "Native bas matches the SDM bytes for the kernel instruction corpus",
                    privileged_encodings,
                ),
            ],
        ),
    ],
);

fn main() {
    let mut nonce = [0; 16];
    if getrandom_bytes(&mut nonce) == Ok(nonce.len()) {
        let _ = LIBC_NONCE.set(nonce);
    }
    // Sequence reset records foreign mounts before the first case and
    // preserves their contents when exec hands off to libc and utilities.
    if busybox_fixture().join("busybox.sh").is_file() {
        let _ = BUSYBOX_SCRATCH.set(busybox_mount());
    }
    SUITE.run()
}

fn io_at<T>(path: &Path, result: std::io::Result<T>) -> Result<T, String> {
    result.map_err(|error| format!("{}: {error}", path.display()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn lower_hex(text: &str, len: usize) -> bool {
    text.len() == len
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn file_sha256(path: &Path) -> Result<String, String> {
    let mut file = io_at(path, File::open(path))?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 16384];
    loop {
        let count = io_at(path, file.read(&mut buffer))?;
        if count == 0 {
            return Ok(hex(&digest.finalize()));
        }
        digest.update(&buffer[..count]);
    }
}

fn tree_files(path: &Path, files: &mut BTreeSet<String>) -> CaseResult {
    let kind = io_at(path, fs::symlink_metadata(path))?.file_type();
    if kind.is_dir() {
        for entry in io_at(path, fs::read_dir(path))? {
            tree_files(&io_at(path, entry)?.path(), files)?;
        }
    } else if kind.is_file() {
        let name = path
            .to_str()
            .and_then(|name| name.strip_prefix('/'))
            .ok_or_else(|| format!("invalid fixture path {}", path.display()))?;
        if name != MANIFEST && name != RECORD {
            files.insert(name.to_owned());
        }
    } else {
        return Err(format!("unexpected fixture file type at {}", path.display()).into());
    }
    Ok(())
}

fn fixture_path(name: &str) -> bool {
    let path = Path::new(name);
    path.components()
        .all(|component| matches!(component, Component::Normal(_)))
        && (name == RUNNER
            || (path != Path::new(DEV) && path.starts_with(DEV))
            || (path != Path::new(CORPUS) && path.starts_with(CORPUS))
            || libc_cases().iter().any(|case| libc_reference(case.id) == path))
        && name != MANIFEST
        && name != RECORD
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'+' | b'-')
        })
}

fn sysroot_manifest() -> CaseResult {
    let root = Path::new("/");
    let manifest_path = root.join(MANIFEST);
    let manifest = io_at(&manifest_path, fs::read_to_string(&manifest_path))?;
    let record_path = root.join(RECORD);
    let record: Value = serde_json::from_slice(&io_at(&record_path, fs::read(&record_path))?)
        .map_err(|error| format!("invalid toolchain image build record: {error}"))?;
    let commit = record["commit"]
        .as_str()
        .filter(|commit| lower_hex(commit, 40))
        .ok_or("missing toolchain image build commit")?;
    let arch = std::env::consts::ARCH;
    let hash = hex(&sha256(manifest.as_bytes()));
    if record["arch"] != arch || record["manifestSha256"] != hash {
        return Err("manifest digest or architecture differs from the image build record".into());
    }

    let mut listed = BTreeMap::new();
    for line in manifest.lines() {
        let (digest, name) = line.split_once("  ").ok_or("invalid manifest row")?;
        if !lower_hex(digest, 64) || !fixture_path(name) || listed.insert(name, digest).is_some() {
            return Err(format!("invalid or duplicate manifest path {name}").into());
        }
    }
    // This catalog is compiled into PID 1 from the independent source list.
    // Dropping a C file, expectation, object, executable or inventory from both the
    // staged tree and its generated manifest still fails here.
    for name in CASES.lines() {
        for (prefix, suffix) in [
            ("", ".c"),
            ("", ".expect"),
            ("", ".elf"),
            ("", ".inventory.json"),
            ("objs/", ".o"),
        ] {
            let path = format!("{CORPUS}/{prefix}{name}{suffix}");
            if !listed.contains_key(path.as_str()) {
                return Err(format!("manifest omits {path}").into());
            }
        }
    }
    for relative in [
        "lib/libc.a",
        "linker.ld",
        "bin/bas",
        "bin/bld",
        "bin/bar",
        "bin/bnm",
    ] {
        let path = format!("{DEV}/{relative}");
        if !listed.contains_key(path.as_str()) {
            return Err(format!("manifest omits {path}").into());
        }
    }

    if arch == "x86_64" {
        for name in LINKER_OBJECTS {
            let path = format!("{CORPUS}/objs/{name}.o");
            if !listed.contains_key(path.as_str()) {
                return Err(format!("manifest omits {path}").into());
            }
        }
        for name in ["got-main", "tls-template"] {
            let path = format!("{CORPUS}/linker/{name}.expect");
            if !listed.contains_key(path.as_str()) {
                return Err(format!("manifest omits {path}").into());
            }
        }
    }
    {
        for name in BAS_INPUTS {
            let path = format!("{CORPUS}/assembler/{name}");
            if !listed.contains_key(path.as_str()) {
                return Err(format!("manifest omits {path}").into());
            }
        }
        for name in BAS_PROGRAMS {
            for suffix in [".s", ".expect", ".elf", ".link.json"] {
                let path = format!("{CORPUS}/assembler/{name}{suffix}");
                if !listed.contains_key(path.as_str()) {
                    return Err(format!("manifest omits {path}").into());
                }
            }
        }
    }

    let mut staged = BTreeSet::new();
    for path in [DEV, CORPUS, RUNNER] {
        tree_files(&root.join(path), &mut staged)?;
    }
    for case in libc_cases() {
        tree_files(&root.join(libc_reference(case.id)), &mut staged)?;
    }
    let declared: BTreeSet<_> = listed.keys().copied().collect();
    let present: BTreeSet<_> = staged.iter().map(String::as_str).collect();
    if let Some(path) = declared.symmetric_difference(&present).next() {
        return Err(format!("manifest and staged paths differ at {path}").into());
    }
    for (path, expected) in listed {
        if file_sha256(&root.join(path))? != expected {
            return Err(format!("SHA-256 differs for {path}").into());
        }
    }
    case_diagnostic(&format!(
        "TOOLCHAIN SYSROOT commit={commit} arch={arch} sha256={hash}"
    ));
    Ok(())
}

fn expected_output(expect: &Path) -> Result<(i32, Vec<u8>), String> {
    let bytes = io_at(expect, fs::read(expect))?;
    let newline = bytes
        .iter()
        .position(|&byte| byte == b'\n')
        .ok_or_else(|| format!("{}: missing expected exit status", expect.display()))?;
    let status = std::str::from_utf8(&bytes[..newline])
        .ok()
        .and_then(|status| status.parse::<u8>().ok())
        .ok_or_else(|| format!("{}: invalid expected exit status", expect.display()))?;
    Ok((i32::from(status), bytes[newline + 1..].to_vec()))
}

fn run_expected(binary: &Path, expect: &Path) -> CaseResult {
    let name = binary.display();
    let (status, expected) = expected_output(expect)?;
    let result = io_at(
        binary,
        Command::new(binary)
            .env_clear()
            .stdin(Stdio::null())
            .output(),
    )?;
    if result.status.code() != Some(status) {
        return Err(format!("{name}: expected exit {status}, got {}", result.status).into());
    }
    if result.stdout != expected {
        return Err(format!(
            "{name}: stdout differs (expected {:?}, got {:?})",
            expected, result.stdout
        )
        .into());
    }
    Ok(())
}

fn reference_corpus_runs() -> CaseResult {
    let corpus = Path::new("/").join(CORPUS);
    for name in CASES.lines() {
        run_expected(
            &corpus.join(format!("{name}.elf")),
            &corpus.join(format!("{name}.expect")),
        )?;
    }
    Ok(())
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(category: &str) -> Result<Self, String> {
        let path = Path::new("/tmp").join(format!("toolchain-{category}-{}", std::process::id()));
        Self::create(path)
    }

    fn create(path: PathBuf) -> Result<Self, String> {
        io_at(&path, fs::DirBuilder::new().mode(0o700).create(&path))?;
        Ok(Self(path))
    }

    fn join(&self, name: impl AsRef<Path>) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn c_core() -> Result<PathBuf, String> {
    Ok(Path::new("/").join(CORPUS).join("c-core"))
}

fn bundle_command(scratch: &Scratch, tool: &str) -> Command {
    let bin = Path::new("/").join(DEV).join("bin");
    let mut command = Command::new(bin.join(tool));
    command
        .current_dir(&scratch.0)
        .env_clear()
        .env("PATH", format!("{}:/bin", bin.display()))
        // The suite supervises the process group that owns the build.
        .env("BMAKE_INHERIT_PROCESS_GROUP", "1")
        .stdin(Stdio::null());
    command
}

fn command_output(command: &mut Command) -> Result<std::process::Output, String> {
    command
        .output()
        .map_err(|error| format!("{}: {error}", command.get_program().to_string_lossy()))
}

fn cc_stage(scratch: &Scratch, source: &Path, args: &[&str]) -> CaseResult {
    let output = command_output(bundle_command(scratch, "cc").args(args).arg(source))?;
    require_success(
        &format!("cc {} {}", args.join(" "), source.display()),
        output,
    )?;
    Ok(())
}

fn cc_hello() -> CaseResult {
    let fixtures = c_core()?;
    let scratch = Scratch::new("hello")?;
    cc_stage(&scratch, &fixtures.join("hello.c"), &["-o", "hello"])?;
    run_expected(&scratch.join("hello"), &fixtures.join("hello.expect"))
}

fn cc_hello_over_ssh() -> CaseResult {
    let fixtures = c_core()?;
    let (status, stdout) = expected_output(&fixtures.join("hello.expect"))?;
    ssh_lease::with_listener(
        "SUITE toolchain HOSTSSH c-core/hello-over-ssh",
        "TOOLCHAIN LEASE-END",
        |account, nonce| {
            let directory = Path::new(&account.home).join(format!(".toolchain-{nonce}"));
            let binary = directory.join("hello");
            let receipt = directory.join("result.json");
            let record: Value = serde_json::from_slice(&io_at(&receipt, fs::read(&receipt))?)
                .map_err(|error| format!("invalid toolchain SSH result: {error}"))?;
            if record["nonce"] != nonce || record["uid"] != account.uid {
                return Err("toolchain SSH result has the wrong nonce or fixture uid".into());
            }
            if record["path"].as_str() != binary.to_str()
                || record["sha256"] != file_sha256(&binary)?
            {
                return Err("toolchain SSH binary differs from the recorded path or digest".into());
            }
            if record["status"] != status || record["stdoutHex"] != hex(&stdout) {
                return Err("toolchain SSH program does not match hello.expect".into());
            }
            Ok(())
        },
    )
}

fn cc_corpus_three_producers() -> CaseResult {
    let fixtures = c_core()?;
    let scratch = Scratch::new("c-core")?;
    let corpus = Path::new("/").join(CORPUS);
    for name in CC_CASES.lines() {
        let expected = corpus.join(format!("{name}.expect"));
        let source = corpus.join(format!("{name}.c"));
        let elf = format!("{name}.elf");
        run_expected(&corpus.join(&elf), &expected)?;
        run_expected(&fixtures.join("mac").join(&elf), &expected)?;
        cc_stage(&scratch, &source, &["-o", &elf])?;
        let binary = scratch.join(&elf);
        run_expected(&binary, &expected)?;
        io_at(&binary, fs::remove_file(&binary))?;
    }
    Ok(())
}

fn equal_stage(scratch: &Scratch, direct: &str, explicit: &str) -> CaseResult {
    let read = |name| {
        let path = scratch.join(name);
        io_at(&path, fs::read(&path))
    };
    if read(direct)? != read(explicit)? {
        return Err(format!("compiler stages differ at {direct} and {explicit}").into());
    }
    Ok(())
}

fn cc_stages_agree() -> CaseResult {
    let fixtures = c_core()?;
    let scratch = Scratch::new("cc-stages")?;
    let source = fixtures.join("hello.c");
    cc_stage(&scratch, &source, &["-E", "-o", "hello.i"])?;
    cc_stage(&scratch, &source, &["-S", "-o", "direct.s"])?;
    cc_stage(&scratch, Path::new("hello.i"), &["-S", "-o", "split.s"])?;
    equal_stage(&scratch, "direct.s", "split.s")?;

    cc_stage(&scratch, &source, &["-c", "-o", "direct.o"])?;
    require_success(
        "as split.s",
        command_output(bundle_command(&scratch, "as").args(["-o", "split.o", "split.s"]))?,
    )?;
    equal_stage(&scratch, "direct.o", "split.o")?;

    let dev = Path::new("/").join(DEV);
    require_success(
        "ld split.o",
        command_output(
            bundle_command(&scratch, "ld")
                .arg("-T")
                .arg(dev.join("linker.ld"))
                .args(["--gc-sections", "-o", "split", "split.o"])
                .arg(dev.join("lib/libc.a")),
        )?,
    )?;
    cc_stage(&scratch, &source, &["-o", "direct"])?;
    equal_stage(&scratch, "direct", "split")?;
    run_expected(&scratch.join("split"), &fixtures.join("hello.expect"))
}

fn cc_diagnostic_located() -> CaseResult {
    let source = c_core()?.join("diagnostic.c");
    let scratch = Scratch::new("cc-diagnostic")?;
    let output = command_output(
        bundle_command(&scratch, "cc")
            .args(["-o", "unexpected"])
            .arg(&source),
    )?;
    let expected = format!(
        "{}:4:12: error: undeclared identifier 'undeclared'",
        source.display()
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.code() != Some(1)
        || scratch.join("unexpected").exists()
        || !stderr.lines().any(|line| line == expected)
    {
        return Err(
            format!("cc did not fail with the expected diagnostic at 4:12: {stderr}").into(),
        );
    }
    Ok(())
}

fn make_corpus() -> PathBuf {
    Path::new("/").join(CORPUS).join("make")
}

fn make_fixture(name: &str) -> Result<Scratch, String> {
    let scratch = Scratch::new(name)?;
    let source = make_corpus().join(format!("{name}.mk"));
    io_at(&source, fs::copy(&source, scratch.join("Makefile")))?;
    Ok(scratch)
}

fn make_graph_order() -> CaseResult {
    let scratch = make_fixture("graph-order")?;
    let stdout = require_success(
        "make",
        command_output(&mut bundle_command(&scratch, "make"))?,
    )?;
    if stdout != b"shared\nleft\nright\nall\n" {
        return Err(format!(
            "unexpected recipe order: {:?}",
            String::from_utf8_lossy(&stdout)
        )
        .into());
    }
    Ok(())
}

fn touch_make_files(scratch: &Scratch, date: &str, files: &[&str]) -> CaseResult {
    let output = command_output(
        Command::new("/bin/touch")
            .current_dir(&scratch.0)
            .env_clear()
            .env("TZ", "UTC0")
            .args(["-d", date])
            .args(files),
    )?;
    require_success(&format!("touch -d {date} {}", files.join(" ")), output)?;
    Ok(())
}

fn make_noop_when_current() -> CaseResult {
    let scratch = make_fixture("noop-when-current")?;
    let input = scratch.join("input");
    let output = scratch.join("output");
    io_at(&input, fs::write(&input, b"rebuilt\n"))?;
    io_at(&output, fs::write(&output, b"current\n"))?;

    touch_make_files(&scratch, "2000-01-01 00:00:00", &["input", "output"])?;
    let mtime = |path: &Path| io_at(path, fs::metadata(path).and_then(|meta| meta.modified()));
    let current = mtime(&output)?;
    if mtime(&input)? != current {
        return Err("touch -d did not give input and output equal mtimes".into());
    }
    require_success(
        "make",
        command_output(&mut bundle_command(&scratch, "make"))?,
    )?;
    if mtime(&output)? != current || io_at(&output, fs::read(&output))? != b"current\n" {
        return Err("make changed output with an equal-time prerequisite".into());
    }

    touch_make_files(&scratch, "2000-01-02 00:00:00", &["input"])?;
    if mtime(&input)? <= current {
        return Err("touch -d did not make input newer than output".into());
    }
    require_success(
        "make",
        command_output(&mut bundle_command(&scratch, "make"))?,
    )?;
    if io_at(&output, fs::read(&output))? != b"rebuilt\n" {
        return Err("make did not rebuild output from its newer prerequisite".into());
    }
    Ok(())
}

fn make_suffix_inference() -> CaseResult {
    let scratch = make_fixture("suffix-inference")?;
    let source = make_corpus().join("sample.src");
    io_at(&source, fs::copy(&source, scratch.join("sample.src")))?;
    require_success(
        "make sample.out",
        command_output(bundle_command(&scratch, "make").arg("sample.out"))?,
    )?;
    let output = scratch.join("sample.out");
    if io_at(&output, fs::read(&output))? != b"sample\nsource payload\n" {
        return Err("suffix rule wrote the wrong stem or source bytes to sample.out".into());
    }
    Ok(())
}

fn make_macro_precedence() -> CaseResult {
    let scratch = make_fixture("macro-precedence")?;
    for (args, expected) in [
        (&[][..], "makefile\n"),
        (&["-e"][..], "environment\n"),
        (&["-e", "VALUE=command-line"][..], "command-line\n"),
    ] {
        let label = format!("make {args:?}");
        let stdout = require_success(
            &label,
            command_output(
                bundle_command(&scratch, "make")
                    .env("VALUE", "environment")
                    .args(args),
            )?,
        )?;
        if stdout != expected.as_bytes() {
            return Err(format!(
                "{label}: expected {expected:?}, got {:?}",
                String::from_utf8_lossy(&stdout)
            )
            .into());
        }
    }
    Ok(())
}

fn make_failure_status() -> CaseResult {
    let scratch = make_fixture("failure-status")?;
    let output = command_output(&mut bundle_command(&scratch, "make"))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.code() != Some(2)
        || output.stdout != b"before failure\n"
        || !stderr.contains("target 'fail': recipe exited with status 7")
    {
        return Err(format!(
            "make returned {}; stdout {:?}; stderr {:?}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            stderr.trim_end()
        )
        .into());
    }
    Ok(())
}

const PROJECT_INPUTS: &[&str] = &[
    "Makefile",
    "main.c",
    "util.c",
    "util.h",
    "value.s",
    "project.expect",
];
const PROJECT_OUTPUTS: &[&str] = &["main.o", "util.o", "value.o", "project"];
const PROJECT_CC_MAIN: &str = "cc -c -o main.o main.c\n";
const PROJECT_CC_UTIL: &str = "cc -c -o util.o util.c\n";
const PROJECT_AS: &str = "as -o value.o value.s\n";
const PROJECT_LINK: &str = "cc -o project main.o util.o value.o\n";

fn project_make(scratch: &Scratch, recipes: &[&str]) -> CaseResult {
    let stdout = require_success(
        "make",
        command_output(&mut bundle_command(scratch, "make"))?,
    )?;
    let expected = recipes.concat();
    if stdout != expected.as_bytes() {
        return Err(format!(
            "make recipes: expected {expected:?}, got {:?}",
            String::from_utf8_lossy(&stdout)
        )
        .into());
    }
    Ok(())
}

fn project_clean_build(case: &str) -> Result<Scratch, CaseError> {
    let scratch = Scratch::new(case)?;
    let fixture = Path::new("/").join(CORPUS).join("project");
    for name in PROJECT_INPUTS {
        let source = fixture.join(name);
        io_at(&source, fs::copy(&source, scratch.join(name)))?;
    }
    project_make(
        &scratch,
        &[PROJECT_CC_MAIN, PROJECT_CC_UTIL, PROJECT_AS, PROJECT_LINK],
    )?;
    Ok(scratch)
}

fn project_snapshot(scratch: &Scratch, name: &str) -> Result<(Vec<u8>, SystemTime), String> {
    let path = scratch.join(name);
    let bytes = io_at(&path, fs::read(&path))?;
    let modified = io_at(&path, fs::metadata(&path).and_then(|meta| meta.modified()))?;
    Ok((bytes, modified))
}

fn project_builds_and_runs() -> CaseResult {
    let scratch = project_clean_build("builds-and-runs")?;
    run_expected(&scratch.join("project"), &scratch.join("project.expect"))
}

fn project_noop_rebuild() -> CaseResult {
    let scratch = project_clean_build("noop-rebuild")?;
    let before = PROJECT_OUTPUTS
        .iter()
        .map(|name| project_snapshot(&scratch, name))
        .collect::<Result<Vec<_>, _>>()?;
    project_make(&scratch, &[])?;
    for (name, snapshot) in PROJECT_OUTPUTS.iter().zip(before) {
        if project_snapshot(&scratch, name)? != snapshot {
            return Err(format!("no-op rebuild changed bytes or mtime of {name}").into());
        }
    }
    Ok(())
}

fn project_header_rebuild() -> CaseResult {
    let scratch = project_clean_build("header-rebuild")?;
    touch_make_files(&scratch, "2000-01-01 00:00:00", PROJECT_INPUTS)?;
    touch_make_files(&scratch, "2000-01-01 00:00:00", PROJECT_OUTPUTS)?;
    let before = PROJECT_OUTPUTS
        .iter()
        .map(|name| project_snapshot(&scratch, name))
        .collect::<Result<Vec<_>, _>>()?;
    touch_make_files(&scratch, "2000-01-02 00:00:00", &["util.h"])?;
    let (_, header_time) = project_snapshot(&scratch, "util.h")?;
    if before.iter().any(|(_, modified)| header_time <= *modified) {
        return Err("touch -d did not make util.h newer than the build outputs".into());
    }

    project_make(&scratch, &[PROJECT_CC_MAIN, PROJECT_CC_UTIL, PROJECT_LINK])?;
    for (name, snapshot) in PROJECT_OUTPUTS.iter().copied().zip(before) {
        let current = project_snapshot(&scratch, name)?;
        if name == "value.o" {
            if current != snapshot {
                return Err("header rebuild changed bytes or mtime of value.o".into());
            }
        } else if current.1 <= snapshot.1 {
            return Err(format!("header rebuild did not update {name}").into());
        }
    }
    Ok(())
}

struct AssemblyCase {
    corpus: PathBuf,
    scratch: Scratch,
}

impl AssemblyCase {
    fn start() -> Result<Self, String> {
        Self::start_at("assembler")
    }

    fn start_at(category: &str) -> Result<Self, String> {
        Ok(Self {
            corpus: Path::new("/").join(CORPUS).join(category),
            scratch: Scratch::new(category)?,
        })
    }

    fn run(&self, source: &Path, object: &Path) -> Result<std::process::Output, String> {
        let bas = Path::new("/").join(DEV).join("bin/bas");
        let mut command = Command::new(&bas);
        if std::env::consts::ARCH == "aarch64" {
            command.args(["--target", "aarch64"]);
        }
        io_at(
            &bas,
            command
                .arg("-o")
                .arg(object)
                .arg(source)
                .env_clear()
                .stdin(Stdio::null())
                .output(),
        )
    }

    fn assemble(&self, name: &str) -> Result<PathBuf, String> {
        let source = self.corpus.join(format!("{name}.s"));
        let object = self.scratch.join(format!("{name}.o"));
        let result = self.run(&source, &object)?;
        if !result.status.success() {
            return Err(format!(
                "{name}: bas exited {}: {}",
                result.status,
                String::from_utf8_lossy(&result.stderr).trim_end()
            ));
        }
        Ok(object)
    }
}

fn elf_field<const N: usize>(bytes: &[u8], offset: usize) -> Option<[u8; N]> {
    bytes.get(offset..offset.checked_add(N)?)?.try_into().ok()
}

// ELF gABI Listings 2.1 and 3.1. Read named section bytes independently of bas.
fn object_text(object: &[u8]) -> Option<&[u8]> {
    if !object.starts_with(b"\x7fELF\x02\x01\x01")
        || u16::from_le_bytes(elf_field(object, 16)?) != 1
        || u16::from_le_bytes(elf_field(object, 18)?)
            != if cfg!(target_arch = "aarch64") {
                183
            } else {
                62
            }
        || u16::from_le_bytes(elf_field(object, 58)?) != 64
    {
        return None;
    }
    let start = usize::try_from(u64::from_le_bytes(elf_field(object, 40)?)).ok()?;
    let count = usize::from(u16::from_le_bytes(elf_field(object, 60)?));
    let names = usize::from(u16::from_le_bytes(elf_field(object, 62)?));
    let table = object.get(start..start.checked_add(64 * count)?)?;
    let contents = |header: &[u8]| {
        let offset = usize::try_from(u64::from_le_bytes(elf_field(header, 24)?)).ok()?;
        let size = usize::try_from(u64::from_le_bytes(elf_field(header, 32)?)).ok()?;
        object.get(offset..offset.checked_add(size)?)
    };
    let strings = contents(table.chunks_exact(64).nth(names)?)?;
    let mut text = None;
    for header in table.chunks_exact(64) {
        let name = usize::try_from(u32::from_le_bytes(elf_field(header, 0)?)).ok()?;
        if strings.get(name..)?.starts_with(b".text\0")
            && (u32::from_le_bytes(elf_field(header, 4)?) != 1
                || text.replace(contents(header)?).is_some())
        {
            return None;
        }
    }
    text
}

fn x86_encoding_bytes(path: &Path, table: &str) -> Result<Vec<u8>, String> {
    table
        .split_ascii_whitespace()
        .map(|byte| u8::from_str_radix(byte, 16))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("{}: {error}", path.display()))
}

fn assembler_encodings_match() -> CaseResult {
    let case = AssemblyCase::start()?;
    let object = case.assemble("encodings")?;
    let bytes = io_at(&object, fs::read(&object))?;
    let actual = object_text(&bytes).ok_or("bas object has no valid .text section")?;
    let path = case.corpus.join("encodings.hex");
    let hex_table = io_at(&path, fs::read_to_string(&path))?;
    let expected = if cfg!(target_arch = "aarch64") {
        let mut bytes = Vec::new();
        for (index, row) in hex_table.lines().enumerate() {
            let word = row.split_once('#').map_or(row, |(word, _)| word).trim();
            if !lower_hex(word, 8) {
                return Err(format!(
                    "{}:{}: expected an eight-digit instruction word",
                    path.display(),
                    index + 1
                )
                .into());
            }
            let word = u32::from_str_radix(word, 16)
                .map_err(|error| format!("{}:{}: {error}", path.display(), index + 1))?;
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes
    } else {
        x86_encoding_bytes(&path, &hex_table)?
    };
    if actual != expected || expected.is_empty() {
        let spec = if cfg!(target_arch = "aarch64") {
            "Arm ARM"
        } else {
            "Intel SDM"
        };
        return Err(format!("bas .text differs from the {spec} encoding table").into());
    }
    Ok(())
}

fn assembler_object_runs() -> CaseResult {
    let case = AssemblyCase::start()?;
    for name in BAS_PROGRAMS {
        let object = case.assemble(name)?;
        let object_name = format!("{name}.o");
        let elf_name = format!("{name}.elf");
        let elf = case.corpus.join(&elf_name);
        let path = case.corpus.join(format!("{name}.link.json"));
        let receipt: Value = serde_json::from_slice(&io_at(&path, fs::read(&path))?)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let inputs = receipt["inputObjects"]
            .as_object()
            .filter(|inputs| inputs.len() == 1)
            .ok_or_else(|| format!("{name}: expected one object in link receipt"))?;
        let map = receipt["map"]
            .as_str()
            .ok_or_else(|| format!("{name}: missing lld map"))?;
        let map_input = format!("{object_name}:(.text)");
        if !map
            .lines()
            .any(|line| line.split_ascii_whitespace().last() == Some(map_input.as_str()))
        {
            return Err(format!("{name}: lld map does not name {object_name}").into());
        }
        if inputs.get(&object_name).and_then(Value::as_str) != Some(file_sha256(&object)?.as_str())
        {
            return Err(format!("{name}: fresh object differs from the recorded lld input").into());
        }
        if receipt["elf"]["name"] != elf_name || receipt["elf"]["sha256"] != file_sha256(&elf)? {
            return Err(format!("{name}: staged ELF differs from the link receipt").into());
        }
        run_expected(&elf, &case.corpus.join(format!("{name}.expect")))?;
    }
    Ok(())
}

fn assembler_bad_input_located() -> CaseResult {
    let case = AssemblyCase::start()?;
    let source = case.corpus.join("bad-input.s");
    let object = case.scratch.join("bad-input.o");
    let result = case.run(&source, &object)?;
    let location = format!("{}:2:", source.display());
    if !result.status.code().is_some_and(|code| code != 0)
        || !String::from_utf8_lossy(&result.stderr).starts_with(&location)
        || object.exists()
    {
        return Err(format!(
            "expected nonzero exit and {location} with no output object, got {}: {}",
            result.status,
            String::from_utf8_lossy(&result.stderr).trim_end()
        )
        .into());
    }
    Ok(())
}

#[cfg(target_arch = "x86_64")]
const LINKER_UNDEFINED_RELOCATION: &str = "R_X86_64_PLT32";
#[cfg(target_arch = "aarch64")]
const LINKER_UNDEFINED_RELOCATION: &str = "R_AARCH64_CALL26";
#[cfg(target_arch = "aarch64")]
const LINKER_BNM_SYMBOLS: &str = "{0}[got-entry.o]:\narchive_cell U\narchive_entry T 0 20\narchive_value U\n{0}[archive-value-with-long-name.o]:\narchive_cell R 0 4\narchive_value T 0 8\n";

struct LinkerCase {
    dev: PathBuf,
    corpus: PathBuf,
    scratch: Scratch,
}

impl LinkerCase {
    fn start() -> Result<Self, String> {
        Ok(Self {
            dev: Path::new("/").join(DEV),
            corpus: Path::new("/").join(CORPUS),
            scratch: Scratch::new("linker")?,
        })
    }

    fn object(&self, name: &str) -> PathBuf {
        self.corpus.join("objs").join(format!("{name}.o"))
    }

    fn tool(
        &self,
        tool: &str,
        args: impl IntoIterator<Item = impl AsRef<std::ffi::OsStr>>,
    ) -> Result<std::process::Output, String> {
        let binary = self.dev.join("bin").join(tool);
        io_at(
            &binary,
            Command::new(&binary)
                .args(args)
                .env_clear()
                .stdin(Stdio::null())
                .output(),
        )
    }

    fn link(&self, output: &Path, inputs: &[&Path]) -> Result<std::process::Output, String> {
        self.tool(
            "bld",
            [
                Path::new("-T"),
                &self.dev.join("linker.ld"),
                Path::new("--gc-sections"),
                Path::new("-o"),
                output,
            ]
            .into_iter()
            .chain(inputs.iter().copied())
            .chain([self.dev.join("lib/libc.a").as_path()]),
        )
    }

    fn link_and_run(&self, name: &str, expect: &Path, inputs: &[&Path]) -> CaseResult {
        let output = self.scratch.join(format!("{name}.elf"));
        require_success("bld", self.link(&output, inputs)?)?;
        run_expected(&output, expect)
    }

    fn make_archive(&self, name: &str) -> Result<PathBuf, String> {
        let archive = self.scratch.join(name);
        let members = ARCHIVE_MEMBERS.iter().map(|name| self.object(name));
        let args = [PathBuf::from("rcs"), archive.clone()]
            .into_iter()
            .chain(members);
        require_success("bar", self.tool("bar", args)?)?;
        Ok(archive)
    }

    fn run_got(&self, archive: &Path) -> CaseResult {
        self.link_and_run(
            "got-main",
            &self.corpus.join("linker/got-main.expect"),
            &[&self.object("got-main"), archive],
        )
    }
}

fn require_success(tool: &str, result: std::process::Output) -> Result<Vec<u8>, String> {
    if result.status.success() {
        Ok(result.stdout)
    } else {
        Err(format!(
            "{tool} exited {}: {}",
            result.status,
            String::from_utf8_lossy(&result.stderr).trim_end()
        ))
    }
}

fn links_reference_objects() -> CaseResult {
    let case = LinkerCase::start()?;
    for name in CASES.lines() {
        case.link_and_run(
            name,
            &case.corpus.join(format!("{name}.expect")),
            &[&case.object(name)],
        )?;
    }
    Ok(())
}

fn got_and_archives() -> CaseResult {
    let case = LinkerCase::start()?;
    let archive = case.make_archive("got.a")?;
    case.run_got(&archive)
}

fn tls_template() -> CaseResult {
    let case = LinkerCase::start()?;
    case.link_and_run(
        "tls-template",
        &case.corpus.join("linker/tls-template.expect"),
        &[&case.object("tls-template")],
    )
}

fn check_archive_index_rebuilds(case: &LinkerCase, archive: &Path, expected: &[u8]) -> CaseResult {
    // Solaris ar(3HEAD): the first member is the 32-bit symbol index. Keep
    // the long-name table, object members and their padding in both fixtures.
    const MAGIC: &[u8] = b"!<arch>\n";
    const INDEX_DATA: usize = MAGIC.len() + 60;
    let header = expected
        .strip_prefix(MAGIC)
        .and_then(|members| members.get(..60))
        .filter(|header| header.starts_with(b"/               ") && header.ends_with(b"`\n"))
        .ok_or("bar did not write a leading 32-bit index")?;
    let size = std::str::from_utf8(&header[48..58])
        .ok()
        .and_then(|field| field.trim_end().parse::<usize>().ok())
        .filter(|&size| (8..=expected.len() - INDEX_DATA).contains(&size))
        .ok_or("bar wrote an invalid index size")?;
    let remaining = expected
        .get(INDEX_DATA + size + size % 2..)
        .ok_or("bar wrote a truncated index member")?;
    let mut missing = MAGIC.to_vec();
    missing.extend_from_slice(remaining);
    let mut corrupt = expected.to_vec();
    // After the count, the first offset must locate an object member header.
    corrupt[INDEX_DATA + 4..INDEX_DATA + 8].fill(0);
    for (label, bytes) in [("missing", missing), ("corrupt", corrupt)] {
        if bytes == expected {
            return Err(format!("archive fixture did not create a {label} index").into());
        }
        io_at(archive, fs::write(archive, bytes))?;
        require_success(
            &format!("bar s ({label} index)"),
            case.tool("bar", [Path::new("s"), archive])?,
        )?;
        if io_at(archive, fs::read(archive))? != expected {
            return Err(format!("bar s did not rebuild the {label} index").into());
        }
    }
    Ok(())
}

fn archive_tools() -> CaseResult {
    let case = LinkerCase::start()?;
    let archive = case.make_archive("first.a")?;
    let second = case.make_archive("second.a")?;
    require_success("bar", case.tool("bar", [Path::new("s"), &second])?)?;
    let original = io_at(&archive, fs::read(&archive))?;
    if original != io_at(&second, fs::read(&second))? {
        return Err("bar produced different archive bytes after reindexing".into());
    }
    check_archive_index_rebuilds(&case, &second, &original)?;
    let symbols = require_success("bnm", case.tool("bnm", [Path::new("-Pg"), &second])?)?;
    #[cfg(target_arch = "x86_64")]
    let expected = format!(
        "{0}[got-entry.o]:\narchive_cell U\narchive_entry T 0 17\narchive_value U\n\
         {0}[archive-value-with-long-name.o]:\narchive_cell R 0 4\narchive_value T 0 6\n",
        second.display()
    );
    #[cfg(target_arch = "aarch64")]
    let expected = LINKER_BNM_SYMBOLS.replace("{0}", &second.display().to_string());
    if symbols != expected.as_bytes() {
        return Err(format!(
            "bnm returned unexpected symbols: {}",
            String::from_utf8_lossy(&symbols)
        )
        .into());
    }
    case.run_got(&second)
}

fn check_link_failure(result: std::process::Output, output: &Path, diagnostic: &str) -> CaseResult {
    if result.status.code() != Some(1)
        || output.exists()
        || !String::from_utf8_lossy(&result.stderr)
            .lines()
            .any(|line| line.contains(diagnostic))
    {
        return Err(format!(
            "expected bld exit 1, no output, and {diagnostic:?}; got {}: {}",
            result.status,
            String::from_utf8_lossy(&result.stderr).trim_end()
        )
        .into());
    }
    Ok(())
}

fn undefined_located() -> CaseResult {
    let case = LinkerCase::start()?;
    let output = case.scratch.join("undefined.elf");
    let main = case.object("got-main");
    let result = case.link(&output, &[&main])?;
    let stderr = String::from_utf8_lossy(&result.stderr);
    let location = format!("{}: .text+0x", main.display());
    if !stderr.lines().any(|line| {
        line.starts_with(&location) && line.ends_with(" archive_entry: undefined symbol")
    }) {
        return Err(format!("bld did not locate the reference to archive_entry: {stderr}").into());
    }
    check_link_failure(result, &output, LINKER_UNDEFINED_RELOCATION)?;
    let archive = case.make_archive("resolved.a")?;
    case.run_got(&archive)
}

fn duplicate_located() -> CaseResult {
    let case = LinkerCase::start()?;
    let output = case.scratch.join("duplicate.elf");
    let entry = case.object("got-entry");
    let duplicate = case.scratch.join("duplicate-entry.o");
    io_at(&duplicate, fs::copy(&entry, &duplicate))?;
    let result = case.link(&output, &[&case.object("got-main"), &entry, &duplicate])?;
    let diagnostic = format!(
        "{}: archive_entry: duplicate definition (first defined in {})",
        duplicate.display(),
        entry.display()
    );
    check_link_failure(result, &output, &diagnostic)?;
    let archive = case.make_archive("deduplicated.a")?;
    case.run_got(&archive)
}

fn language_case(category: &str) -> Result<(LinkerCase, PathBuf), String> {
    let case = LinkerCase::start()?;
    #[cfg(target_arch = "aarch64")]
    let category = if category == "abi" { "abi-aarch64" } else { category };
    let fixtures = case.corpus.join(category);
    Ok((case, fixtures))
}

fn interop_case(name: &str) -> CaseResult {
    let (case, fixtures) = language_case("interop")?;
    let object = case.scratch.join("interop.o");
    cc_stage(&case.scratch, &fixtures.join(format!("{name}.c")), &["-c", "-o", "interop.o"])?;
    case.link_and_run(name, &fixtures.join(format!("{name}.expect")),
        &[&object, &case.dev.join("lib/libinterop.a")])
}

fn abi_case(name: &str) -> CaseResult {
    let (case, fixtures) = language_case("abi")?;
    for (compiled, staged) in [("caller", "callee"), ("callee", "caller")] {
        let object = case.scratch.join("compiled.o");
        cc_stage(&case.scratch, &fixtures.join(format!("{name}-{compiled}.c")), &["-c", "-o", "compiled.o"])?;
        let staged_object = fixtures.join(format!("{name}-{staged}.o"));
        #[cfg(target_arch = "aarch64")]
        let brt = case.dev.join("lib/libbrt.a");
        let inputs: &[&Path] = &[
            &object,
            &staged_object,
            #[cfg(target_arch = "aarch64")]
            &brt,
        ];
        case.link_and_run(name, &fixtures.join(format!("{name}.expect")), inputs)?;
    }
    Ok(())
}

fn tls_case(name: &str) -> CaseResult {
    let (case, fixtures) = language_case("tls")?;
    let object = case.scratch.join("tls.o");
    cc_stage(&case.scratch, &fixtures.join(format!("{name}.c")), &["-c", "-o", "tls.o"])?;
    case.link_and_run(name, &fixtures.join(format!("{name}.expect")), &[&object])
}

fn c_calls_rust() -> CaseResult { interop_case("c-calls-rust") }
fn rust_calls_c() -> CaseResult { interop_case("rust-calls-c") }
fn aggregates_exhaustion() -> CaseResult { abi_case("aggregates-exhaustion") }
fn varargs() -> CaseResult { abi_case("varargs") }
fn sse_float() -> CaseResult { abi_case("sse-float") }
fn x87_long_double() -> CaseResult { abi_case("x87-long-double") }
fn template_init_zero_fill_aligned() -> CaseResult { tls_case("template-init-zero-fill-aligned") }
fn errno_per_thread() -> CaseResult { tls_case("errno-per-thread") }
fn tls_across_fork_exec() -> CaseResult { tls_case("tls-across-fork-exec") }

fn c11_fixtures() -> Result<PathBuf, String> {
    Ok(Path::new("/").join(CORPUS).join("c11"))
}

fn c11_program(name: &str) -> CaseResult {
    let fixtures = c11_fixtures()?;
    let scratch = Scratch::new(name)?;
    cc_stage(&scratch, &fixtures.join(format!("{name}.c")), &["-o", name])?;
    run_expected(&scratch.join(name), &fixtures.join(format!("{name}.expect")))
}

fn c11_declarations_initializers() -> CaseResult {
    c11_program("declarations-initializers")
}

fn c11_generic_static_assert_alignment() -> CaseResult {
    c11_program("generic-static-assert-alignment")
}

fn c11_floating() -> CaseResult {
    c11_program("floating")
}

fn c11_atomics() -> CaseResult {
    c11_program("atomics")
}

fn c11_constraint_diagnostics() -> CaseResult {
    let fixtures = c11_fixtures()?;
    let path = fixtures.join("constraints.json");
    let manifest: Value = serde_json::from_slice(&io_at(&path, fs::read(&path))?)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let mut guest = manifest["guest"]
        .as_object()
        .ok_or("missing C11 guest constraint subset")?
        .values()
        .map(|file| file.as_str().ok_or("invalid C11 guest constraint filename"))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let constraints = manifest["fixtures"]
        .as_array()
        .ok_or("missing C11 constraint fixtures")?;

    let (status, expected) = expected_output(&fixtures.join("constraint-diagnostics.expect"))?;
    let scratch = Scratch::new("constraint-diagnostics")?;
    let object = scratch.join("unexpected.o");
    let mut diagnostics = Vec::new();
    for constraint in constraints {
        let file = constraint["file"].as_str().ok_or("missing C11 constraint filename")?;
        if !guest.remove(file) {
            continue;
        }
        let diagnostic = constraint["diagnostic"]
            .as_str()
            .ok_or_else(|| format!("{file}: missing expected diagnostic"))?;
        let result = command_output(
            bundle_command(&scratch, "cc")
                .current_dir(&fixtures)
                .arg("-c")
                .arg("-o")
                .arg(&object)
                .arg(file),
        )?;
        if result.status.code() != Some(status)
            || !result.stdout.is_empty()
            || result.stderr != format!("{diagnostic}\n").as_bytes()
            || io_at(&object, object.try_exists())?
        {
            return Err(format!(
                "{file}: expected exit {status}, empty stdout, no object and {diagnostic:?}; got {}, stdout {:?}, stderr {:?}",
                result.status,
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr),
            ).into());
        }
        diagnostics.extend(result.stderr);
    }
    if !guest.is_empty() {
        return Err(format!("C11 guest constraints missing from the manifest: {guest:?}").into());
    }
    if diagnostics != expected {
        return Err("guest diagnostics differ from constraint-diagnostics.expect".into());
    }
    Ok(())
}

fn c11_profile_macros() -> CaseResult {
    c11_program("profile-macros")
}

fn c17_program() -> CaseResult {
    let name = running_case().and_then(|case| case.strip_prefix("c17/"))
        .ok_or("missing C17 case name")?;
    let fixtures = Path::new("/").join(CORPUS).join("c17");
    let source = fixtures.join(format!("{name}.c"));
    let expect = fixtures.join(format!("{name}.expect"));
    let scratch = Scratch::new(name)?;
    for level in ["-O0", "-O2"] {
        let output = scratch.join(format!("{name}{level}.elf"));
        let mut command = bundle_command(&scratch, "cc");
        command.args(["-std=c17", level]);
        if name == "rounding-math" {
            command.arg("-frounding-math");
        }
        command.arg("-o").arg(&output).arg(&source);
        build_in_group(&scratch, &mut command, &format!("cc {level} {name}"))?;
        run_expected(&output, &expect)?;
    }
    Ok(())
}

fn build_in_group(scratch: &Scratch, command: &mut Command, description: &str) -> CaseResult {
    build_in_group_at(scratch.join("build.log").as_path(), command, description, false)
}

fn build_in_group_at(path: &Path, command: &mut Command, description: &str, progress: bool) -> CaseResult {
    let log = io_at(path, File::create(path))?;
    let mut reader = BufReader::new(io_at(path, File::open(path))?);
    let mut line = String::new();
    command.process_group(0).stdout(io_at(path, log.try_clone())?).stderr(log);
    let mut child = command.spawn().map_err(|error| format!("{description}: {error}"))?;
    loop {
        let status = child.try_wait().map_err(|error| format!("{description}: {error}"))?;
        if progress {
            if let Err(error) = build_progress(&mut reader, &mut line) {
                if status.is_none() {
                    let _ = signal::kill(-(child.id() as i32), signal::SIGKILL);
                    let _ = child.wait();
                }
                return Err(format!("{}: reading build progress: {error}", path.display()).into());
            }
        }
        if let Some(status) = status {
            if status.success() {
                return Ok(());
            }
            let log = io_at(path, fs::read_to_string(path))?;
            // Make echoes the recipes, so keep the tail containing the tool's diagnostic.
            let tail = log.lines().rev().take(6).collect::<Vec<_>>();
            return Err(format!("{description}: {status}: {}", tail.iter().rev().copied().collect::<Vec<_>>().join(" | ")).into());
        }
        if case_ms_left() < 500 {
            signal::kill(-(child.id() as i32), signal::SIGKILL)
                .map_err(|error| format!("{description}: time limit reached, group kill failed: {error}"))?;
            child.wait().map_err(|error| format!("{description}: reaping timed-out build: {error}"))?;
            return Err(format!("{description}: timed out and its process group was killed").into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn build_progress(reader: &mut impl BufRead, line: &mut String) -> std::io::Result<()> {
    while reader.read_line(line)? > 0 {
        if !line.ends_with('\n') {
            break;
        }
        if line.starts_with("BUSYBOX_PHASE ") {
            case_diagnostic(line.trim_end());
        }
        line.clear();
    }
    Ok(())
}

fn libc_cases() -> &'static [breenix_suite::suite::Case] {
    SUITE.categories.iter().find(|category| category.id == "libc-suite").unwrap().cases
}

fn libc_reference(category: &str) -> PathBuf {
    Path::new(LIBC_REFERENCE_DIR).join(format!("libc-{category}_test.elf"))
}

fn libc_make(scratch: &Scratch, category: &str, targets: &[&str]) -> CaseResult {
    let corpus = Path::new("/").join(CORPUS);
    let dev = Path::new("/").join(DEV);
    let mut command = bundle_command(scratch, "make");
    command.arg("-f").arg(corpus.join("libc-suite.mk")).args([
        "BUILD=.".to_string(),
        format!("ARCH={}", std::env::consts::ARCH),
        format!("CATEGORY={category}"),
        format!("SUITE={}", corpus.join("libc-suite").display()),
        format!("INCLUDE={}", dev.join("include").display()),
        format!("CC_INCLUDE={}", dev.join("include-support").display()),
        format!("LINKER_SCRIPT={}", dev.join("linker.ld").display()),
        format!("LIBC={}", dev.join("lib/libc.a").display()),
        format!("BRT={}", dev.join("lib/libbrt.a").display()),
        format!("FUNCTIONS={}", corpus.join("libc-functions.txt").display()),
        "NM=bnm".to_string(),
    ]).args(targets);
    build_in_group(scratch, &mut command, &format!("bmake {category}"))
}

fn libc_prerequisite_hashes(directory: &Path) -> Result<String, String> {
    let mut hashes = String::new();
    for name in LIBC_PREREQUISITES {
        hashes.push_str(&format!("{}  {name}\n", file_sha256(&directory.join(name))?));
    }
    Ok(hashes)
}

fn libc_category() -> CaseResult {
    let category = running_case().and_then(|case| case.strip_prefix("libc-suite/"))
        .ok_or("missing libc category name")?;
    let nonce = LIBC_NONCE.get().ok_or("could not obtain libc build nonce")?;
    let chain = Path::new("/tmp").join(format!("toolchain-libc-{}", hex(nonce)));
    let prereqs = chain.join("prerequisites");
    let receipt = chain.join("prerequisites.sha256");
    if libc_cases().first().is_some_and(|case| case.id == category) {
        io_at(&chain, fs::DirBuilder::new().mode(0o700).create(&chain))?;
        let build = Scratch::create(prereqs.clone())?;
        libc_make(&build, category, &["./abi_check.o", "./harness.o"])?;
        io_at(&receipt, fs::write(&receipt, libc_prerequisite_hashes(&prereqs)?))?;
        // The remaining cases consume these two objects under this boot's nonce.
        std::mem::forget(build);
    }
    let _cleanup = libc_cases().last().filter(|case| case.id == category)
        .map(|_| Scratch(chain.clone()));
    let scratch = Scratch::create(chain.join(category))?;
    let expected = io_at(&receipt, fs::read_to_string(&receipt))?;
    if libc_prerequisite_hashes(&prereqs)? != expected {
        return Err("libc prerequisite hashes differ from the first case's receipt".into());
    }
    for name in LIBC_PREREQUISITES {
        let path = scratch.join(name);
        io_at(&path, fs::copy(prereqs.join(name), &path))?;
    }
    libc_make(&scratch, category, &["all"])?;
    if libc_prerequisite_hashes(&scratch.0)? != expected {
        return Err("libc category build changed its prerequisite objects".into());
    }
    compare_libc_results(&scratch, category)
}

fn compare_libc_results(scratch: &Scratch, category: &str) -> CaseResult {
    let path = Path::new("/").join(CORPUS).join("libc-suite/libc.json");
    let manifest: Value = serde_json::from_slice(&io_at(&path, fs::read(&path))?)
        .map_err(|error| format!("invalid libc manifest: {error}"))?;
    let cases = manifest["categories"].as_array()
        .and_then(|categories| categories.iter().find(|entry| entry["id"] == category))
        .and_then(|entry| entry["cases"].as_array()).filter(|cases| !cases.is_empty())
        .ok_or_else(|| format!("no libc cases declared for {category}"))?;
    let reference = Path::new("/").join(libc_reference(category));
    let candidate = scratch.join(format!("libc-{category}_test.elf"));
    let mut failures = Vec::new();
    for case in cases {
        let id = case["id"].as_str().ok_or("missing libc case ID")?;
        let reference = libc_result(scratch, &reference, id)?;
        let candidate = libc_result(scratch, &candidate, id)?;
        if reference != candidate {
            failures.push(format!("{id}: reference {reference:?}, native {candidate:?}"));
        } else if reference != "PASS" {
            failures.push(format!("{id}: both builds reported {reference}"));
        }
    }
    if failures.is_empty() {
        return Ok(());
    }
    for failure in &failures {
        case_diagnostic(failure);
    }
    Err(format!("{category}: {} of {} cases failed, first: {}", failures.len(), cases.len(), failures[0]).into())
}

fn libc_result(scratch: &Scratch, binary: &Path, id: &str) -> Result<String, CaseError> {
    // Keep the result descriptor outside the case's working directory. Some
    // cases count directory entries and the two producers need separate fixtures.
    let directory = Scratch::create(scratch.join("run"))?;
    let path = scratch.join("result");
    let report = io_at(&path, File::create(&path))?;
    let fd = report.as_raw_fd();
    io::fcntl_setfd(Fd::from_raw(fd as u64), 0)
        .map_err(|error| format!("libc result fd: {error}"))?;
    let result = Command::new(binary)
        .arg0(binary.file_stem().ok_or("libc ELF is missing a name")?)
        .arg(id).arg(fd.to_string())
        .current_dir(&directory.0)
        .env_clear().env("PATH", "/usr/libexec/breenix/libc/bin")
        .env("HOME", "/tmp").env("LIBC_SUITE", "1")
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .status();
    drop(report);
    let status = io_at(binary, result)?;
    let report = io_at(&path, fs::read_to_string(&path))?;
    // OBS lines describe timing phases, separate from the harness's verdict.
    let mut verdicts = report.lines().filter(|line| !line.starts_with("OBS "));
    let verdict = verdicts.next().unwrap_or_default();
    if !status.success() || !report.ends_with('\n') || verdicts.next().is_some()
        || !(verdict == "PASS" || verdict.starts_with("FAIL "))
    {
        return Err(format!("{} {id}: {status}, invalid result {report:?}", binary.display()).into());
    }
    Ok(verdict.to_string())
}

fn inline_make_at(scratch: &Scratch, name: &str, recipe: &str) -> CaseResult {
    let makefile = scratch.join("Makefile");
    io_at(&makefile, fs::write(&makefile, recipe))?;
    let mut command = bundle_command(scratch, "make");
    command.arg("all");
    build_in_group(scratch, &mut command, name)
}

fn run_inline_make(name: &str, recipe: &str) -> Result<Scratch, CaseError> {
    let scratch = Scratch::new(name)?;
    inline_make_at(&scratch, name, recipe)?;
    Ok(scratch)
}

fn make_archive_members() -> CaseResult {
    let scratch = Scratch::new("archive-members")?;
    for (name, contents) in [
        ("member.c", "int member(void) { return 42; }\n"),
        ("main.c", "int member(void); int main(void) { return member() != 42; }\n"),
        ("linked.expect", "0\n"),
    ] {
        let path = scratch.join(name);
        io_at(&path, fs::write(&path, contents))?;
    }
    inline_make_at(&scratch, "archive-members", concat!(
        "all: lib.a(member.o)\n",
        "lib.a(member.o): member.o\n\tar rcs $@ $%\n",
        "member.o: member.c\n\tcc -c -o $@ member.c\n",
    ))?;
    let mut command = bundle_command(&scratch, "cc");
    command.args(["-o", "linked", "main.c", "lib.a"]);
    build_in_group(&scratch, &mut command, "link archive member")?;
    run_expected(&scratch.join("linked"), &scratch.join("linked.expect"))
}

fn make_include_remake() -> CaseResult {
    let scratch = run_inline_make("include-remake", concat!(
        "include generated.mk\n",
        "all:\n\tprintf '%s\\n' '$(VALUE)' > result\n",
        "generated.mk:\n\tprintf 'VALUE = remade\\n' > $@\n",
    ))?;
    expect_bytes(&scratch.join("result"), b"remade\n")
}

fn make_recursive_makeflags() -> CaseResult {
    let scratch = Scratch::new("recursive-makeflags")?;
    let parent = scratch.join("Makefile");
    let child = scratch.join("child.mk");
    io_at(&parent, fs::write(&parent, "all:\n\t$(MAKE) -f child.mk child\n"))?;
    io_at(&child, fs::write(&child, "child:\n\tprintf '%s\\n' '$(TOKEN)' > result\n"))?;
    // The child recipe must stay silent under inherited -s and receive TOKEN.
    let output = command_output(bundle_command(&scratch, "make").args(["-s", "TOKEN=inherited", "all"]))?;
    if !require_success("recursive make", output)?.is_empty() {
        return Err("submake echoed a recipe despite inherited -s".into());
    }
    expect_bytes(&scratch.join("result"), b"inherited\n")
}

fn expect_bytes(path: &Path, expected: &[u8]) -> CaseResult {
    let actual = io_at(path, fs::read(path))?;
    if actual != expected {
        return Err(format!("{}: expected {expected:?}, got {actual:?}", path.display()).into());
    }
    Ok(())
}

fn gnu_make_features() -> CaseResult {
    let scratch = run_inline_make("gnu-make-features", concat!(
        "names := alpha beta\n",
        "ifeq ($(words $(names)),2)\n",
        "all: $(patsubst %,%.out,$(names))\n",
        "endif\n",
        "define recipe\n",
        "$(1).out: | ready\n",
        "\tprintf '%s\\n' '$(1)' > $$@\n",
        "endef\n",
        "$(foreach name,$(names),$(eval $(call recipe,$(name))))\n",
        "ready:\n\tprintf 'ready\\n' > $@\n",
    ))?;
    expect_bytes(&scratch.join("alpha.out"), b"alpha\n")?;
    expect_bytes(&scratch.join("beta.out"), b"beta\n")?;
    expect_bytes(&scratch.join("ready"), b"ready\n")
}

fn gnu_c_extensions() -> CaseResult {
    let scratch = Scratch::new("gnu-c-extensions")?;
    let source = scratch.join("gnu.c");
    let binary = scratch.join("gnu");
    let expect = scratch.join("gnu.expect");
    io_at(&source, fs::write(&source, r#"
#define SUM(first, extra...) sum(first, ## extra, 0)
static int sum(int a, int b, ...) { return a + b; }
int main(void) {
    int n = 6;
    typeof(n) copied = ({ int doubled = 2 * n; doubled; });
    __asm__ __volatile__("" : : : "memory");
    switch (copied) {
    case 10 ... 12: return SUM(copied, 30) != 42 || SUM(42) != 42;
    default: return 2;
    }
}
"#))?;
    io_at(&expect, fs::write(&expect, "0\n"))?;
    let mut command = bundle_command(&scratch, "cc");
    command.args(["-std=gnu11", "-O2", "-o"]).arg(&binary).arg(&source);
    build_in_group(&scratch, &mut command, "compile GNU C fixture")?;
    run_expected(&binary, &expect)
}

static BUSYBOX_SCRATCH: OnceLock<Result<(), String>> = OnceLock::new();
const BUSYBOX_MOUNT: &str = "/mnt/breenix-toolchain";
const BUSYBOX_RECORD: &str = "/mnt/breenix-toolchain/busybox-under-test";

fn busybox_fixture() -> PathBuf {
    Path::new("/").join(CORPUS).join("busybox")
}

fn busybox_run() -> Result<PathBuf, String> {
    let nonce = LIBC_NONCE.get().ok_or("could not get a toolchain boot nonce")?;
    Ok(Path::new(BUSYBOX_MOUNT).join(hex(nonce)))
}

fn busybox_mount() -> Result<(), String> {
    use libbreenix::fs as sysfs;

    io_at(Path::new(BUSYBOX_MOUNT), fs::create_dir_all(BUSYBOX_MOUNT))?;
    let mut companions = Vec::new();
    for disk in io_at(Path::new("/dev"), fs::read_dir("/dev"))? {
        let disk = io_at(Path::new("/dev"), disk)?;
        let name = disk.file_name();
        if name.to_str().and_then(|name| name.strip_prefix("disk"))
            .and_then(|index| index.parse::<u8>().ok()).is_none()
        {
            continue;
        }
        let path = disk.path();
        let device = path.to_str().ok_or("invalid scratch disk path")?;
        // Probe read-only before binding the selected disk for writes.
        if unsafe { sysfs::mount(device, BUSYBOX_MOUNT, "breenfs", 1, std::ptr::null()) }.is_err() {
            continue;
        }
        let marker = Path::new(BUSYBOX_MOUNT).join("busybox-scratch");
        let matches = fs::read(marker).is_ok_and(|bytes| bytes == b"toolchain/busybox-on-box\n");
        sysfs::umount2(BUSYBOX_MOUNT, 0).map_err(|error| format!("detach {device}: {error}"))?;
        if matches {
            companions.push(path);
        }
    }
    let [disk] = companions.as_slice() else {
        return Err(format!("expected one BusyBox scratch companion, found {companions:?}"));
    };
    let device = disk.to_str().ok_or("invalid scratch disk path")?;
    unsafe { sysfs::mount(device, BUSYBOX_MOUNT, "breenfs", 0, std::ptr::null()) }
        .map_err(|error| format!("mount {device} at {BUSYBOX_MOUNT}: {error}"))?;
    Ok(())
}

fn busybox_driver(phase: &str, run: &Path) -> CaseResult {
    let fixture = busybox_fixture();
    let driver = fixture.join("busybox.sh");
    if !driver.is_file() {
        return Err(format!("{} is required for toolchain/busybox-on-box", driver.display()).into());
    }
    if phase == "unpack" {
        BUSYBOX_SCRATCH.get().ok_or("BusyBox scratch mount was not prepared")?
            .as_ref().map_err(Clone::clone)?;
        io_at(run, fs::create_dir(run))?;
    }
    let arch = if cfg!(target_arch = "aarch64") { "aarch64" } else { "x86_64" };
    let mut command = Command::new("/bin/sh");
    command
        .env_clear()
        .env("BUSYBOX_BUNDLE", Path::new("/").join(DEV))
        .env("BMAKE_INHERIT_PROCESS_GROUP", "1")
        .args([driver.as_os_str(), phase.as_ref(), fixture.as_os_str(), run.join("build").as_os_str(), arch.as_ref()])
        .stdin(Stdio::null());
    build_in_group_at(&run.join(format!("{phase}.log")), &mut command, phase, true)
}

fn busybox_phase() -> CaseResult {
    let name = running_case().and_then(|name| name.strip_prefix("busybox/"))
        .ok_or("missing BusyBox phase case name")?;
    busybox_driver(name, &busybox_run()?)
}

fn applet_list(text: &str) -> Result<BTreeSet<&str>, String> {
    let mut names = BTreeSet::new();
    for name in text.lines() {
        if name.is_empty() || !names.insert(name) {
            return Err(format!("invalid applet list entry {name:?}"));
        }
    }
    if names.is_empty() {
        return Err("empty applet list".into());
    }
    Ok(names)
}

fn busybox_link_and_list() -> CaseResult {
    let run = busybox_run()?;
    busybox_driver("link-and-list", &run)?;
    let build = run.join("build");
    let binary = build.join("busybox.elf");
    io_at(&binary, fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)))?;
    let report = build.join("applets.json");
    let report = serde_json::from_slice::<Value>(&io_at(&report, fs::read(&report))?)
        .map_err(|error| format!("bad on-box applets.json: {error}"))?;
    let verdicts = report["applets"].as_object().ok_or("missing per-applet results")?;
    let reference = busybox_fixture().join("mac-applets.txt");
    let expected = io_at(&reference, fs::read_to_string(&reference))?;
    let expected = applet_list(&expected)?;
    let mut command = Command::new(&binary);
    command.arg0("busybox").arg("--list").env_clear();
    let listing = run.join("applets.list");
    build_in_group_at(&listing, &mut command, "fresh BusyBox --list", false)?;
    let output = io_at(&listing, fs::read_to_string(&listing))?;
    let actual = applet_list(&output)?;
    if actual != expected {
        return Err(format!("BusyBox applet list: missing {:?}, additional {:?}",
            expected.difference(&actual).collect::<Vec<_>>(),
            actual.difference(&expected).collect::<Vec<_>>()).into());
    }
    if verdicts.len() != expected.len() || expected.iter().any(|name| {
        verdicts.get(*name).is_none_or(|row| row["compiles"] != true || row["links"] != true)
    }) {
        return Err("on-box applets.json differs from the executable's applet list".into());
    }
    let names: Vec<_> = actual.into_iter().collect();
    busybox_record::publish(Path::new(BUSYBOX_RECORD), &binary, &names)?;
    Ok(())
}

fn busybox_required_usage() -> CaseResult {
    let build = busybox_run()?.join("build");
    let bin = build.join("bin");
    let expected = include_str!("../../../vendor/busybox/required-applets");
    for applet in expected.lines().filter(|line| !line.is_empty() && !line.starts_with('#')) {
        if !bin.join(applet).is_file() {
            return Err(format!("built BusyBox is missing required applet {applet}").into());
        }
    }
    let scratch = Scratch::new("busybox-required-usage")?;
    let mut command = Command::new(bin.join("sh"));
    command.env_clear().env("PATH", bin)
        .current_dir(&scratch.0)
        .args(["-ec", "printf '%s\\n' 'BusyBox 42' > input; awk '{ print $2 }' input; sha256sum input"]);
    let log = scratch.join("usage.log");
    build_in_group_at(&log, &mut command, "fresh BusyBox sh and required applets", false)?;
    let expected = format!("42\n{}  input\n", hex(&sha256(b"BusyBox 42\n")));
    expect_bytes(&log, expected.as_bytes())
}

fn kbuild_host_programs() -> CaseResult {
    let source = busybox_run()?.join("build/busybox-1.37.0");
    let scratch = Scratch::new("kbuild-host-programs")?;
    let deps = scratch.join("probe.d");
    io_at(&deps, fs::write(&deps, "probe.o: probe.c\n"))?;
    io_at(&scratch.join("probe.c"), fs::write(scratch.join("probe.c"), b"int probe;\n"))?;
    let output = require_success(
        "native Kbuild fixdep",
        command_output(Command::new(source.join("scripts/basic/fixdep"))
            .current_dir(&scratch.0).args(["probe.d", "probe.o", "cc -c probe.c"]))?,
    )?;
    let text = std::str::from_utf8(&output).map_err(|error| error.to_string())?;
    if !(text.contains("cmd_probe.o := cc -c probe.c") && text.contains("probe.c")) {
        return Err(format!("unexpected fixdep dependency record {text:?}").into());
    }
    let receipt = source.parent().unwrap().join("generators.sha256");
    for executable in ["scripts/basic/split-include", "scripts/kconfig/conf", "applets/usage", "applets/applet_tables"] {
        if !source.join(executable).is_file() {
            return Err(format!("native Kbuild generator {executable} missing").into());
        }
    }
    if !receipt.is_file() {
        return Err("native Kbuild generator receipt missing".into());
    }
    Ok(())
}

#[cfg(target_arch = "x86_64")]
fn four_objects_match_record() -> CaseResult {
    const OBJECTS: [(&str, &str); 4] = [
        ("src/syscall/entry.asm", "syscall_entry.o"),
        ("src/interrupts/timer_entry.asm", "timer_entry.o"),
        ("src/interrupts/breakpoint_entry.asm", "breakpoint_entry.o"),
        ("src/arch_impl/x86_64/ap_trampoline.asm", "ap_trampoline.o"),
    ];

    let case = AssemblyCase::start_at("kernel-asm")?;
    let path = case.corpus.join("kernel-assembly.record");
    let record = io_at(&path, fs::read_to_string(&path))?;
    let mut fields = BTreeMap::new();
    for row in record.lines() {
        let (name, value) = row.split_once('=').ok_or("malformed kernel assembly record")?;
        if name.is_empty() || value.is_empty() || fields.insert(name, value).is_some() {
            return Err(format!("invalid kernel assembly record field {name:?}").into());
        }
    }

    for (source, object) in OBJECTS {
        let input = case.corpus.join(source);
        if fields.get(source).copied() != Some(file_sha256(&input)?.as_str()) {
            return Err(format!("{source} does not match the kernel assembly record").into());
        }
        let staged = case.corpus.join(object);
        let expected = io_at(&staged, fs::read(&staged))?;
        if fields.get(object).copied() != Some(hex(&sha256(&expected)).as_str()) {
            return Err(format!("{object} does not match the kernel assembly record").into());
        }
        let fresh = case.scratch.join(object);
        require_success(source, case.run(&input, &fresh)?)?;
        if io_at(&fresh, fs::read(&fresh))? != expected {
            return Err(format!("fresh {object} differs from the recorded object").into());
        }
    }
    Ok(())
}

#[cfg(target_arch = "x86_64")]
fn privileged_encodings() -> CaseResult {
    let case = AssemblyCase::start_at("kernel-asm")?;
    let object = case.assemble("privileged")?;
    let bytes = io_at(&object, fs::read(&object))?;
    let actual = object_text(&bytes).ok_or("privileged object has no valid .text section")?;
    let path = case.corpus.join("privileged.hex");
    let expected = x86_encoding_bytes(&path, &io_at(&path, fs::read_to_string(&path))?)?;
    if expected.is_empty() || actual != expected {
        return Err(format!(
            "privileged .text differs from the SDM table (expected {}, got {})",
            hex(&expected), hex(actual),
        ).into());
    }
    Ok(())
}

#[cfg(target_arch = "x86_64")]
mod breakpoint {
    use super::{signal, CaseResult};
    use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

    const TRAP_BRKPT: i32 = 1;
    static DELIVERIES: AtomicUsize = AtomicUsize::new(0);
    static CODE: AtomicI32 = AtomicI32::new(0);
    static ADDRESS: AtomicUsize = AtomicUsize::new(0);

    // The x86 siginfo_t prefix through si_addr, including ABI padding.
    #[repr(C)]
    struct FaultInfo {
        signo: i32,
        errno: i32,
        code: i32,
        addr: usize,
    }

    extern "C" fn handler(sig: i32, info: *const FaultInfo, _context: *const u8) {
        // SAFETY: SA_SIGINFO gives the handler a readable siginfo_t on its stack.
        if let Some(info) = unsafe { info.as_ref() } {
            if sig == signal::SIGTRAP && info.signo == sig {
                CODE.store(info.code, Ordering::Relaxed);
                ADDRESS.store(info.addr, Ordering::Relaxed);
                DELIVERIES.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub(super) fn roundtrip() -> CaseResult {
        let action = signal::Sigaction {
            handler: handler as *const () as u64,
            flags: signal::SA_SIGINFO | signal::SA_RESTORER,
            restorer: signal::__restore_rt as *const () as u64,
            mask: 0,
        };
        signal::sigaction(signal::SIGTRAP, Some(&action), None)?;
        let address: usize;
        let resumed: usize;
        // Increment the marker at the instruction where INT3 must resume.
        unsafe {
            core::arch::asm!(
                "lea {address}, [rip + 2f]",
                "xor {resumed:e}, {resumed:e}",
                "2: int3",
                "inc {resumed}",
                address = out(reg) address,
                resumed = out(reg) resumed,
                options(nostack),
            );
        }
        if resumed != 1 || DELIVERIES.load(Ordering::Relaxed) != 1
            || CODE.load(Ordering::Relaxed) != TRAP_BRKPT
            || ADDRESS.load(Ordering::Relaxed) != address
        {
            return Err("INT3 did not deliver SIGTRAP with TRAP_BRKPT and resume after the breakpoint".into());
        }
        Ok(())
    }
}

#[cfg(target_arch = "x86_64")]
fn breakpoint_roundtrip() -> CaseResult {
    breakpoint::roundtrip()
}
