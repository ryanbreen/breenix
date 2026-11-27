//! Run the E25 corpus with the six digest-bound self-host outputs.
use crate::sha256::Sha256;
use std::collections::BTreeMap;
use std::fs::{self, DirBuilder, File};
use std::io::{self, Read};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

#[cfg(target_arch = "x86_64")]
pub const PROFILE: &str = "target/x86_64-breenix/selfhost";
#[cfg(target_arch = "aarch64")]
pub const PROFILE: &str = "target/aarch64-breenix/selfhost";
const TOOLS: [(&str, &str); 6] = [
    ("bcc", "cc"),
    ("bas", "as"),
    ("bld", "ld"),
    ("bar", "ar"),
    ("bnm", "nm"),
    ("bmake", "make"),
];

#[cfg(target_arch = "x86_64")]
// 20261124T033037Z-metal, accepted into main by #3145 (E25 ruling 75).
// Assembler (3/3): 150 + 65 + 26. Linker (6/6): 10881 + 392 + 396
// + 614 + 803 + 539. Corpus-three-producers: 22663. Builds-and-runs: 1398.
const WINDOW: Duration = Duration::from_millis((37_927u64 * 3).div_ceil(2));

#[cfg(target_arch = "aarch64")]
// 20261126T234303Z-qemu-62721, accepted into main by #3182 (E25 ruling 144).
// Assembler (3/3): 11 + 13 + 3. Linker (6/6): 2134 + 93 + 83
// + 110 + 142 + 84. Corpus-three-producers: 3703. Builds-and-runs: 179.
const WINDOW: Duration = Duration::from_millis((6_555u64 * 3).div_ceil(2));

pub struct ToolTable {
    bin: PathBuf,
    work: PathBuf,
    digests: BTreeMap<&'static str, String>,
    deadline: Instant,
}

impl ToolTable {
    pub fn stage(run: &Path, recorded: &BTreeMap<String, String>) -> Result<Self, String> {
        let deadline = Instant::now() + WINDOW;
        let bin = run.join("rebuilt-bin");
        let work = run.join("rebuilt-work");
        for dir in [&bin, &work] {
            at(dir, DirBuilder::new().mode(0o700).create(dir))?;
        }
        let mut digests = BTreeMap::new();
        for (package, alias) in TOOLS {
            let digest = recorded
                .get(package)
                .ok_or_else(|| format!("missing verified digest for {package}"))?;
            let source = run.join(PROFILE).join(package);
            at(&source, fs::copy(&source, bin.join(alias)))?;
            digests.insert(alias, digest.clone());
        }
        let table = Self {
            bin,
            work,
            digests,
            deadline,
        };
        table.verify_all()?;
        Ok(table)
    }

    fn verify(&self, name: &str, executable: &Path) -> Result<(), String> {
        let digest = self
            .digests
            .get(name)
            .ok_or_else(|| format!("unknown rebuilt tool {name}"))?;
        let expected = self.bin.join(name);
        if executable.as_os_str() != expected.as_os_str() {
            return Err(format!(
                "expected rebuilt executable {}, got {}",
                expected.display(),
                executable.display()
            ));
        }
        let metadata = at(executable, fs::symlink_metadata(executable))?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            return Err(format!(
                "{}: expected a regular executable copy",
                executable.display()
            ));
        }
        if file_digest(executable)? != *digest {
            return Err(format!("SHA-256 changed for {}", executable.display()));
        }
        Ok(())
    }

    pub fn verify_all(&self) -> Result<(), String> {
        for &name in self.digests.keys() {
            self.verify(name, &self.bin.join(name))?;
        }
        for entry in at(&self.bin, fs::read_dir(&self.bin))? {
            let entry = at(&self.bin, entry)?;
            if !entry
                .file_name()
                .to_str()
                .is_some_and(|name| self.digests.contains_key(name))
            {
                return Err(format!(
                    "unexpected rebuilt-bin entry {}",
                    entry.path().display()
                ));
            }
        }
        Ok(())
    }

    pub fn output(&self, name: &str, command: &mut Command) -> Result<Output, String> {
        self.verify(name, Path::new(command.get_program()))?;
        // The driver resolves these exact paths through -B or its own directory.
        // Make recipes receive absolute macro values and use the same driver.
        let delegates: &[&str] = match name {
            "cc" => &["as", "ld"],
            "make" => &["cc", "as", "ld", "ar"],
            _ => &[],
        };
        for &tool in delegates {
            self.verify(tool, &self.bin.join(tool))?;
        }
        self.run(command)
    }

    fn run(&self, command: &mut Command) -> Result<Output, String> {
        let path = self.work.join("stdout");
        let stdout = at(&path, File::create(&path))?;
        let error_path = self.work.join("stderr");
        let stderr = at(&error_path, File::create(&error_path))?;
        command
            .env_clear()
            .env("PATH", format!("{}:/bin", self.bin.display()))
            .env("TMPDIR", &self.work)
            .env("BMAKE_INHERIT_PROCESS_GROUP", "1")
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .process_group(0);
        let executable = PathBuf::from(command.get_program());
        if Instant::now() >= self.deadline {
            return Err(format!(
                "{}: rebuilt tools exceeded {} ms",
                executable.display(),
                WINDOW.as_millis()
            ));
        }
        let mut child = at(&executable, command.spawn())?;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < self.deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                pending => {
                    let stop = kill_group(child.id());
                    if stop.is_ok() {
                        at(&executable, child.wait())?;
                    }
                    at(&executable, stop)?;
                    at(&executable, pending)?;
                    return Err(format!(
                        "{}: rebuilt tools exceeded {} ms",
                        executable.display(),
                        WINDOW.as_millis()
                    ));
                }
            }
        };
        Ok(Output {
            status,
            stdout: at(&path, fs::read(&path))?,
            stderr: at(&error_path, fs::read(&error_path))?,
        })
    }
}

