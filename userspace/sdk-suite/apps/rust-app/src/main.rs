//! The SDK suite's Rust std app: one program, one subcommand per guest case.
//!
//! Each subcommand prints fixed lines on stdout and exits 0 when the behaviour it
//! exercises is right; the guest suite compares the exact output and exit status. A
//! subcommand that sees something wrong prints what it saw and exits 1.

#![feature(f16, f128, float_minimum_maximum)]

mod fs_cmds;
mod net_cmds;
mod proc_cmds;
mod thread_cmds;

use std::process::ExitCode;

type CmdResult = Result<(), String>;

// C23's _Float128 functions. rustc reports no reliable f128 math on either Breenix target (on
// x86-64, LLVM names its binary128 calls after long double, the x87 format there), so f128's
// methods are not what an application can rely on; these are, and are called directly.
extern "C" {
    fn sqrtf128(x: f128) -> f128;
    fn fmaximumf128(x: f128, y: f128) -> f128;
    fn roundevenf128(x: f128) -> f128;
    fn fmodf128(x: f128, y: f128) -> f128;
    fn fmaf128(x: f128, y: f128, z: f128) -> f128;
}

/// IEEE 754-2019 maximum and minimum and round-half-to-even, on values the compiler cannot
/// see: where the target has no instruction for them (x86-64 soft-float), LLVM calls the C
/// library's fmaximum, fminimumf and roundeven. Then the C library's _Float128 functions, and
/// f16's methods, which LLVM evaluates through f32 or the C library.
fn float() -> CmdResult {
    use std::hint::black_box;
    let nan = black_box(f64::NAN);
    println!(
        "maximum={} {} {}",
        black_box(1.5f64).maximum(nan),
        black_box(-0.0f64).maximum(black_box(0.0)),
        black_box(1.25f32).maximum(black_box(0.5)),
    );
    println!(
        "minimum={} {} {}",
        nan.minimum(black_box(1.0)),
        black_box(-0.0f32).minimum(black_box(0.0)),
        black_box(2.5f32).minimum(black_box(-3.5)),
    );
    println!(
        "round_ties_even={} {} {} {} {}",
        black_box(2.5f64).round_ties_even(),
        black_box(3.5f64).round_ties_even(),
        black_box(-2.5f64).round_ties_even(),
        black_box(0.5f32).round_ties_even(),
        black_box(5.5f32).round_ties_even(),
    );
    // SAFETY: the C library defines these with these signatures; they take and return values.
    let quad = unsafe {
        [
            sqrtf128(black_box(2.25)),
            fmaximumf128(black_box(-0.0), black_box(0.0)),
            roundevenf128(black_box(2.5)),
            fmodf128(black_box(7.5), black_box(2.0)),
            fmaf128(black_box(2.0), black_box(3.0), black_box(1.0)),
        ]
    };
    println!("f128={} {} {} {} {}", quad[0] as f64, quad[1] as f64, quad[2] as f64, quad[3] as f64, quad[4] as f64);
    println!(
        "f16={} {} {} {} {}",
        black_box(2.25f16).sqrt() as f32,
        black_box(-0.0f16).maximum(black_box(0.0)) as f32,
        black_box(2.5f16).round_ties_even() as f32,
        (black_box(7.5f16) % black_box(2.0f16)) as f32,
        black_box(1.5f16).minimum(black_box(-3.0)) as f32,
    );
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let Some(command) = args.get(1) else {
        println!("usage: rust-app COMMAND [ARG...]");
        return ExitCode::from(2);
    };
    let rest = &args[2..];
    let result = match command.as_str() {
        "hello" => {
            println!("hello from rust-app");
            Ok(())
        }
        "args" => proc_cmds::args(rest),
        "env" => proc_cmds::env(),
        "exit" => proc_cmds::exit(rest),
        "cat" => proc_cmds::cat(),
        "process-status" => proc_cmds::status(&args[0]),
        "process-output" => proc_cmds::output(&args[0]),
        "cwd" => proc_cmds::cwd(),
        "fs-rw" => fs_cmds::read_write(),
        "fs-dir" => fs_cmds::dirs(),
        "fs-metadata" => fs_cmds::metadata(),
        "threads" => thread_cmds::threads(),
        "sync" => thread_cmds::sync(),
        "net-tcp" => net_cmds::tcp(),
        "net-udp" => net_cmds::udp(),
        "net-unix" => net_cmds::unix(),
        "float" => float(),
        other => Err(format!("unknown command {other}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            println!("{why}");
            ExitCode::from(1)
        }
    }
}
