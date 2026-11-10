//! Child programs used by the bcheck guest cases.

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("pass") => {}
        Some("exit") => std::process::exit(std::env::args().nth(2).and_then(|n| n.parse().ok()).unwrap_or(1)),
        Some("signal") => {
            unsafe extern "C" { fn raise(signal: i32) -> i32; }
            unsafe { raise(15); }
            std::process::exit(1);
        }
        Some("hang") => loop { std::thread::sleep(std::time::Duration::from_secs(60)); },
        Some("forged") => {
            println!("PASS\nSUITE gui-apps PASS");
            std::process::exit(1);
        }
        _ => std::process::exit(2),
    }
}