#[cfg(target_os = "breenix")]
fn kill_group(pid: u32) -> io::Result<()> {
    use libbreenix::{errno::Errno, error::Error, signal};
    match signal::kill(-(pid as i32), signal::SIGKILL) {
        Ok(()) | Err(Error::Os(Errno::ESRCH)) => Ok(()),
        Err(Error::Os(errno)) => Err(io::Error::from_raw_os_error(errno as i32)),
    }
}

#[cfg(not(target_os = "breenix"))]
fn kill_group(pid: u32) -> io::Result<()> {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    // SAFETY: Command created a private group identified by this child's PID.
    if unsafe { kill(-(pid as i32), 9) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn at<T>(path: &Path, result: io::Result<T>) -> Result<T, String> {
    result.map_err(|error| format!("{}: {error}", path.display()))
}

fn file_digest(path: &Path) -> Result<String, String> {
    let mut file = at(path, File::open(path))?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 32 * 1024];
    loop {
        let count = at(path, file.read(&mut buffer))?;
        if count == 0 {
            return Ok(hash
                .finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect());
        }
        hash.update(&buffer[..count]);
    }
}

fn field<const N: usize>(bytes: &[u8], offset: usize) -> Option<[u8; N]> {
    bytes.get(offset..offset.checked_add(N)?)?.try_into().ok()
}

// ELF gABI, ELF64 header and section header layouts.
pub fn text_bytes(object: &[u8]) -> Option<&[u8]> {
    if !object.starts_with(b"\x7fELF\x02\x01\x01")
        || u16::from_le_bytes(field(object, 16)?) != 1
        || u16::from_le_bytes(field(object, 18)?)
            != if cfg!(target_arch = "aarch64") {
                183
            } else {
                62
            }
        || u16::from_le_bytes(field(object, 58)?) != 64
    {
        return None;
    }
    let start = usize::try_from(u64::from_le_bytes(field(object, 40)?)).ok()?;
    let count = usize::from(u16::from_le_bytes(field(object, 60)?));
    let names = usize::from(u16::from_le_bytes(field(object, 62)?));
    let sections = object
        .get(start..start.checked_add(64 * count)?)?
        .chunks_exact(64);
    let contents = |header: &[u8]| {
        let offset = usize::try_from(u64::from_le_bytes(field(header, 24)?)).ok()?;
        let size = usize::try_from(u64::from_le_bytes(field(header, 32)?)).ok()?;
        object.get(offset..offset.checked_add(size)?)
    };
    let strings = contents(sections.clone().nth(names)?)?;
    let mut text = None;
    for header in sections {
        let name = usize::try_from(u32::from_le_bytes(field(header, 0)?)).ok()?;
        if strings.get(name..)?.starts_with(b".text\0")
            && (u32::from_le_bytes(field(header, 4)?) != 1
                || text.replace(contents(header)?).is_some())
        {
            return None;
        }
    }
    text
}

#[cfg(not(test))]
mod corpus {
    use super::*;
    use std::ffi::OsStr;
    use std::os::unix::fs::symlink;

    const CORPUS: &str = "/usr/share/breenix/toolchain";
    const SYSROOT: &str = "/opt/breenix-dev/25.2";
    const CASES: &str = include_str!("../../../../toolchain/corpus/cases");
    #[cfg(target_arch = "x86_64")]
    const CC_CASES: &str = include_str!("../fixture/toolchain-cc-cases");
    #[cfg(target_arch = "aarch64")]
    const CC_CASES: &str = include_str!("../fixture/toolchain-cc-cases-aarch64");
    #[cfg(target_arch = "x86_64")]
    const BAS_PROGRAMS: &[&str] = &["sse-exit", "x87-exit"];
    #[cfg(target_arch = "aarch64")]
    const BAS_PROGRAMS: &[&str] = &["fp-exit", "call-exit"];

    pub fn exercise(run: &Path, recorded: &BTreeMap<String, String>) -> Result<(), String> {
        let table = ToolTable::stage(run, recorded)?;
        let result = (|| {
            // bcc locates headers, archives and the linker script relative to
            // its executable. Bind those inputs without importing bundle tools.
            for name in ["include", "include-support", "lib", "linker.ld"] {
                let path = run.join(name);
                at(&path, symlink(Path::new(SYSROOT).join(name), &path))?;
            }
            table.assembler()?;
            table.linker()?;
            table.compiler()?;
            table.project()
        })();
        let verified = table.verify_all();
        result.and(verified)?;
        if Instant::now() >= table.deadline {
            return Err(format!("rebuilt tools exceeded {} ms", WINDOW.as_millis()));
        }
        Ok(())
    }

    impl ToolTable {
        fn invoke(
            &self,
            tool: &str,
            args: impl IntoIterator<Item = impl AsRef<OsStr>>,
        ) -> Result<Output, String> {
            self.output(
                tool,
                Command::new(self.bin.join(tool))
                    .current_dir(&self.work)
                    .args(args),
            )
        }

        fn assemble(&self, name: &str) -> Result<Output, String> {
            let input = Path::new(CORPUS)
                .join("assembler")
                .join(format!("{name}.s"));
            let object = self.work.join(format!("{name}.o"));
            self.invoke("as", [Path::new("-o"), &object, &input])
        }

        fn assembler(&self) -> Result<(), String> {
            let dir = Path::new(CORPUS).join("assembler");
            success("as encodings", self.assemble("encodings")?)?;
            let object = self.work.join("encodings.o");
            let bytes = at(&object, fs::read(&object))?;
            let hex = dir.join("encodings.hex");
            let hex_table = at(&hex, fs::read_to_string(&hex))?;
            #[cfg(target_arch = "x86_64")]
            let expected = hex_table
                .split_ascii_whitespace()
                .map(|n| u8::from_str_radix(n, 16))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| format!("{}: {error}", hex.display()))?;
            #[cfg(target_arch = "aarch64")]
            let expected =
                hex_table
                    .lines()
                    .enumerate()
                    .try_fold(Vec::new(), |mut bytes, (index, row)| {
                        let word = row.split_once('#').map_or(row, |(word, _)| word).trim();
                        if word.len() != 8
                            || !word
                                .bytes()
                                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                        {
                            return Err(format!(
                                "{}:{}: expected an eight-digit instruction word",
                                hex.display(),
                                index + 1
                            ));
                        }
                        let word = u32::from_str_radix(word, 16)
                            .map_err(|error| format!("{}:{}: {error}", hex.display(), index + 1))?;
                        bytes.extend_from_slice(&word.to_le_bytes());
                        Ok(bytes)
                    })?;
            if expected.is_empty() || text_bytes(&bytes) != Some(expected.as_slice()) {
                return Err(format!(
                    "rebuilt as: .text differs from the {} encoding fixture",
                    if cfg!(target_arch = "aarch64") {
                        "Arm ARM"
                    } else {
                        "Intel SDM"
                    }
                ));
            }
            for &name in BAS_PROGRAMS {
                success(name, self.assemble(name)?)?;
                self.link_and_run(
                    name,
                    &[&self.work.join(format!("{name}.o"))],
                    &dir.join(format!("{name}.expect")),
                )?;
            }
            let bad = self.assemble("bad-input")?;
            let location = format!("{}/bad-input.s:2:", dir.display());
            if !bad.status.code().is_some_and(|code| code != 0)
                || !bad.stderr.starts_with(location.as_bytes())
                || self.work.join("bad-input.o").exists()
            {
                return Err(format!(
                    "rebuilt as: expected a located failure at {location}, got {}: {}",
                    bad.status,
                    String::from_utf8_lossy(&bad.stderr)
                ));
            }
            Ok(())
        }

        fn link(&self, name: &str, inputs: &[&Path]) -> Result<Output, String> {
            let script = Path::new(SYSROOT).join("linker.ld");
            let executable = self.work.join(format!("{name}.elf"));
            let libc = Path::new(SYSROOT).join("lib/libc.a");
            self.invoke(
                "ld",
                [
                    Path::new("-T"),
                    &script,
                    Path::new("--gc-sections"),
                    Path::new("-o"),
                    &executable,
                ]
                .into_iter()
                .chain(inputs.iter().copied())
                .chain([libc.as_path()]),
            )
        }

        fn link_and_run(&self, name: &str, inputs: &[&Path], expect: &Path) -> Result<(), String> {
            success(name, self.link(name, inputs)?)?;
            self.run_expected(&self.work.join(format!("{name}.elf")), expect)
        }

        fn linker(&self) -> Result<(), String> {
            let dir = Path::new(CORPUS);
            let object = |name| dir.join("objs").join(format!("{name}.o"));
            for name in CASES.lines() {
                self.link_and_run(name, &[&object(name)], &dir.join(format!("{name}.expect")))?;
            }
            let expected = dir.join("linker");
            self.link_and_run(
                "tls-template",
                &[&object("tls-template")],
                &expected.join("tls-template.expect"),
            )?;

            let main = object("got-main");
            let entry = object("got-entry");
            let value = object("archive-value-with-long-name");
            let archive = self.work.join("got.a");
            success(
                "ar rcs got.a",
                self.invoke("ar", [Path::new("rcs"), &archive, &entry, &value])?,
            )?;
            self.reindex(&archive)?;
            let symbols = success(
                "nm -Pg got.a",
                self.invoke("nm", [Path::new("-Pg"), &archive])?,
            )?;
            #[cfg(target_arch = "x86_64")]
            let wanted = format!(
                "{0}[got-entry.o]:\narchive_cell U\narchive_entry T 0 17\narchive_value U\n\
                 {0}[archive-value-with-long-name.o]:\narchive_cell R 0 4\narchive_value T 0 6\n",
                archive.display(),
            );
            #[cfg(target_arch = "aarch64")]
            let wanted = format!(
                "{0}[got-entry.o]:\narchive_cell U\narchive_entry T 0 20\narchive_value U\n\
                 {0}[archive-value-with-long-name.o]:\narchive_cell R 0 4\narchive_value T 0 8\n",
                archive.display(),
            );
            if symbols != wanted.as_bytes() {
                return Err(format!(
                    "rebuilt nm: unexpected archive symbols: {}",
                    String::from_utf8_lossy(&symbols)
                ));
            }
            self.link_and_run(
                "got-main",
                &[&main, &archive],
                &expected.join("got-main.expect"),
            )?;
            let copy = self.work.join("duplicate-entry.o");
            at(&copy, fs::copy(&entry, &copy))?;
            for (name, inputs, diagnostic) in [
                (
                    "undefined",
                    vec![main.as_path()],
                    format!("{}: .text+", main.display()),
                ),
                (
                    "duplicate",
                    vec![&main, &entry, &copy]
                        .into_iter()
                        .map(PathBuf::as_path)
                        .collect(),
                    format!(
                        "{}: archive_entry: duplicate definition (first defined in {})",
                        copy.display(),
                        entry.display()
                    ),
                ),
            ] {
                let result = self.link(name, &inputs)?;
                let stderr = String::from_utf8_lossy(&result.stderr);
                if result.status.code() != Some(1)
                    || self.work.join(format!("{name}.elf")).exists()
                    || !stderr.lines().any(|line| {
                        line.starts_with(&diagnostic)
                            && (name == "duplicate"
                                || (line.contains(if cfg!(target_arch = "aarch64") {
                                    "R_AARCH64_CALL26"
                                } else {
                                    "R_X86_64_PLT32"
                                }) && line.ends_with(" archive_entry: undefined symbol")))
                    })
                {
                    return Err(format!("rebuilt ld {name}: expected exit 1, no ELF and {diagnostic:?}; got {}: {stderr}", result.status));
                }
            }
            Ok(())
        }

        fn reindex(&self, archive: &Path) -> Result<(), String> {
            let original = at(archive, fs::read(archive))?;
            // Solaris ar(3HEAD): the first member of this fixture is the 32-bit index.
            const MAGIC: &[u8] = b"!<arch>\n";
            const DATA: usize = MAGIC.len() + 60;
            let index_size = original
                .strip_prefix(MAGIC)
                .and_then(|bytes| bytes.get(..60))
                .filter(|header| {
                    header.starts_with(b"/               ") && header.ends_with(b"`\n")
                })
                .and_then(|header| {
                    std::str::from_utf8(&header[48..58])
                        .ok()?
                        .trim_end()
                        .parse::<usize>()
                        .ok()
                })
                .filter(|&size| (8..=original.len() - DATA).contains(&size))
                .ok_or("rebuilt ar: missing or invalid archive index")?;
            let members = original
                .get(DATA + index_size + index_size % 2..)
                .ok_or("rebuilt ar: truncated index")?;
            let mut missing = MAGIC.to_vec();
            missing.extend_from_slice(members);
            let mut corrupt = original.clone();
            corrupt[DATA + 4..DATA + 8].fill(0);
            for (label, bytes) in [("missing", missing), ("corrupt", corrupt)] {
                if bytes == original {
                    return Err(format!("failed to construct {label} archive index fixture"));
                }
                at(archive, fs::write(archive, bytes))?;
                success("ar s got.a", self.invoke("ar", [Path::new("s"), archive])?)?;
                if at(archive, fs::read(archive))? != original {
                    return Err(format!("rebuilt ar: failed to restore the {label} index"));
                }
            }
            Ok(())
        }

        fn compiler(&self) -> Result<(), String> {
            let dir = Path::new(CORPUS);
            for name in CC_CASES.lines() {
                let executable = self.work.join("compiled.elf");
                let source = dir.join(format!("{name}.c"));
                success(
                    name,
                    self.invoke(
                        "cc",
                        [
                            Path::new("-B"),
                            &self.bin,
                            Path::new("-o"),
                            &executable,
                            &source,
                        ],
                    )?,
                )?;
                self.run_expected(&executable, &dir.join(format!("{name}.expect")))?;
                at(&executable, fs::remove_file(&executable))?;
            }
            Ok(())
        }

        fn project(&self) -> Result<(), String> {
            let dir = self.work.join("project");
            at(&dir, fs::create_dir(&dir))?;
            for file in [
                "Makefile",
                "main.c",
                "util.c",
                "util.h",
                "value.s",
                "project.expect",
            ] {
                let source = Path::new(CORPUS).join("project").join(file);
                at(&source, fs::copy(&source, dir.join(file)))?;
            }
            let args = [("CC", "cc"), ("AS", "as"), ("LD", "ld"), ("AR", "ar")]
                .map(|(key, tool)| format!("{key}={}", self.bin.join(tool).display()));
            let output = success(
                "make project",
                self.output(
                    "make",
                    Command::new(self.bin.join("make"))
                        .current_dir(&dir)
                        .args(args),
                )?,
            )?;
            let cc = self.bin.join("cc");
            let as_path = self.bin.join("as");
            let expected = format!(
                "{0} -c -o main.o main.c\n{0} -c -o util.o util.c\n{1} -o value.o value.s\n{0} -o project main.o util.o value.o\n",
                cc.display(), as_path.display(),
            );
            if output != expected.as_bytes() {
                return Err(format!(
                    "rebuilt make: unexpected project recipes: {}",
                    String::from_utf8_lossy(&output)
                ));
            }
            self.run_expected(&dir.join("project"), &dir.join("project.expect"))
        }

        fn run_expected(&self, executable: &Path, expect: &Path) -> Result<(), String> {
            let bytes = at(expect, fs::read(expect))?;
            let newline = bytes
                .iter()
                .position(|&b| b == b'\n')
                .ok_or_else(|| format!("{}: missing expected status", expect.display()))?;
            let code = std::str::from_utf8(&bytes[..newline])
                .ok()
                .and_then(|s| s.parse::<u8>().ok())
                .ok_or_else(|| format!("{}: invalid expected status", expect.display()))?;
            let result = self.run(Command::new(executable).current_dir(&self.work))?;
            if result.status.code() != Some(i32::from(code))
                || result.stdout != bytes[newline + 1..]
            {
                return Err(format!(
                    "{}: expected exit {code} and stdout {:?}, got {} and stdout {:?}",
                    expect.display(),
                    &bytes[newline + 1..],
                    result.status,
                    result.stdout
                ));
            }
            Ok(())
        }
    }

    fn success(label: &str, output: Output) -> Result<Vec<u8>, String> {
        if output.status.success() {
            Ok(output.stdout)
        } else {
            Err(format!(
                "{label}: {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ))
        }
    }
}

#[cfg(not(test))]
pub use corpus::exercise;
