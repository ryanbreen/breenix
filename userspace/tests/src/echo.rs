//! echo - print arguments to stdout
//!
//! Usage: echo [STRING]...
//!
//! Prints the arguments separated by spaces, followed by a newline.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    println!("{}", args.join(" "));
}
