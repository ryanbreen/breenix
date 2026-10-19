//! Host lock model for the kernel's actual console lookup and reply admission.
//! Interrupt masking is modeled; target boots validate the hardware behavior.
use quote::ToTokens;
use std::{
    fs,
    process::Command,
    time::{Duration, Instant},
};

#[test]
fn contended_console_lookup_returns_for_retry() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = fs::read_to_string(root.join("kernel/src/tty/driver.rs")).unwrap();
    // Parse Rust syntax rather than counting braces inside comments or literals.
    let parsed = syn::parse_file(&source).unwrap();
    let mut functions = String::new();
    for name in ["console", "vt", "push_reply"] {
        let function = parsed
            .items
            .iter()
            .find_map(|item| match item {
                syn::Item::Fn(function) if function.sig.ident == name => Some(function),
                _ => None,
            })
            .unwrap();
        functions.push_str(&function.to_token_stream().to_string());
    }
    let temporary = std::env::temp_dir().join(format!("console-reply-lock-{}", std::process::id()));
    fs::create_dir(&temporary).unwrap();
    let source = temporary.join("reply.rs");
    let binary = temporary.join("reply");
    fs::write(
        &source,
        include_str!("fixtures/console_reply_lock.rs").replace("// DRIVER_FUNCTIONS", &functions),
    )
    .unwrap();
    assert!(Command::new("rustc")
        .args(["--edition=2021", "-Dwarnings"])
        .arg(&source)
        .arg("-o")
        .arg(&binary)
        .status()
        .unwrap()
        .success());
    let mut child = Command::new(&binary).spawn().unwrap();
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if start.elapsed() >= Duration::from_secs(5) {
            child.kill().unwrap(); // Only the child this test started.
            child.wait().unwrap();
            fs::remove_dir_all(&temporary).unwrap();
            panic!("reply lookup deadlocked behind its suspended holder");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    fs::remove_dir_all(&temporary).unwrap();
    assert!(status.success());
}
