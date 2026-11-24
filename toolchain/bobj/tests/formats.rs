use bobj::{
    ar::{self, Archive, IndexEntry},
    elf::{self, Elf, RelSection, RelSymbol, Rela, StringTable},
    Error,
};

fn put(bytes: &mut [u8], offset: usize, width: usize, value: u64) {
    bytes[offset..offset + width].copy_from_slice(&value.to_le_bytes()[..width]);
}

const SHOFF: usize = 232;
const TEXT: usize = 64;
const RELA: usize = 72;
const SYMTAB: usize = 96;
const STRTAB: usize = 168;
const SHSTRTAB: usize = 182;

// ELF gABI Chapters 2–6. The on-disk .text, RELA, symbol and string tables
// start at the offsets above. .bss aligns the file cursor and has no payload.
fn object() -> Vec<u8> {
    let mut bytes = vec![0; SHOFF + 7 * 64];
    bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    for (offset, width, value) in [
        (16, 2, 1),
        (18, 2, 62),
        (20, 4, 1),
        (40, 8, SHOFF as u64),
        (52, 2, 64),
        (58, 2, 64),
        (60, 2, 7),
        (62, 2, 6),
    ] {
        put(&mut bytes, offset, width, value);
    }
    bytes[TEXT..TEXT + 5].copy_from_slice(b"\xe8\0\0\0\0");
    put(&mut bytes, RELA, 8, 1);
    put(&mut bytes, RELA + 8, 8, (2 << 32) | 4);
    put(&mut bytes, RELA + 16, 8, (-4_i64) as u64);
    // The local symbol is first in .symtab. Names stay in caller order.
    put(&mut bytes, SYMTAB + 24, 4, 8);
    put(&mut bytes, SYMTAB + 30, 2, 1);
    put(&mut bytes, SYMTAB + 48, 4, 1);
    bytes[SYMTAB + 52] = 0x12;
    bytes[STRTAB..SHSTRTAB].copy_from_slice(b"\0callee\0local\0");
    let names = b"\0.text\0.bss\0.rela.text\0.symtab\0.strtab\0.shstrtab\0";
    bytes[SHSTRTAB..SHSTRTAB + names.len()].copy_from_slice(names);
    for (index, (name, kind, flags, offset, size, link, info, align, entry_size)) in [
        (1, 1, 6, 64, 5, 0, 0, 1, 0),
        (7, 8, 3, 72, 4096, 0, 0, 8, 0),
        (12, 4, 0x40, 72, 24, 4, 1, 8, 24),
        (23, 2, 0, 96, 72, 5, 2, 8, 24),
        (31, 3, 0, 168, 14, 0, 0, 1, 0),
        (39, 3, 0, 182, names.len() as u64, 0, 0, 1, 0),
    ]
    .into_iter()
    .enumerate()
    {
        let base = SHOFF + (index + 1) * 64;
        for (field, width, value) in [
            (0, 4, name),
            (4, 4, kind),
            (8, 8, flags),
            (24, 8, offset),
            (32, 8, size),
            (40, 4, link),
            (44, 4, info),
            (48, 8, align),
            (56, 8, entry_size),
        ] {
            put(&mut bytes, base + field, width, value);
        }
    }
    bytes
}

fn executable() -> Vec<u8> {
    let mut bytes = vec![0; 121];
    bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    // ELF gABI Chapters 2 and 7. A sectionless ELF with one PT_LOAD.
    for (offset, width, value) in [
        (16, 2, 2),
        (18, 2, 62),
        (20, 4, 1),
        (24, 8, 0x400078),
        (32, 8, 64),
        (52, 2, 64),
        (54, 2, 56),
        (56, 2, 1),
        (64, 4, 1),
        (68, 4, 5),
        (80, 8, 0x400000),
        (96, 8, 121),
        (104, 8, 128),
        (112, 8, 4096),
    ] {
        put(&mut bytes, offset, width, value);
    }
    bytes[120] = 0xc3;
    bytes
}

// A panic fails the test outright. Rejecting everything also fails through
// the paired valid input in each test.
fn located_error<T: std::fmt::Debug>(result: Result<T, Error>, offset: usize) {
    let error = result.unwrap_err();
    assert_eq!(error.offset, offset, "{error:?}");
}

#[test]
fn elf_reader_is_error_not_panic() {
    let mut input = object();
    assert_eq!(
        Elf::parse(&input).unwrap().section_name(1).unwrap(),
        b".text"
    );
    for end in 0..input.len() {
        assert!(Elf::parse(&input[..end]).is_err(), "ELF prefix {end}");
    }
    put(&mut input, 40, 8, u64::MAX - 7);
    located_error(Elf::parse(&input), 40);

    let mut input = executable();
    let program = Elf::parse(&input).unwrap().program(0).unwrap();
    assert_eq!(program.virtual_address, 0x400000);
    assert_eq!((program.file_size, program.memory_size), (121, 128));
    for end in 0..input.len() {
        assert!(Elf::parse(&input[..end]).is_err(), "ELF prefix {end}");
    }
    put(&mut input, 96, 8, u64::MAX);
    located_error(Elf::parse(&input), 96);
}

#[test]
fn string_table_reader_is_error_not_panic() {
    let input = b"\0name\0";
    let table = StringTable::parse(input, 100).unwrap();
    assert_eq!(table.get(2).unwrap(), b"ame");
    located_error(StringTable::parse(&input[..5], 100), 104);
    located_error(StringTable::parse(b"x\0", 100), 100);
    assert!(table.get(u32::MAX).is_err());
}

