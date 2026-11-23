//! Command-line stages for the Breenix C toolchain.

use std::collections::HashSet;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{self, Command, ExitCode};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::pp::{self, MacroOption, Standard, Target};
use crate::{codegen, opt, sema};

#[path = "../../bas/src/default_target.rs"]
mod native_target;
pub use native_target::default_target;

const USAGE: &str = "usage: bcc [-E|-S|-c|-fsyntax-only|--dump-types] [-o FILE] \
    [-I DIR] [-nostdinc] [-include FILE] [-D NAME[=VALUE]] [-U NAME] [-L DIR] [-l NAME] [-B DIR] \
    [-MD [-MF FILE] | -Wp,-MD,FILE] [-std=c11|c17|gnu99|gnu11] \
    [-O0|-O1|-O2] [-fno-builtin] [-frounding-math] [-Wall] [-Werror] [-w] \
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
    AssemblyWithCpp,
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
    dependency_file: Option<PathBuf>,
    forwarded_dependency_file: Option<PathBuf>,
    tools: Option<PathBuf>,
    pp: pp::Options,
    no_standard_includes: bool,
    rounding_math: bool,
    optimize: bool,
    function_sections: bool,
    warnings_as_errors: bool,
    suppress_warnings: bool,
    libraries: Vec<PathBuf>,
    inputs: Vec<Input>,
}

