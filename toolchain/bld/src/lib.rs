#![forbid(unsafe_code)]

//! Static AMD64 ELF64 links from relocatable objects and common archives.

mod input;
mod layout;
mod object;
mod relocate;
mod script;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use input::{FileSource, ReadSource};
use object::Link;
use script::Script;

type Result<T> = std::result::Result<T, String>;

const IMAGE_BASE: u64 = 0x4000_0000;
const PAGE_SIZE: u64 = 4096;

const USAGE: &str = "usage: bld -o OUTPUT [-T SCRIPT] [--gc-sections]\n\
       \x20      [--whole-archive | --no-whole-archive | --start-group |\n\
       \x20       --end-group | OBJECT | ARCHIVE] ...";

struct Input {
    path: PathBuf,
    whole_archive: bool,
}

#[derive(Default)]
struct Options {
    output: PathBuf,
    script: Option<PathBuf>,
    inputs: Vec<Vec<Input>>,
    gc: bool,
}

impl Options {
    fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self> {
        let mut args = args.into_iter();
        let mut options = Self::default();
        let mut group = None::<Vec<Input>>;
        let mut whole_archive = false;
        while let Some(arg) = args.next() {
            let value = match arg.as_encoded_bytes() {
                b"-o" if options.output.as_os_str().is_empty() => &mut options.output,
                b"-T" | b"--script" if options.script.is_none() => {
                    options.script.insert(PathBuf::new())
                }
                b"-o" | b"-T" | b"--script" => return Err(USAGE.into()),
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
                [] | [b'-', ..] => {
                    return Err(format!(
                        "bld: invalid link input or option: {}\n{USAGE}",
                        arg.to_string_lossy()
                    ));
                }
                _ => {
                    let input = Input {
                        path: arg.into(),
                        whole_archive,
                    };
                    if let Some(group) = &mut group {
                        group.push(input);
                    } else {
                        options.inputs.push(vec![input]);
                    }
                    continue;
                }
            };
            *value = args
                .next()
                .filter(|arg| !arg.is_empty())
                .ok_or(USAGE)?
                .into();
        }
        if group.is_some() || options.output.as_os_str().is_empty() || options.inputs.is_empty() {
            return Err(USAGE.into());
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

fn link(options: Options, source: &mut dyn ReadSource) -> Result<()> {
    let script = match options.script {
        Some(path) => Script::parse(&path, &input::read_file(&path, source)?)?,
        None => Script::default(),
    };
    let mut link = Link::new(script, options.gc);
    for group in options.inputs {
        link.include_group(&group, source)?;
    }
    link.select_live_sections();
    link.allocate_common()?;
    link.allocate_got()?;
    let mut image = layout::Image::new(&mut link)?;
    let entry = link.entry_address()?;
    link.relocate(&mut image)?;
    image.write_headers(&link, entry);
    input::write_executable(&options.output, &image.bytes).map_err(|error| {
        format!(
            "{}: cannot write executable: {error}",
            options.output.display()
        )
    })
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