#[test]
fn symbol_table_reader_is_error_not_panic() {
    let mut input = object();
    let file = Elf::parse(&input).unwrap();
    let symbols = file.symbols(4).unwrap();
    assert_eq!(symbols.get(1).unwrap().name, b"local");
    assert_eq!(symbols.get(2).unwrap().binding, 1);
    assert!(symbols.get(usize::MAX).is_err());

    let size = SHOFF + 4 * 64 + 32;
    put(&mut input, size, 8, 71);
    located_error(Elf::parse(&input).unwrap().symbols(4), size);
    put(&mut input, size, 8, 72);
    put(&mut input, SYMTAB + 24, 4, u32::MAX.into());
    located_error(
        Elf::parse(&input).unwrap().symbols(4).unwrap().get(1),
        SYMTAB + 24,
    );
}

#[test]
fn rela_reader_is_error_not_panic() {
    let mut input = object();
    let file = Elf::parse(&input).unwrap();
    let relocs = file.relocations(3).unwrap();
    assert_eq!(
        relocs.get(0).unwrap(),
        Rela {
            offset: 1,
            kind: 4,
            symbol: 2,
            addend: -4
        }
    );
    assert!(relocs.get(usize::MAX).is_err());

    let size = SHOFF + 3 * 64 + 32;
    put(&mut input, size, 8, 23);
    located_error(Elf::parse(&input).unwrap().relocations(3), size);
    put(&mut input, size, 8, 24);
    put(&mut input, RELA + 8, 8, u64::MAX);
    located_error(
        Elf::parse(&input).unwrap().relocations(3).unwrap().get(0),
        RELA + 8,
    );
}

#[test]
fn relocatable_writer_layout_and_symbol_order() {
    let reloc = Rela {
        offset: 1,
        kind: 4,
        symbol: 0,
        addend: -4,
    };
    let sections = [
        RelSection {
            name: b".text",
            kind: 1,
            flags: 6,
            alignment: 1,
            bytes: b"\xe8\0\0\0\0",
            size: 5,
            relocations: &[reloc],
        },
        RelSection {
            name: b".bss",
            kind: 8,
            flags: 3,
            alignment: 8,
            bytes: b"",
            size: 4096,
            relocations: &[],
        },
    ];
    let symbols = [
        RelSymbol {
            other: 0,
            name: b"callee",
            section: 0,
            kind: 2,
            binding: 1,
            value: 0,
            size: 0,
        },
        RelSymbol {
            other: 0,
            name: b"local",
            section: 1,
            kind: 0,
            binding: 0,
            value: 0,
            size: 0,
        },
    ];
    assert_eq!(elf::write_rel(62, &sections, &symbols).unwrap(), object());
}

// Solaris ar(3HEAD): member alignment and the big-endian symbol directory.
fn member(output: &mut Vec<u8>, name: &str, data: &[u8]) {
    output.extend_from_slice(
        format!(
            "{name:<16}0           0     0     644     {:<10}`\n",
            data.len()
        )
        .as_bytes(),
    );
    output.extend_from_slice(data);
    if data.len() % 2 == 1 {
        output.push(b'\n');
    }
}

fn archive(wide: bool) -> (Vec<u8>, usize) {
    let mut output = b"!<arch>\n".to_vec();
    let long_name = b"long member name.o/\n";
    let offset = 8 + 60 + if wide { 16 } else { 8 } + 4 + 60 + long_name.len();
    let mut index = if wide {
        [1_u64.to_be_bytes(), (offset as u64).to_be_bytes()].concat()
    } else {
        [1_u32.to_be_bytes(), (offset as u32).to_be_bytes()].concat()
    };
    index.extend_from_slice(b"sym\0");
    member(&mut output, if wide { "/SYM64/" } else { "/" }, &index);
    member(&mut output, "//", long_name);
    assert_eq!(output.len(), offset);
    member(&mut output, "/0", b"odd");
    (output, offset)
}

#[test]
fn archive_reader_is_error_not_panic() {
    for wide in [false, true] {
        let (mut input, offset) = archive(wide);
        let file = Archive::parse(&input).unwrap();
        assert_eq!(file.members[0].header_offset, offset);
        assert_eq!(file.members[0].name, b"long member name.o");
        assert_eq!(file.members[0].bytes, b"odd");
        assert_eq!(
            file.index.unwrap(),
            [IndexEntry {
                name: b"sym",
                member: 0
            }]
        );
        for end in (0..input.len()).filter(|end| *end != 8) {
            assert!(
                Archive::parse(&input[..end]).is_err(),
                "archive prefix {end}"
            );
        }

        // Point the index one byte into the header; members can still be reindexed.
        let word_size = if wide { 8 } else { 4 };
        input[68 + 2 * word_size - 1] += 1;
        located_error(Archive::parse(&input), 68 + word_size);
        assert_eq!(
            Archive::members_for_reindex(&input).unwrap()[0].bytes,
            b"odd"
        );
        input[offset + 1] = b'1';
        located_error(Archive::members_for_reindex(&input), offset);
    }
}

#[test]
fn archive_writer_emits_deterministic_long_names_and_index() {
    let (expected, _) = archive(false);
    let members: &[(&[u8], &[u8])] = &[(b"long member name.o", b"odd")];
    let index = [IndexEntry {
        name: b"sym",
        member: 0,
    }];
    assert_eq!(ar::write(members, &index).unwrap(), expected);
}
