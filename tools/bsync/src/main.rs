use bsync::remote::Destination;
use bsync::tree::{plan, scan, Action, Entry, Excludes};
use bsync::Result;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str =
    "usage: bsync [--dry-run] [--delete] [--sudo] [--exclude GLOB ...] SRC USER@HOST:DEST";

fn run() -> Result<()> {
    let mut dry_run = false;
    let mut delete = false;
    let mut sudo = false;
    let mut patterns = Vec::new();
    let mut operands = Vec::new();
    let mut positional = false;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match (positional, arg.to_str()) {
            (false, Some("-h" | "--help")) => {
                println!("{USAGE}");
                return Ok(());
            }
            (false, Some("--")) => positional = true,
            (false, Some("--dry-run")) => dry_run = true,
            (false, Some("--delete")) => delete = true,
            (false, Some("--sudo")) => sudo = true,
            (false, Some("--exclude")) => {
                let pattern = args
                    .next()
                    .and_then(|arg| arg.into_string().ok())
                    .ok_or("--exclude requires a UTF-8 glob")?;
                patterns.push(pattern);
            }
            (false, Some(flag)) if flag.starts_with('-') => {
                return Err(format!("unknown option {flag:?}\n{USAGE}").into());
            }
            _ => operands.push(arg),
        }
    }
    let [source, remote] = operands.as_slice() else {
        return Err(USAGE.into());
    };
    let destination = Destination::parse(remote.to_str().ok_or("destination must be UTF-8")?)?;
    let excludes = Excludes::new(&patterns)?;
    let source: PathBuf = std::path::Path::new(source).components().collect();
    let local = scan(&source, &excludes)?;
    let listing = destination.inventory(sudo)?;
    let actions = plan(&local, &listing.entries, &excludes, delete)?;

    {
        let mut stdout = io::stdout().lock();
        if actions.is_empty() {
            writeln!(stdout, "No changes.")?;
        }
        for action in &actions {
            let path = if action.path().is_empty() {
                "."
            } else {
                action.path()
            };
            match action {
                Action::FileMode(relative) | Action::DirMode(relative) => {
                    let mode = match local[relative] {
                        Entry::Dir(mode) | Entry::File { mode, .. } => mode,
                        Entry::Other => unreachable!(),
                    };
                    writeln!(stdout, "CHMOD {path:?} {mode:03o}")?;
                }
                _ => writeln!(stdout, "{} {path:?}", action.label())?,
            }
        }
        stdout.flush()?;
    }
    if !dry_run {
        destination.apply(&listing, &source, &local, &actions, sudo)?;
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bsync: {error}");
            ExitCode::FAILURE
        }
    }
}