impl Options {
    fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self> {
        let mut options = Self {
            stage: Stage::Link,
            output: None,
            dependency_file: None,
            forwarded_dependency_file: None,
            tools: None,
            pp: pp::Options {
                target: if default_target(env::consts::OS, env::consts::ARCH) == "aarch64" {
                    Target::Aarch64
                } else {
                    Target::X86_64
                },
                ..pp::Options::default()
            },
            no_standard_includes: false,
            rounding_math: false,
            optimize: false,
            function_sections: false,
            warnings_as_errors: false,
            suppress_warnings: false,
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
                    // GCC Overall Options describes .S as assembler input
                    // that runs through the preprocessor before assembly.
                    Some("S") => InputKind::AssemblyWithCpp,
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
                Some("-std=c11") => options.pp.standard = Standard::C11,
                Some("-std=c17") => options.pp.standard = Standard::C17,
                Some("-std=gnu99") => options.pp.standard = Standard::Gnu99,
                Some("-std=gnu11") => options.pp.standard = Standard::Gnu11,
                Some("-nostdinc") => options.no_standard_includes = true,
                Some("-MD") => options.pp.track_includes = true,
                Some(flag @ ("-MF" | "-include")) => {
                    let path = args
                        .next()
                        .map(|(_, value)| value)
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| usage(format!("{flag} requires a file")))?;
                    if flag == "-MF" {
                        options.dependency_file = Some(path.into());
                    } else {
                        options.pp.forced_includes.push(path.into());
                    }
                }
                Some(flag) if flag.starts_with("-Wp,-MD,") => {
                    let path = &flag["-Wp,-MD,".len()..];
                    if path.is_empty() || path.contains(',') {
                        return Err(usage("-Wp,-MD,FILE requires exactly one file".into()));
                    }
                    options.pp.track_includes = true;
                    options.forwarded_dependency_file = Some(path.into());
                }
                // Ordinary library names are already compiled as calls.
                Some("-fno-builtin" | "-fno-builtin-printf" | "-fno-builtin-strlen") => {}
                Some("-frounding-math") => options.rounding_math = true,
                Some("-Wall") => options.pp.warn_unknown_pragmas = true,
                Some("-Werror") => options.warnings_as_errors = true,
                Some("-w") => options.suppress_warnings = true,
                Some("-O0" | "-O1") => options.optimize = false,
                Some("-O2") => options.optimize = true,
                Some("-ffunction-sections") => options.function_sections = true,
                Some("-fno-function-sections") => options.function_sections = false,
                // Exact spellings from the staged BusyBox command receipts.
                // The scope and effects of these no-ops are in the spec ledger.
                Some(
                    "-Wshadow"
                    | "-Wwrite-strings"
                    | "-Wundef"
                    | "-Wstrict-prototypes"
                    | "-Wunused"
                    | "-Wunused-parameter"
                    | "-Wunused-function"
                    | "-Wunused-value"
                    | "-Wmissing-prototypes"
                    | "-Wmissing-declarations"
                    | "-Wno-format-security"
                    | "-Wdeclaration-after-statement"
                    | "-Wold-style-definition"
                    | "-Wno-string-plus-int"
                    | "-Wno-constant-logical-operand"
                    | "-fomit-frame-pointer"
                    | "-fdata-sections"
                    | "-funsigned-char"
                    | "-falign-functions=1"
                    | "-fno-unwind-tables"
                    | "-fno-asynchronous-unwind-tables"
                    | "-Oz",
                ) => {}
                Some("-E" | "-S" | "-c" | "-fsyntax-only" | "--dump-types") => {
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
        if options.dependency_file.is_some() && !options.pp.track_includes {
            return Err("bcc: -MF requires -MD or -Wp,-MD,FILE".into());
        }
        if options.pp.track_includes
            && (options.output.is_some()
                || options.dependency_file.is_some()
                || options.forwarded_dependency_file.is_some())
            && options
                .inputs
                .iter()
                .filter(|input| {
                    matches!(
                        input,
                        Input::File(
                            _,
                            InputKind::C | InputKind::Preprocessed | InputKind::AssemblyWithCpp
                        )
                    )
                })
                .count()
                > 1
        {
            return Err("bcc: a shared dependency output requires one C input".into());
        }
        if options.stage != Stage::Link {
            if options.output.is_some() && options.inputs.len() != 1 {
                return Err("bcc: -o with separate compilation requires one input".into());
            }
            for input in &options.inputs {
                let valid = match input {
                    Input::File(_, InputKind::C | InputKind::Preprocessed) => true,
                    Input::File(_, InputKind::Assembly) => options.stage == Stage::Object,
                    Input::File(_, InputKind::AssemblyWithCpp) => {
                        matches!(options.stage, Stage::Preprocess | Stage::Object)
                    }
                    _ => false,
                };
                if !valid {
                    return Err("bcc: input does not support the selected compilation stage".into());
                }
            }
        }
        Ok(options)
    }

    fn preprocess_output_is_dependency_file(&self) -> bool {
        self.stage == Stage::Preprocess
            && self.pp.track_includes
            && self.dependency_file.is_none()
            && self.forwarded_dependency_file.is_none()
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
        if !options.no_standard_includes {
            options
                .pp
                .system_dirs
                .extend([bundle.join("include-support"), bundle.join("include")]);
        }
        options.libraries.push(bundle.join("lib"));
        Ok(Self {
            tools: options.tools.take().unwrap_or_else(|| bin.to_path_buf()),
            bundle: bundle.to_path_buf(),
            options,
        })
    }

    fn compile(&self, path: &Path, kind: InputKind, stage: Stage) -> Result<Vec<u8>> {
        let mut errors = Vec::new();
        let mut warn = |warning: pp::Diagnostic| {
            if self.options.suppress_warnings {
                return;
            }
            if self.options.warnings_as_errors {
                errors.push(warning.to_string());
            } else {
                eprintln!("{}", warning.as_warning());
            }
        };
        let input = match kind {
            InputKind::Preprocessed => pp::read_preprocessed(path, self.options.pp.standard),
            _ => pp::preprocess_with_warnings(path, &self.options.pp, &mut warn),
        };
        let input = match input {
            Ok(input) if errors.is_empty() => input,
            Ok(_) => return Err(errors.join("\n")),
            Err(error) => {
                errors.push(error.to_string());
                return Err(errors.join("\n"));
            }
        };
        if self.options.pp.track_includes {
            self.write_dependencies(path, &input.includes)?;
        }
        let mut bytes = Vec::new();
        if matches!(kind, InputKind::AssemblyWithCpp) {
            input
                .write_assembly(&mut bytes)
                .map_err(|error| error.to_string())?;
            return Ok(bytes);
        }
        if stage == Stage::Preprocess {
            input.write(&mut bytes).map_err(|error| error.to_string())?;
            return Ok(bytes);
        }
        let target = self.options.pp.target;
        let mut unit =
            sema::analyze_with_rounding_math(&input, path, target, self.options.rounding_math)
                .map_err(|error| error.to_string())?;
        match stage {
            Stage::Syntax => Ok(bytes),
            Stage::Types => {
                unit.write_types(&mut bytes)
                    .map_err(|error| error.to_string())?;
                Ok(bytes)
            }
            _ => {
                if self.options.optimize {
                    opt::run(&mut unit, target);
                }
                codegen::emit_with_options(&unit, target, codegen::Options {
                    optimize: self.options.optimize,
                    function_sections: self.options.function_sections,
                })
                    .map(String::into_bytes)
                    .map_err(|error| error.to_string())
            }
        }
    }

    fn write_dependencies(&self, source: &Path, includes: &[PathBuf]) -> Result<()> {
        let options = &self.options;
        let output = options.output.as_deref();
        let default_target = Path::new(source.file_name().unwrap()).with_extension("o");
        let target = output
            .filter(|_| options.stage != Stage::Preprocess)
            .unwrap_or(&default_target);
        let default_depfile = target.with_extension("d");
        let depfile = options
            .dependency_file
            .as_deref()
            .or(options.forwarded_dependency_file.as_deref())
            .or(output.filter(|_| options.preprocess_output_is_dependency_file()))
            .unwrap_or(&default_depfile);
        if (output == Some(depfile) && !options.preprocess_output_is_dependency_file())
            || depfile == source
            || includes.iter().any(|header| header == depfile)
        {
            return Err(format!(
                "bcc: dependency file '{}' overlaps an input or output",
                depfile.display()
            ));
        }

        let mut rule = String::new();
        make_path(&mut rule, target)?;
        rule.push(':');
        let mut seen = HashSet::new();
        for path in std::iter::once(source).chain(includes.iter().map(PathBuf::as_path)) {
            if seen.insert(path) {
                rule.push(' ');
                make_path(&mut rule, path)?;
            }
        }
        rule.push('\n');
        write_output(Some(depfile), rule.as_bytes())
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
                .arg("--target")
                .arg(match self.options.pp.target {
                    Target::X86_64 => "x86_64",
                    Target::Aarch64 => "aarch64",
                })
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
            let runtime = self.bundle.join("lib/libbrt.a");
            if self.options.pp.target == Target::Aarch64 || runtime.is_file() {
                linker.arg(runtime);
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
                let output =
                    output.filter(|_| !self.options.preprocess_output_is_dependency_file());
                write_output(output, &bytes)?;
            }
        }
        Ok(())
    }
}

fn make_path(rule: &mut String, path: &Path) -> Result<()> {
    let name = path
        .to_str()
        .filter(|name| !name.contains(['\n', '\r']))
        .ok_or_else(|| {
            format!(
                "bcc: cannot represent '{}' in a dependency rule",
                path.display()
            )
        })?;
    let mut backslashes = 0;
    for ch in name.chars() {
        if ch == '\\' {
            backslashes += 1;
            continue;
        }
        let escape = matches!(ch, ' ' | '\t' | '#' | ':');
        rule.extend(std::iter::repeat_n(
            '\\',
            if escape {
                2 * backslashes + 1
            } else {
                backslashes
            },
        ));
        if ch == '$' {
            rule.push('$');
        }
        rule.push(ch);
        backslashes = 0;
    }
    rule.extend(std::iter::repeat_n('\\', 2 * backslashes));
    Ok(())
}

fn write_output(path: Option<&Path>, bytes: &[u8]) -> Result<()> {
    if let Some(path) = path.filter(|path| *path != Path::new("-")) {
        write_file(path, bytes)
    } else {
        io::stdout()
            .lock()
            .write_all(bytes)
            .map_err(|error| format!("bcc: cannot write stdout: {error}"))
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
