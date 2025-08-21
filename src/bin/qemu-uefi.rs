use std::{
    env, process::{self, Command}
};

use ovmf_prebuilt::{Arch, FileType, Prebuilt, Source};

fn main() {
    let prebuilt =
        Prebuilt::fetch(Source::LATEST, "target/ovmf").unwrap();
    let ovmf_code = prebuilt.get_file(Arch::X64, FileType::Code);
    let ovmf_vars = prebuilt.get_file(Arch::X64, FileType::Vars);
    let mut qemu = Command::new("qemu-system-x86_64");
    qemu.args([
        "-drive",
        &format!("format=raw,if=pflash,readonly=on,file={}", ovmf_code.display()),
        "-drive",
        &format!("format=raw,if=pflash,file={}", ovmf_vars.display()),
        "-drive",
        &format!("format=raw,file={}", env!("UEFI_IMAGE")),
    ]);
    // Improve CI capture and stability: force non-reboot/no-shutdown
    qemu.args([
        "-no-reboot",
        "-no-shutdown",
    ]);
    // Forward any additional command-line arguments to QEMU
    qemu.args(env::args().skip(1));
    let exit_status = qemu.status().unwrap();
    process::exit(exit_status.code().unwrap_or(-1));
}