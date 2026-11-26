#![forbid(unsafe_code)]

//! Static and relocatable ELF64 links from objects and common archives.

mod eh_frame;
mod input;
mod layout;
mod object;
mod relocatable;
mod relocate;
mod script;

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use bobj::elf::{aarch64::EM_AARCH64, EM_X86_64};
use input::{FileSource, ReadSource};
use object::{Definition, Link};
use script::Script;

type Result<T> = std::result::Result<T, String>;

const IMAGE_BASE: u64 = 0x4000_0000;
const PAGE_SIZE: u64 = 4096;

#[derive(Clone, Copy, Eq, PartialEq)]
enum Machine {
    Amd64,
    Aarch64,
}

impl Machine {
    fn from_elf(machine: u16) -> Option<Self> {
        match machine {
            EM_X86_64 => Some(Self::Amd64),
            EM_AARCH64 => Some(Self::Aarch64),
            _ => None,
        }
    }

    fn elf(self) -> u16 {
        match self {
            Self::Amd64 => EM_X86_64,
            Self::Aarch64 => EM_AARCH64,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Amd64 => "AMD64",
            Self::Aarch64 => "AArch64",
        }
    }
}

const USAGE: &str = "usage: bld -o OUTPUT [-nostdlib] [-r | [-T SCRIPT] [--gc-sections] [--eh-frame-hdr]] [-Map FILE]\n\
       \x20      [-L DIRECTORY] [-Bstatic | -Bdynamic] [-l NAME | -l:FILE]\n\
       \x20      [--whole-archive | --no-whole-archive | --start-group |\n\
       \x20       --end-group | OBJECT | ARCHIVE] ...";

struct Input {
    name: InputName,
    whole_archive: bool,
    static_only: bool,
    as_needed: bool,
}

enum InputName {
    File(PathBuf),
    Library(OsString),
}

#[derive(Default)]
struct Options {
    output: PathBuf,
    script: Option<(PathBuf, usize)>,
    map: Option<PathBuf>,
    inputs: Vec<Vec<Input>>,
    search: Vec<PathBuf>,
    gc: bool,
    eh_frame_hdr: bool,
    relocatable: bool,
    allow_multiple_definition: bool,
    executable_symbols: bool,
}

impl Options {
    fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self> {
        let mut args = args.into_iter();
        let mut options = Self::default();
        let mut group = None::<Vec<Input>>;
        let mut whole_archive = false;
        let mut static_only = false;
        let mut as_needed = false;
        while let Some(arg) = args.next() {
            // GNU ld §2.1 allows attached operands for these short options.
            let (flag, attached) = match arg.as_encoded_bytes() {
                [b'-', b'L' | b'l' | b'T', operand @ ..] if !operand.is_empty() => (
                    &arg.as_encoded_bytes()[..2],
                    Some(OsStr::from_bytes(operand)),
                ),
                flag => (flag, None),
            };
            let mut operand = || {
                attached
                    .map(OsStr::to_os_string)
                    .or_else(|| args.next())
                    .filter(|arg| !arg.is_empty())
                    .ok_or(USAGE)
            };
            let value = match flag {
                b"-o" if options.output.as_os_str().is_empty() => &mut options.output,
                b"-T" | b"--script" if options.script.is_none() => {
                    let prior_search_paths = options.search.len();
                    &mut options
                        .script
                        .insert((PathBuf::new(), prior_search_paths))
                        .0
                }
                // GNU ld 2.45 §2.1 names the map option. bld writes
                // selected symbol, binding and input path as TSV.
                b"-Map" if options.map.is_none() => options.map.insert(PathBuf::new()),
                b"-o" | b"-T" | b"--script" | b"-Map" => return Err(USAGE.into()),
                b"-r" | b"--relocatable" => {
                    options.relocatable = true;
                    continue;
                }
                // GNU ld 2.45 §2.1 disables implicit library search.
                // bld has no implicit library search directories.
                b"-nostdlib" => continue,
                b"-L" => {
                    options.search.push(operand()?.into());
                    continue;
                }
                b"-Bstatic" | b"-Bdynamic" => {
                    static_only = flag == b"-Bstatic";
                    continue;
                }
                b"--as-needed" | b"--no-as-needed" => {
                    as_needed = flag == b"--as-needed";
                    continue;
                }
                b"--allow-multiple-definition" => {
                    options.allow_multiple_definition = true;
                    continue;
                }
                // rustc selects the GNU flavor of a multi-flavor linker.
                b"-flavor" if operand()? == "gnu" => {
                    options.executable_symbols = true;
                    continue;
                }
                // Executables already have a non-executable PT_GNU_STACK.
                b"-z" if operand()? == "noexecstack" => continue,
                // GNU ld §2.1 applies this optimization to shared output.
                b"-O1" => continue,
                b"--strip-debug" => {
                    options.executable_symbols = true;
                    continue;
                }
                b"--eh-frame-hdr" => {
                    options.eh_frame_hdr = true;
                    continue;
                }
                b"--gc-sections" => {
                    options.gc = true;
                    continue;
                }
                b"--no-gc-sections" => {
                    options.gc = false;
                    continue;
                }
                b"--whole-archive" => {
                    whole_archive = true;
                    continue;
                }
                b"--no-whole-archive" => {
                    whole_archive = false;
                    continue;
                }
                b"--start-group" | b"-(" if group.is_none() => {
                    group = Some(Vec::new());
                    continue;
                }
                b"--end-group" | b"-)" if group.is_some() => {
                    options
                        .inputs
                        .extend(group.take().filter(|items| !items.is_empty()));
                    continue;
                }
                b"--start-group" | b"-(" | b"--end-group" | b"-)" => return Err(USAGE.into()),
                [] | [b'-', ..] if flag != b"-l" => {
                    return Err(format!(
                        "bld: invalid link input or option: {}\n{USAGE}",
                        arg.to_string_lossy()
                    ));
                }
                _ => {
                    let input = Input {
                        name: if flag == b"-l" {
                            InputName::Library(operand()?)
                        } else {
                            InputName::File(arg.into())
                        },
                        whole_archive,
                        static_only,
                        as_needed,
                    };
                    if let Some(group) = &mut group {
                        group.push(input);
                    } else {
                        options.inputs.push(vec![input]);
                    }
                    continue;
                }
            };
            *value = operand()?.into();
        }
        if group.is_some() || options.output.as_os_str().is_empty() || options.inputs.is_empty() {
            return Err(USAGE.into());
        }
        if options.map.as_ref() == Some(&options.output) {
            return Err("bld: map and link output must have different paths".into());
        }
        if options.relocatable && (options.script.is_some() || options.gc) {
            return Err("bld: -r cannot be combined with -T or --gc-sections".into());
        }
        if options.relocatable && options.eh_frame_hdr {
            return Err("bld: -r cannot be combined with --eh-frame-hdr".into());
        }
        Ok(options)
    }
}

