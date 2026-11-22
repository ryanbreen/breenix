//! Command-line stages for the Breenix C toolchain.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{self, Command, ExitCode};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::pp::{self, MacroOption, Target};
use crate::{codegen, sema};

const USAGE: &str = "usage: bcc [-E|-S|-c|-fsyntax-only|--dump-types] [-o FILE] \
    [-I DIR] [-D NAME[=VALUE]] [-U NAME] [-L DIR] [-l NAME] [-B DIR] \
    [--target=x86_64|aarch64] INPUT ...";

type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Preprocess,
    Syntax,
    Types,
    Assembly,
    Object,
    Link,
}

#[derive(Clone, Copy)]
enum InputKind {
    C,
    Preprocessed,
    Assembly,
    Object,
    Archive,
}

enum Input {
    File(PathBuf, InputKind),
    Library(OsString),
}

struct Options {
    stage: Stage,
    output: Option<PathBuf>,
    tools: Option<PathBuf>,
    pp: pp::Options,
    libraries: Vec<PathBuf>,
    inputs: Vec<Input>,
}

impl Options {
    fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self> {
        let mut options = Self {
            stage: Stage::Link,
            output: None,
            tools: None,
            pp: pp::Options::default(),
            libraries: Vec::new(),
            inputs: Vec::new(),
        };
        let mut args = args.into_iter().enumerate();
        let mut positional = false;
        while let Some((index, arg)) = args.next() {
            let usage =
                |message: String| format!("<command-line>:1:{}: error: {message}", index + 1);
            if positional || !arg.as_encoded_bytes().starts_with(b"-") {
                let path = PathBuf::from(arg);
                let kind = match path.extension().and_then(|ext| ext.to_str()) {
                    Some("c") => InputKind::C,
                    Some("i") => InputKind::Preprocessed,
                    Some("s") => InputKind::Assembly,
                    Some("o") => InputKind::Object,
                    Some("a") => InputKind::Archive,
                    _ => return Err(usage(format!("unknown input type '{}'", path.display()))),
                };
                options.inputs.push(Input::File(path, kind));
                continue;
            }
            match arg.to_str() {
                Some("--") => positional = true,
                Some("--target=x86_64") => options.pp.target = Target::X86_64,
                Some("--target=aarch64") => options.pp.target = Target::Aarch64,
                Some("-E" | "-S" | "-c" | "-fsyntax-only" | "--dump-types" | "-O0") => {
                    let mut stage = match arg.to_str() {
                        Some("-E") => Stage::Preprocess,
                        Some("-S") => Stage::Assembly,
                        Some("-c") => Stage::Object,
                        Some("-fsyntax-only") => Stage::Syntax,
                        Some("--dump-types") => Stage::Types,
                        _ => continue,
                    };
                    if matches!(
                        (options.stage, stage),
                        (Stage::Syntax, Stage::Types) | (Stage::Types, Stage::Syntax)
                    ) {
                        stage = Stage::Types;
                    } else if options.stage != Stage::Link && options.stage != stage {
                        return Err(usage("conflicting compilation modes".into()));
                    }
                    options.stage = stage;
                }
                _ => {
                    let unknown = || usage(format!("unknown option '{}'", arg.to_string_lossy()));
                    let Some(flag @ (b'o' | b'I' | b'D' | b'U' | b'L' | b'l' | b'B')) =
                        arg.as_encoded_bytes().get(1).copied()
                    else {
                        return Err(unknown());
                    };
                    let value = if arg.len() == 2 {
                        args.next().map(|(_, value)| value)
                    } else {
                        arg.to_str().map(|text| OsString::from(&text[2..]))
                    }
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| usage(format!("-{} requires a value", char::from(flag))))?;
                    match flag {
                        b'o' => {
                            if options.output.replace(value.into()).is_some() {
                                return Err(usage("duplicate -o".into()));
                            }
                        }
                        b'B' => options.tools = Some(value.into()),
                        b'I' => options.pp.include_dirs.push(value.into()),
                        b'D' | b'U' => {
                            let value = value.into_string().map_err(|_| {
                                usage(format!("-{} requires UTF-8", char::from(flag)))
                            })?;
                            options.pp.macros.push(if flag == b'D' {
                                MacroOption::Define(value)
                            } else {
                                MacroOption::Undefine(value)
                            });
                        }
                        b'L' => options.libraries.push(value.into()),
                        b'l' => options.inputs.push(Input::Library(value)),
                        _ => unreachable!(),
                    }
                }
            }
        }
        if options.inputs.is_empty() {
            return Err("bcc: expected an input file".into());
        }
        if options.stage != Stage::Link {
            if options.output.is_some() && options.inputs.len() != 1 {
                return Err("bcc: -o with separate compilation requires one input".into());
            }
            for input in &options.inputs {
                let valid = match input {
                    Input::File(_, InputKind::C | InputKind::Preprocessed) => true,
                    Input::File(_, InputKind::Assembly) => options.stage == Stage::Object,
                    _ => false,
                };
                if !valid {
                    return Err("bcc: input does not support the selected compilation stage".into());
                }
            }
        }
        Ok(options)
    }
}

struct Driver {
    options: Options,
    bundle: PathBuf,
    tools: PathBuf,
}

impl Driver {
    fn new(mut options: Options) -> Result<Self> {
        let executable = env::current_exe().map_err(|error| format!("bcc: {error}"))?;
        let bin = executable
            .parent()
            .ok_or("bcc: cannot locate executable directory")?;
        let bundle = bin.parent().ok_or("bcc: cannot locate bundle directory")?;
        options
            .pp
            .system_dirs
            .extend([bundle.join("include-support"), bundle.join("include")]);
        options.libraries.push(bundle.join("lib"));
        Ok(Self {
            tools: options.tools.take().unwrap_or_else(|| bin.to_path_buf()),
            bundle: bundle.to_path_buf(),
            options,
        })
    }

