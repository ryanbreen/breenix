mod checks;
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn run(prefix: &str) {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut failures = Vec::new();
    for &(name, run) in checks::CHECKS {
        let _ = checks::take_judged_image();
        if name.starts_with(prefix) {
            match std::panic::catch_unwind(run) {
                Ok(Ok(())) => {}
                Ok(Err(e)) => failures.push(format!("{name}: {e}")),
                Err(_) => failures.push(format!("{name}: codec panicked")),
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
#[test]
fn inflate_contracts() {
    run("inflate_");
}
#[test]
fn png_decode_contracts() {
    run("png_decode_");
}
#[test]
fn png_encode_contracts() {
    run("png_encode_");
}
#[test]
fn bmp_contracts() {
    run("bmp_");
}