fn align_up(value: u64, align: u64) -> Option<u64> {
    let mask = align.saturating_sub(1);
    value.checked_add(value.wrapping_neg() & mask)
}

fn display(bytes: &[u8]) -> std::borrow::Cow<'_, str> {
    String::from_utf8_lossy(bytes)
}

fn write_map(path: &Path, link: &Link, relocatable: bool) -> io::Result<()> {
    let mut out = BufWriter::new(File::create(path)?);
    let mut globals: Vec<_> = link.globals.iter().collect();
    globals.sort_unstable_by_key(|(name, _)| *name);
    for (name, global) in globals {
        let binding = match global.definition {
            Definition::Weak => "weak",
            Definition::Strong => "strong",
            Definition::Common => "common",
            Definition::Undefined => continue,
        };
        let (input, symbol) = global.origin;
        let object = &link.objects[input];
        let section = usize::from(object.symbols[symbol].section);
        if !relocatable
            && object
                .sections
                .get(section)
                .is_some_and(|section| !section.live)
        {
            continue;
        }
        out.write_all(name)?;
        writeln!(out, "\t{binding}\t{}", object.path)?;
    }
    out.flush()
}

fn link(options: Options, source: &mut dyn ReadSource) -> Result<()> {
    let script = match options.script {
        Some((path, prior_search_paths)) => {
            let path = input::find_script(path, &options.search[..prior_search_paths])?;
            Script::parse(&path, &input::read_file(&path, source)?)?
        }
        None => Script::default(),
    };
    let mut link = Link::new(
        script,
        options.gc,
        options.relocatable,
        options.allow_multiple_definition,
    );
    for group in options.inputs {
        link.include_group(&group, &options.search, source)?;
    }
    let (bytes, mode, kind) = if options.relocatable {
        (
            link.relocatable_object(&options.output)?,
            0o666,
            "relocatable object",
        )
    } else {
        let frames = options
            .eh_frame_hdr
            .then(|| eh_frame::Frames::read(&mut link))
            .transpose()?;
        link.select_live_sections(frames.as_ref());
        let index = frames
            .map(|frames| frames.compact(&mut link))
            .transpose()?
            .flatten();
        link.allocate_common()?;
        link.allocate_got()?;
        let mut image = layout::Image::new(&mut link, index.as_ref())?;
        let entry = link.entry_address()?;
        link.relocate(&mut image)?;
        if let Some(index) = index {
            index.write(&link, &mut image)?;
        }
        image.write_headers(&link, entry);
        if options.executable_symbols {
            link.write_executable_symbols(&mut image.bytes)?;
        }
        (image.bytes, 0o777, "executable")
    };
    if let Some(path) = options.map {
        write_map(&path, &link, options.relocatable)
            .map_err(|error| format!("{}: cannot write link map: {error}", path.display()))?;
    }
    input::write_output(&options.output, &bytes, mode)
        .map_err(|error| format!("{}: cannot write {kind}: {error}", options.output.display()))
}

fn run_with(args: impl IntoIterator<Item = OsString>, source: &mut dyn ReadSource) -> ExitCode {
    match Options::parse(args).and_then(|options| link(options, source)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

/// Link the input files named by the command-line arguments (without argv[0]).
pub fn run(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    run_with(args, &mut FileSource)
}

/// Run the linker with the read-fault source used by the host harness.
#[cfg(feature = "read-fault-probe")]
pub fn run_read_probe(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    run_with(args, &mut input::FaultSource::default())
}