    fn compile(&self, path: &Path, kind: InputKind, stage: Stage) -> Result<Vec<u8>> {
        let input = match kind {
            InputKind::Preprocessed => pp::read_preprocessed(path),
            _ => pp::preprocess(path, &self.options.pp),
        }
        .map_err(|error| error.to_string())?;
        let mut bytes = Vec::new();
        if stage == Stage::Preprocess {
            input.write(&mut bytes).map_err(|error| error.to_string())?;
            return Ok(bytes);
        }
        let target = self.options.pp.target;
        let unit = sema::analyze(&input, path, target).map_err(|error| error.to_string())?;
        match stage {
            Stage::Syntax => Ok(bytes),
            Stage::Types => {
                unit.write_types(&mut bytes)
                    .map_err(|error| error.to_string())?;
                Ok(bytes)
            }
            _ => codegen::emit_assembly(&unit, path, target)
                .map(String::into_bytes)
                .map_err(|error| error.to_string()),
        }
    }

    fn object(&self, path: &Path, kind: InputKind, output: &Path, work: &Path) -> Result<()> {
        let assembly = if matches!(kind, InputKind::Assembly) {
            input_path(path)
        } else {
            let file = work.join("input.s");
            write_file(&file, &self.compile(path, kind, Stage::Assembly)?)?;
            file
        };
        run_tool(
            Command::new(self.tools.join("as"))
                .arg("-o")
                .arg(output)
                .arg(assembly),
        )
    }

    fn library(&self, name: &OsString) -> Result<PathBuf> {
        let path = Path::new(name);
        if path.file_name() != Some(name.as_os_str()) {
            return Err(format!(
                "bcc: invalid library name '{}'",
                name.to_string_lossy()
            ));
        }
        let mut file = OsString::from("lib");
        file.push(name);
        file.push(".a");
        self.options
            .libraries
            .iter()
            .map(|dir| dir.join(&file))
            .find(|path| path.is_file())
            .map(|path| input_path(&path))
            .ok_or_else(|| format!("bcc: cannot find -l{}", name.to_string_lossy()))
    }

    fn execute(&self) -> Result<()> {
        let stage = self.options.stage;
        if self.options.pp.target == Target::Aarch64 && matches!(stage, Stage::Object | Stage::Link)
        {
            return Err("bcc: error: assembly and linking for aarch64 are not implemented".into());
        }
        let output = self.options.output.as_deref();
        let work = matches!(stage, Stage::Object | Stage::Link)
            .then(WorkDir::create)
            .transpose()?;
        if stage == Stage::Link {
            let work = &work.as_ref().unwrap().0;
            let mut linker = Command::new(self.tools.join("ld"));
            linker
                .arg("-T")
                .arg(self.bundle.join("linker.ld"))
                .arg("--gc-sections")
                .arg("-o")
                .arg(output.unwrap_or(Path::new("a.out")));
            for (index, input) in self.options.inputs.iter().enumerate() {
                let object = match input {
                    Input::File(path, InputKind::Object | InputKind::Archive) => input_path(path),
                    Input::File(path, kind) => {
                        let object = work.join(format!("{index}.o"));
                        self.object(path, *kind, &object, work)?;
                        object
                    }
                    Input::Library(name) => self.library(name)?,
                };
                linker.arg(object);
            }
            return run_tool(linker.arg(self.bundle.join("lib/libc.a")));
        }

        for input in &self.options.inputs {
            let Input::File(path, kind) = input else {
                unreachable!()
            };
            if stage == Stage::Object {
                let default_output = Path::new(path.file_name().unwrap()).with_extension("o");
                self.object(
                    path,
                    *kind,
                    output.unwrap_or(&default_output),
                    &work.as_ref().unwrap().0,
                )?;
            } else {
                let bytes = self.compile(path, *kind, stage)?;
                if stage == Stage::Syntax {
                    continue;
                }
                if let Some(path) = output.filter(|path| *path != Path::new("-")) {
                    write_file(path, &bytes)?;
                } else {
                    io::stdout()
                        .lock()
                        .write_all(&bytes)
                        .map_err(|error| format!("bcc: cannot write stdout: {error}"))?;
                }
            }
        }
        Ok(())
    }
}

fn input_path(path: &Path) -> PathBuf {
    if path.as_os_str().as_encoded_bytes().starts_with(b"-") {
        Path::new(".").join(path)
    } else {
        path.to_path_buf()
    }
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::write(path, bytes).map_err(|error| format!("{}: {error}", path.display()))
}

fn run_tool(command: &mut Command) -> Result<()> {
    let tool = command.get_program().to_string_lossy().into_owned();
    let status = command
        .status()
        .map_err(|error| format!("{tool}: {error}"))?;
    if !status.success() {
        return Err(format!("{tool}: {status}"));
    }
    Ok(())
}

struct WorkDir(PathBuf);

impl WorkDir {
    fn create() -> Result<Self> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        for _ in 0..100 {
            let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir = env::temp_dir().join(format!("bcc-{}-{sequence}", process::id()));
            match builder.create(&dir) {
                Ok(()) => return Ok(Self(dir)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(format!("{}: {error}", dir.display())),
            }
        }
        Err("bcc: cannot create temporary directory".into())
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Run the compiler driver with arguments excluding the executable name.
pub fn run(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let options = match Options::parse(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match Driver::new(options).and_then(|driver| driver.execute()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
