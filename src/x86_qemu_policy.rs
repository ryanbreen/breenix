pub fn require_operator_override() {
    if std::env::var("BREENIX_ALLOW_X86_QEMU").ok().as_deref() != Some("1") {
        eprintln!("x86 QEMU refused by the 2026-10-18 bare-metal-only policy (S23); use the R240 metal boot loop (scripts/metal/metal-run); operator override: BREENIX_ALLOW_X86_QEMU=1.");
        std::process::exit(1);
    }
}
