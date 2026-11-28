use arm64_boot_contract::handoff_elf::handoff_decision;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("expected kernel ELF path")?;
    let elf = std::fs::read(path)?;
    handoff_decision(&elf).map_err(|error| error.to_string().into())
}
