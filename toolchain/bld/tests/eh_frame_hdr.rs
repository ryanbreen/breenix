//! LSB Core 5.0 §§10.5–10.6, Table 11-1, and DWARF 5 §6.4.1.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use bobj::elf::{Elf, SectionHeader};

const PT_GNU_EH_FRAME: u32 = 0x6474_e550;
const SCRIPT: &str = "
ENTRY(_start)
PHDRS {
  text PT_LOAD FILEHDR PHDRS FLAGS(5);
  ro PT_LOAD FLAGS(4);
  data PT_LOAD FLAGS(6);
  frames PT_GNU_EH_FRAME FLAGS(4);
  stack PT_GNU_STACK FLAGS(6);
}
SECTIONS {
  . = 0x40000000 + SIZEOF_HEADERS;
  .text.start : { *(.text._start) } :text
  .text.live : { *(.text.live_fn) } :text
  .text.once : { *(.text.once) } :text
  .eh_frame_hdr : { *(.eh_frame_hdr) } :ro :frames
  .eh_frame : { KEEP(*(.eh_frame)) } :ro
}
";

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("bld-eh-frame-{}-{nanos}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn compile(&self, target: &str, name: &str, source: &str) {
        fs::write(self.0.join(format!("{name}.cc")), source).unwrap();
        let clang = std::env::var_os("CLANG").unwrap_or_else(|| "clang".into());
        assert_success(
            Command::new(clang)
                .current_dir(&self.0)
                .args([
                    "-target",
                    target,
                    "-c",
                    "-ffreestanding",
                    "-fno-exceptions",
                    "-fno-stack-protector",
                    "-ffunction-sections",
                    "-funwind-tables",
                    "-o",
                    &format!("{name}.o"),
                    &format!("{name}.cc"),
                ])
                .output()
                .unwrap(),
        );
    }

    fn link(&self, first: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_bld"))
            .current_dir(&self.0)
            .args([
                "--gc-sections",
                "--eh-frame-hdr",
                "-T",
                "layout.ld",
                "-o",
                "image.elf",
                first,
                "second.o",
                "start.o",
            ])
            .output()
            .unwrap()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn assert_success(output: Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn section<'a>(elf: &Elf<'a>, name: &[u8]) -> (SectionHeader, &'a [u8]) {
    let index = (1..elf.section_count())
        .find(|&i| elf.section_name(i).unwrap() == name)
        .unwrap_or_else(|| panic!("missing {}", String::from_utf8_lossy(name)));
    (
        elf.section(index).unwrap(),
        elf.section_data(index).unwrap().unwrap(),
    )
}

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn displacement(bytes: &[u8], at: usize) -> i64 {
    i64::from(word(bytes, at) as i32)
}

#[test]
fn eh_frame_hdr_indexes_live_fdes() {
    for &machine in elfcheck::Machine::ALL {
        let scratch = Scratch::new();
        let target = format!("{}-unknown-none-elf", machine.name());
        let once = r#"extern "C" __attribute__((used, noinline)) inline int once() { return 7; }"#;
        scratch.compile(
            &target,
            "first",
            &format!(
                r#"{once}
            extern "C" int live_fn() {{ return once(); }}
            extern "C" int dead() {{ return 9; }}"#
            ),
        );
        scratch.compile(&target, "second", once);
        scratch.compile(
            &target,
            "start",
            r#"
            extern "C" int live_fn();
            extern "C" int _start() { return live_fn(); }
        "#,
        );
        fs::write(scratch.0.join("layout.ld"), SCRIPT).unwrap();
        assert_success(scratch.link("first.o"));

        let image = fs::read(scratch.0.join("image.elf")).unwrap();
        let verdict = elfcheck::check_machine(&image, elfcheck::Kind::Executable, machine);
        assert!(
            verdict.error.is_none() && verdict.findings.is_empty(),
            "{verdict:?}"
        );
        let elf = Elf::parse(&image).unwrap();
        let (header, index) = section(&elf, b".eh_frame_hdr");
        let (frames, records) = section(&elf, b".eh_frame");

        assert_eq!(&index[..4], &[1, 0x1b, 0x03, 0x3b]);
        let base = header.address as i64;
        assert_eq!(base + 4 + displacement(index, 4), frames.address as i64);
        let count = word(index, 8) as usize;
        assert_eq!(count, 3, "entry, live function and winning COMDAT only");
        assert_eq!(index.len(), 12 + 8 * count);

        // Walk the physical FDEs too: hiding discarded FDEs from the index
        // alone would still leave them in .eh_frame.
        let mut fdes = Vec::new();
        let mut at = 0;
        while at < records.len() {
            let length = word(records, at) as usize;
            if length == 0 {
                assert_eq!(at + 4, records.len());
                break;
            }
            let cie = word(records, at + 4) as usize;
            if cie != 0 {
                let cie_start = at + 4 - cie;
                assert_eq!(word(records, cie_start + 4), 0);
                let pc_field = frames.address as i64 + at as i64 + 8;
                fdes.push((
                    pc_field + displacement(records, at + 8),
                    frames.address as i64 + at as i64,
                ));
            }
            at += length + 4;
            assert!(at <= records.len());
        }
        fdes.sort_unstable();
        let expected: Vec<_> = [b".text.start".as_slice(), b".text.live", b".text.once"]
            .map(|name| section(&elf, name).0.address as i64)
            .into();
        assert_eq!(fdes.iter().map(|&(pc, _)| pc).collect::<Vec<_>>(), expected);
        for (entry, (pc, fde)) in index[12..].chunks_exact(8).zip(fdes) {
            assert_eq!(base + displacement(entry, 0), pc);
            assert_eq!(base + displacement(entry, 4), fde);
        }

        let segments: Vec<_> = (0..elf.program_count())
            .map(|i| elf.program(i).unwrap())
            .filter(|program| program.kind == PT_GNU_EH_FRAME)
            .collect();
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].offset, header.offset);
        assert_eq!(segments[0].virtual_address, header.address);
        assert_eq!(segments[0].file_size, header.size);
        assert_eq!(segments[0].memory_size, header.size);
        assert_eq!(segments[0].flags, 4);

        let first = fs::read(scratch.0.join("first.o")).unwrap();
        let input = Elf::parse(&first).unwrap();
        let (raw, data) = section(&input, b".eh_frame");
        let fde_pointer = 4 + word(data, 0) as usize + 4;
        for (offset, value, kind, record) in [
            (8, 0xff, "CIE", 0),
            (fde_pointer, 0xff, "FDE", fde_pointer - 4),
        ] {
            let mut corrupt = first.clone();
            corrupt[raw.offset as usize + offset] = value;
            fs::write(scratch.0.join("corrupt.o"), corrupt).unwrap();
            let output = scratch.link("corrupt.o");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(!output.status.success());
            assert!(
                stderr.contains(&format!("corrupt.o: .eh_frame+0x{record:x}:"))
                    && stderr.contains(kind),
                "{stderr}"
            );
        }
    }
}
