use std::env;
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::process::ExitCode;

const DEFAULT_WORD: &[u8] = b"y";
const BLOCK_SIZE: usize = 8192;
const HELP: &str = "\
Usage: yes [STRING]...
Repeat the STRINGs as one line. With no STRING, repeat y.
  --help     Show this help and exit
  --version  Show the version and exit
  --         End option processing
";
const VERSION: &str = concat!("yes (Breenix) ", env!("CARGO_PKG_VERSION"), "\n");

fn write_output(mut stdout: impl Write) -> io::Result<()> {
    let stop_at_operand = env::var_os("POSIXLY_CORRECT").is_some();
    let mut options = true;
    let mut have_operand = false;
    let mut line = Vec::new();
    for argument in env::args_os().skip(1) {
        let bytes = argument.as_bytes();
        if options && bytes.starts_with(b"-") && bytes.len() > 1 {
            match bytes {
                b"--" => {
                    options = false;
                    continue;
                }
                prefix if b"--help".starts_with(prefix) => {
                    return stdout.write_all(HELP.as_bytes());
                }
                prefix if b"--version".starts_with(prefix) => {
                    return stdout.write_all(VERSION.as_bytes());
                }
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("invalid option {argument:?}. Try 'yes --help'."),
                    ));
                }
            }
        }
        if have_operand {
            line.push(b' ');
        }
        line.extend_from_slice(bytes);
        have_operand = true;
        if stop_at_operand {
            options = false;
        }
    }
    if !have_operand {
        line.extend_from_slice(DEFAULT_WORD);
    }
    line.push(b'\n');

    // Assemble whole lines once so each write can carry many repetitions.
    let repeats = (BLOCK_SIZE / line.len()).max(1);
    let block = line.repeat(repeats);
    loop {
        stdout.write_all(&block)?;
    }
}

fn main() -> ExitCode {
    bu_sys::finish(
        "yes",
        bu_sys::default_sigpipe().and_then(|()| write_output(bu_sys::Stdout)),
    )
}
