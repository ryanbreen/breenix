/// Select the native Breenix architecture, retaining x86-64 for host tools.
pub fn default_target(os: &str, arch: &str) -> &'static str {
    if os == "breenix" && arch == "aarch64" {
        "aarch64"
    } else {
        "x86_64"
    }
}
