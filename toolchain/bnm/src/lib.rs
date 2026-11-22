#![forbid(unsafe_code)]

//! Inspect static ELF symbols per ELF gABI §§3 and 5, including archive members.

use bobj::ar::{self, Archive};
use bobj::elf::{
    Elf, Symbol, SHF_ALLOC, SHF_EXECINSTR, SHF_WRITE, SHN_ABS, SHN_COMMON, SHN_UNDEF, SHT_NOBITS,
    STB_LOCAL, STB_WEAK, STT_FILE, STT_OBJECT, STT_SECTION,
};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Write};
use std::process::ExitCode;

#[derive(Default)]
struct Options {
    external: bool,
    undefined: bool,
    prefix: bool,
    portable: bool,
}

fn classify(file: &Elf<'_>, symbol: Symbol<'_>) -> Result<char, bobj::Error> {
    if symbol.binding == STB_WEAK {
        let kind = if symbol.kind == STT_OBJECT { 'V' } else { 'W' };
        return Ok(if symbol.section == SHN_UNDEF {
            kind.to_ascii_lowercase()
        } else {
            kind
        });
    }
    let kind = match symbol.section {
        SHN_UNDEF => 'U',
        SHN_ABS => 'A',
        SHN_COMMON => 'C',
        index => {
            let section = file.section(usize::from(index))?;
            if section.flags & SHF_ALLOC == 0 {
                '?'
            } else if section.flags & SHF_EXECINSTR != 0 {
                'T'
            } else if section.kind == SHT_NOBITS {
                'B'
            } else if section.flags & SHF_WRITE != 0 {
                'D'
            } else {
                'R'
            }
        }
    };
    Ok(if symbol.binding == STB_LOCAL {
        kind.to_ascii_lowercase()
    } else {
        kind
    })
}

fn print_elf(
    out: &mut impl Write,
    options: &Options,
    label: &[u8],
    heading: bool,
    bytes: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let file = Elf::parse(bytes)?;
    let Some(table) = file.static_symbols()? else {
        return Ok(());
    };
    let mut symbols = Vec::new();
    for index in 1..table.len() {
        let symbol = table.get(index)?;
        if !symbol.name.is_empty()
            && !matches!(symbol.kind, STT_FILE | STT_SECTION)
            && (!options.external || symbol.binding != STB_LOCAL)
            && (!options.undefined || symbol.section == SHN_UNDEF)
        {
            symbols.push(symbol);
        }
    }
    symbols.sort_by_key(|symbol| (symbol.name, symbol.section, symbol.value));
    if heading && !options.prefix {
        out.write_all(label)?;
        writeln!(out, ":")?;
    }
    for symbol in symbols {
        if options.prefix {
            out.write_all(label)?;
            write!(out, ": ")?;
        }
        let kind = classify(&file, symbol)?;
        let defined = symbol.section != SHN_UNDEF;
        if options.portable {
            out.write_all(symbol.name)?;
            write!(out, " {kind}")?;
            if defined {
                write!(out, " {:x} {:x}", symbol.value, symbol.size)?;
            }
        } else {
            if defined {
                write!(out, "{:016x} {kind} ", symbol.value)?;
            } else {
                write!(out, "                 {kind} ")?;
            }
            out.write_all(symbol.name)?;
        }
        writeln!(out)?;
    }
    Ok(())
}

fn inspect(out: &mut impl Write, options: &Options, path: &OsStr, heading: bool) -> bool {
    let diagnostic = |label: &[u8], error: &dyn std::fmt::Display| {
        eprintln!("bnm: {}: {error}", String::from_utf8_lossy(label));
        false
    };
    let label = path.as_encoded_bytes();
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => return diagnostic(label, &error),
    };
    if !ar::has_magic(&bytes) {
        return match print_elf(out, options, label, heading, &bytes) {
            Ok(()) => true,
            Err(error) => diagnostic(label, &error),
        };
    }
    let archive = match Archive::parse(&bytes) {
        Ok(archive) => archive,
        Err(error) => return diagnostic(label, &error),
    };
    let mut ok = true;
    for member in archive.members {
        let mut member_label = label.to_vec();
        member_label.push(b'[');
        member_label.extend_from_slice(member.name);
        member_label.push(b']');
        if let Err(error) = print_elf(out, options, &member_label, true, member.bytes) {
            ok = diagnostic(&member_label, &error);
        }
    }
    ok
}

fn parse_options(args: &[OsString]) -> Option<(Options, &[OsString])> {
    let mut options = Options::default();
    for (index, arg) in args.iter().enumerate() {
        let flags = arg.as_encoded_bytes();
        if flags == b"--" {
            return (index + 1 < args.len()).then_some((options, &args[index + 1..]));
        }
        if flags.len() < 2 || flags[0] != b'-' {
            return Some((options, &args[index..]));
        }
        for flag in &flags[1..] {
            match flag {
                b'A' => options.prefix = true,
                b'g' => options.external = true,
                b'P' => options.portable = true,
                b'u' => options.undefined = true,
                _ => return None,
            }
        }
    }
    None
}

pub fn run(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let args: Vec<_> = args.into_iter().collect();
    let Some((options, inputs)) = parse_options(&args) else {
        eprintln!("usage: bnm [-AgPu] file ...");
        return ExitCode::from(2);
    };
    let mut out = io::BufWriter::new(io::stdout().lock());
    let mut ok = true;
    for input in inputs {
        ok &= inspect(&mut out, &options, input, inputs.len() > 1);
    }
    if let Err(error) = out.flush() {
        eprintln!("bnm: stdout: {error}");
        ok = false;
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
