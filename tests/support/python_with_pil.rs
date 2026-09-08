use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

// Delegate candidate probing to the shell helper so fixtures and consumers agree.
pub fn python_with_pil() -> &'static str {
    static PYTHON: OnceLock<String> = OnceLock::new();
    PYTHON.get_or_init(|| {
        let helper = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("scripts/lib/python-with-pil.sh");
        let output = Command::new("bash")
            .args(["-c", "source \"$1\" && printf '%s' \"$BREENIX_PYTHON\"", "resolve-pil"])
            .arg(helper)
            .output()
            .expect("run Pillow interpreter resolver");
        assert!(output.status.success(), "Pillow resolver failed: {}",
            String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stdout).expect("Python executable path is UTF-8")
    })
}
