# elfcheck

`elfcheck` checks that an ELF64 relocatable object for x86-64 is well formed,
and with `--exec` that a static x86-64 executable is. It is written from two
pinned documents alone (digests in `toolchain/docs/spec-ledger.md`):

- gABI: Xinuos ELF Object File Format, version 4.3 draft, 2025-09-04;
- psABI: System V AMD64 ABI, Draft 1.0, 2018-01-28.

No assembler, linker, archiver, loader or ELF library source was read to write
it, its fixtures or its runner. bas (plan step 25.3) and bld (25.4) are checked
against it; it does not share code with either. The `--exec` checks were
written before bld existed.

```text
elfcheck FILE...          check each file as an ET_REL object
elfcheck --exec FILE...   check each file as a static ET_EXEC executable
elfcheck --list           print every check id, one per line
```

Each failed rule prints one line on stdout, `FILE: CHECK-ID: detail`. A file
with no finding prints `FILE: ok`. The exit status is 0 when no file has a
finding, 1 when a rule fails and 2 for a usage error or an unreadable file.

The caller names the file type it expects: an executable checked without
`--exec` fails `ehdr.type`, and so does an object checked with it.

## The Rust elfcheck (plan step 25.R)

`toolchain/elfcheck` is the same checker in Rust, written from the gABI, the
psABI and LSB Core 5.0 alone (E25 ruling 8; it cites Table 11-1 for
PT_GNU_STACK). It shares no code with bobj or the tools it checks, forbids
`unsafe`, builds with overflow checks on, and reports malformed input as
located findings rather than a panic. Until R4 deletes `elfcheck.c`, the C checker
is its parity oracle:

```sh
cd toolchain && cargo test -p elfcheck
```

`elfcheck::verdicts_equal_c_elfcheck` builds `elfcheck.c` with `$CC` (default
`cc`) under the strict flags below, writes the fixtures through
`write_fixtures.py`, and requires the same exit status and check ids from both
checkers, and the fixture's expectation, for the 2 baselines and 165 controls.
`elfcheck::malformed_input_is_error_not_panic` truncates and corrupts those
files and requires a report without a panic for each.

## Running the checks

```sh
python3 toolchain/tests/elfcheck/run.py --clang /absolute/path/to/clang \
    --lld /absolute/path/to/ld.lld --host-arch arm64 --host-arch x86_64
```

The runner builds `elfcheck.c` under `-std=c11 -Wall -Wextra -Werror
-pedantic`, plainly and under ASan and UBSan, and on macOS once per
`--host-arch`. Each build must:

- list exactly the check ids below;
- accept the hand-built baseline object from `fixtures.py`, and with `--exec`
  the hand-built baseline executable from `fixtures_exec.py`;
- reject each negative control with exit 1 and exactly the check ids the
  control names, no more and no fewer. The 106 controls in `fixtures.py` run
  without `--exec` and the 59 in `fixtures_exec.py` run with it.

Each of the 85 check ids below has at least one of the 165 controls, which the
runner's `every_check_has_a_control` test enforces. <!-- claim-lint:ok: 85 of 85 ids covered, toolchain/tests/elfcheck/run.py every_check_has_a_control. --> The runner also compiles
`blackbox/objects.c` and `blackbox/comdat.cc` with the given clang for
`x86_64-unknown-linux-gnu` and requires elfcheck to accept each object. clang
is a black-box producer here, never the definition of a rule. Those objects are
built with `-fno-addrsig` because clang marks `.llvm_addrsig` with sh_flags bit
`0x80000000`, a processor-specific bit psABI 1.0 does not assign, which
`shdr.flags` rejects.

For executables, the runner compiles `blackbox/program.c` four ways (`-O0`,
`-O2`, `-O2 -fPIE`, and `-O2 -ffunction-sections -fdata-sections` linked with
`--gc-sections`), links each with the given lld through
`userspace/programs/linker.ld`, the script Breenix programs link with, and
requires `elfcheck --exec` to accept each executable. lld is a black-box
producer too. `--lld` names an `ld.lld`; a generic `lld` or `rust-lld` driver
is given `-flavor gnu`.

## Checks

Rules marked *profile* go beyond what the documents require. They narrow the
format to what an x86-64 relocatable object from our tools may contain, and the
reason is given with each one. Where a rule's input structure is unusable (a
header that cannot be decoded, a table that is out of bounds or has the wrong
entry size, a link that does not name the right kind of section), the checks
that depend on that structure are skipped, so one defect reports one finding.

### ELF header (gABI 2.1, 2.2; psABI 4.1)

| Check | Rule |
| --- | --- |
| `ehdr.size` | The file holds a complete 64-byte Elf64_Ehdr (gABI Listing 2.1). Other checks are skipped for a shorter file. |
| `ehdr.magic` | `e_ident[0..3]` is `0x7f 'E' 'L' 'F'` (Table 2.4). |
| `ehdr.class` | `EI_CLASS` is ELFCLASS64, required for LP64 (psABI 4.1.1). |
| `ehdr.data` | `EI_DATA` is ELFDATA2LSB (psABI 4.1.1). |
| `ehdr.ident-version` | `EI_VERSION` is EV_CURRENT (gABI 2.2). |
| `ehdr.osabi` | `EI_OSABI` is ELFOSABI_NONE and `EI_ABIVERSION` is 0 (gABI 2.2). *Profile*: our objects use no OS extensions, so OS-specific symbol values have no meaning to check against. |
| `ehdr.pad` | `EI_PAD` bytes 9 to 15 hold 0, as gABI 2.2 reserves them. |
| `ehdr.type` | `e_type` is ET_REL (Table 2.1). |
| `ehdr.machine` | `e_machine` is EM_X86_64, 62 (psABI 4.1.1). |
| `ehdr.version` | `e_version` is EV_CURRENT (Table 2.2). |
| `ehdr.entry` | `e_entry` is 0: a relocatable file has no entry point (gABI 2.1). |
| `ehdr.phdr` | `e_phoff` and `e_phnum` are 0 and `e_phentsize` is 0 or 56. Relocatable files need no program header table (gABI 1.1), and program headers are meaningful only for executables and shared objects (gABI 7). |
| `ehdr.flags` | `e_flags` is 0: psABI 1.0 assigns no EF_X86_64 flags, and unassigned values are reserved (gABI 1.3). |
| `ehdr.ehsize` | `e_ehsize` is 64. |
| `ehdr.shentsize` | `e_shentsize` is 64, the Elf64_Shdr size (Listing 3.1). The section header table is not read otherwise. |

### Section header table (gABI 1.1, 1.2, 2.1, 3, 3.6)

| Check | Rule |
| --- | --- |
| `shdr.table-present` | `e_shoff` is nonzero: files used for linking must have a section header table (gABI 1.1). |
| `shdr.table-align` | `e_shoff` is a multiple of 8, the Elf64_Shdr alignment ("data also have suitable alignment from the beginning of the file", gABI 1.2). |
| `shdr.table-bounds` | `e_shoff + e_shnum * 64` lies inside the file without overflow. No section is read otherwise. |
| `shdr.null-entry` | Section header 0 holds 0 in `sh_name`, `sh_type`, `sh_flags`, `sh_addr`, `sh_offset`, `sh_info`, `sh_addralign` and `sh_entsize` (Table 3.5). |
| `shdr.shstrndx` | `e_shstrndx` names an existing SHT_STRTAB section. *Profile*: gABI 2.1 allows SHN_UNDEF, but our objects name their sections. Section names are not checked otherwise. |
| `shdr.type` | `sh_type` is one of SHT_NULL, PROGBITS, SYMTAB, STRTAB, RELA, NOTE, NOBITS, INIT_ARRAY, FINI_ARRAY, PREINIT_ARRAY, GROUP, SHT_X86_64_UNWIND (psABI Table 4.2), or a value in the OS-specific or user ranges, whose linking gABI 3.8 defines. Rejected: reserved values (gABI 3.3, "Other section type values are reserved"); processor values psABI 1.0 does not define; SHT_REL, since LP64 "uses only Elf64_Rela relocation entries" (psABI 4.4.1); SHT_RELR, which "may only appear in object files of type ET_EXEC or ET_DYN" (gABI 3.3); SHT_SHLIB, reserved with unspecified semantics. *Profile*: SHT_HASH, SHT_DYNAMIC and SHT_DYNSYM, which serve dynamic linking (gABI 8), are rejected in a relocatable object. |
| `shdr.flags` | Only flags defined in gABI Table 3.3, bits in SHF_MASKOS, and SHF_X86_64_LARGE (psABI Table 4.1) are set. Undefined attribute bits stay clear (gABI 3.4). |
| `shdr.addr` | `sh_addr` is 0. A relocatable file's sections have no place in a process image yet (gABI 3.2: "Otherwise, the member contains 0"). |
| `shdr.align` | `sh_addralign` is 0 or a power of two (gABI 3.2). |
| `shdr.bounds` | A section other than SHT_NULL and SHT_NOBITS lies inside the file: `sh_offset + sh_size` does not overflow and does not pass the end (gABI 3.2). |
| `shdr.overlap` | No two in-bounds sections share a file byte, and no section overlaps the ELF header or the section header table (gABI 3: "Sections in a file may not overlap"; sections contain everything except those headers). Empty and SHT_NOBITS sections occupy no bytes. |
| `shdr.entsize` | SHT_SYMTAB and SHT_RELA sections have `sh_entsize` 24 (Listings 5.1 and 6.1) and SHT_GROUP sections 4 (gABI 3.9), and `sh_size` is a multiple of it. A table that fails this is not read. |
| `shdr.merge` | A section with SHF_MERGE or SHF_STRINGS has a nonzero `sh_entsize`, and its `sh_size` is a multiple of it (gABI 3.4). |
| `shdr.data-align` | SHT_SYMTAB and SHT_RELA data start at a multiple of 8 and SHT_GROUP data at a multiple of 4 in the file (gABI 1.2). |
| `shdr.info-link` | A section with SHF_INFO_LINK has `sh_info` naming an existing section other than 0 (gABI 3.2, 3.4). |
| `shdr.link-order` | A section with SHF_LINK_ORDER has `sh_link` naming an existing section other than 0 (gABI 3.4). |
| `shdr.name` | `sh_name` is an index inside the section name string table (gABI 3.2, 4). |

### String tables (gABI 4)

| Check | Rule |
| --- | --- |
| `strtab.first-null` | A non-empty SHT_STRTAB section's first byte is a null character. |
| `strtab.last-null` | A non-empty SHT_STRTAB section's last byte is a null character, which terminates the strings it holds. |

### Symbol table (gABI 3.3, 3.5, 5)

| Check | Rule |
| --- | --- |
| `symtab.unique` | At most one SHT_SYMTAB section ("Currently, an object file may have only one section of each type", gABI 3.3). The first one is checked. |
| `symtab.link` | `sh_link` names an SHT_STRTAB section (Table 3.4). Symbol names are not checked otherwise. |
| `symtab.info` | `sh_info`, one greater than the last local symbol's index (Table 3.4), lies between 1 and the symbol count. Entry 0 is local (Table 5.5). |
| `sym.null-entry` | The table holds entry 0, and each field of entry 0 holds 0 (Table 5.5). |
| `sym.name` | A symbol's `st_name` is an index inside the linked string table (gABI 5.1). |
| `sym.binding` | The binding is STB_LOCAL, STB_GLOBAL or STB_WEAK (Table 5.2). OS values are rejected under ELFOSABI_NONE; psABI 1.0 defines no processor values. |
| `sym.type` | The type is STT_NOTYPE through STT_TLS (Table 5.3), with OS and processor values rejected for the same reason. |
| `sym.other` | `st_other` has no bits above the visibility field ("Other bits contain 0", gABI 5.1), and the visibility is STV_DEFAULT, INTERNAL, HIDDEN, PROTECTED or EXPORTED. STV_SINGLETON and STV_ELIMINATE are reserved to psABI supplements, and psABI 1.0 defines neither (Table 5.4). |
| `sym.visibility` | A local symbol is not STV_PROTECTED ("A symbol with STB_LOCAL binding may not have STV_PROTECTED visibility", gABI 5.4). |
| `sym.shndx` | `st_shndx` is SHN_UNDEF, SHN_ABS, SHN_COMMON or an existing section index. psABI 1.0 defines no processor-specific indexes, and the other reserved indexes have no meaning here (Table 3.1). A symbol that fails this has no further section checks. |
| `sym.value` | A symbol defined in a section has `st_value`, a section offset in a relocatable file (gABI 5.7), no greater than that section's `sh_size`. |
| `sym.size` | A symbol defined in a section, other than STT_SECTION and STT_FILE, lies inside it: `st_value + st_size` does not overflow and does not pass `sh_size` (gABI 5.1: "a data object's size is the number of bytes contained in the object"). Checked only when `sym.value` passes. |
| `sym.section` | An STT_SECTION symbol is defined relative to a real section, not SHN_UNDEF, SHN_ABS or SHN_COMMON ("The symbol is associated with a section", gABI 5.3). |
| `sym.file` | An STT_FILE symbol is local and its section index is SHN_ABS (gABI 5.3). |
| `sym.common-index` | An STT_COMMON symbol has section index SHN_COMMON ("In relocatable objects ... must have the special section index SHN_COMMON", gABI 5.3). |
| `sym.common-align` | A symbol with section index SHN_COMMON has an `st_value`, its alignment, that is a power of two. Storage is allocated "at an address that is a multiple of st_value" (gABI 5.5), so 0 is rejected. |
| `sym.tls-section` | A defined STT_TLS symbol lies in a section with SHF_TLS. Its value is an offset within the TLS template, which is formed from the SHF_TLS sections (gABI 5.3, 7.7). |
| `sym.local-order` | Symbols below `sh_info` are local and symbols at or above it are not ("all symbols with STB_LOCAL binding precede the weak and global symbols", gABI 5.2). <!-- claim-lint:ok: quoted from gABI 4.3 section 5.2, pinned in toolchain/docs/spec-ledger.md. --> |

### Relocations (gABI 3.5, 6.1; psABI 4.4)

| Check | Rule |
| --- | --- |
| `rela.link` | An SHT_RELA section's `sh_link` names the SHT_SYMTAB section (Table 3.4). Entry symbols are not checked otherwise. |
| `rela.info` | `sh_info` names the section the entries modify (Table 3.4). That section exists, is not section 0 and has file contents to modify: SHT_PROGBITS, SHT_NOTE, an array section, SHT_X86_64_UNWIND, or an OS- or user-range type. Entry offsets are not checked otherwise. |
| `rela.type` | `ELF64_R_TYPE(r_info)` is a value in psABI Table 4.9 or 4.10. Rejected: the values the tables mark Deprecated (30, 39, 40) or leave unassigned; R_X86_64_RELATIVE64, which "only appears in ILP32 executable files or shared objects"; and R_X86_64_8, 16, PC16 and PC8, since an object using them "is not conformant to this ABI" (psABI 4.4.1). *Profile*: R_X86_64_COPY, GLOB_DAT, JUMP_SLOT, RELATIVE, IRELATIVE and TLSDESC are also rejected. Their calculations use the load base or fill run-time GOT, PLT and descriptor entries (psABI 4.4.1, 5.2), so they describe a loaded image, not a relocatable input. |
| `rela.symbol` | `ELF64_R_SYM(r_info)` is less than the linked symbol table's entry count (gABI 6.1). Index 0 is allowed and means the value 0. |
| `rela.offset` | The relocated field lies inside the target section: `r_offset` plus the field width from psABI Figure 4.1 (0 bytes for no field, 4 for word32, 8 for word64 and LP64 wordclass) does not overflow and does not pass `sh_size` (gABI 6.1). |
| `rela.tls` | A thread-local storage relocation (DTPMOD64, DTPOFF64, TPOFF64, TLSGD, TLSLD, DTPOFF32, GOTTPOFF, TPOFF32, GOTPC32_TLSDESC, TLSDESC_CALL) references an STT_TLS symbol, and any other relocation does not reference one ("Symbols of type STT_TLS can be referenced by only special thread-local storage relocations and thread-local storage relocations can only reference symbols with type STT_TLS", gABI 5.3). |

### Section groups (gABI 3.4, 3.9)

| Check | Rule |
| --- | --- |
| `group.header` | An SHT_GROUP section's `sh_link` names the SHT_SYMTAB section, `sh_info` names a symbol other than entry 0 that the table holds, `sh_flags` is 0 and the section holds at least its flag word. |
| `group.flags` | The flag word sets only GRP_COMDAT and bits in GRP_MASKOS. psABI 1.0 assigns no GRP_MASKPROC bits (Table 3.7). |
| `group.members` | Each member index names an existing section other than 0 and the group itself. The member has SHF_GROUP ("Each such section must have the SHF_GROUP flag set") and comes after the group in the section header table ("must appear in the section header table before the entries for any of the sections that are members"). |
| `group.unique` | No section is a member twice ("A section cannot be a member of more than one group"), and each section with SHF_GROUP is a member of a group ("The section must be referenced by a section of type SHT_GROUP", gABI 3.4). Checked only when each group section can be read. |

### Limits

| Check | Rule |
| --- | --- |
| `limit.extended-numbering` | Extended section numbering is not supported: `e_shnum` 0 with a section header table, `e_shstrndx` SHN_XINDEX, a nonzero `sh_size` or `sh_link` in section header 0, an SHT_SYMTAB_SHNDX section, or a symbol with SHN_XINDEX (gABI 2.1, 3.3, 5.1). The format is valid; elfcheck reports it rather than checking it, and our tools do not produce it. |
| `limit.compressed` | SHF_COMPRESSED sections are not supported (gABI 3.7). elfcheck reports them rather than checking their contents. |

## Executables (`--exec`)

`--exec` checks a static executable of the kind `userspace/programs/linker.ld`
describes: no interpreter, no dynamic section and no run-time relocations
(design §5). Rules marked *profile* narrow the format to that shape.

### What changes for an executable

- `ehdr.type` requires ET_EXEC (gABI Table 2.1).
- `ehdr.entry` and `ehdr.phdr` do not apply; `load.entry` and the `phdr.*`
  checks below replace them.
- `shdr.addr`: a section without SHF_ALLOC has `sh_addr` 0, and a
  section's `sh_addr` is congruent to 0 modulo its power-of-two `sh_addralign`
  (gABI 3.2).
- `shdr.overlap` also keeps sections off the program header table, which is a
  header like the other two (gABI 7).
- `shdr.table-present` stays. *Profile*: gABI 1.1 makes the section header
  table optional in an executable, but the TLS checks need its sections and bnm
  reads its symbols, so our executables keep it.
- Not applied, because they state rules for relocatable files: `sym.value`,
  `sym.size`, `sym.common-index` and `sym.common-align` (in an executable
  `st_value` is a virtual address, gABI 5.7), and the `rela.*` checks (there
  `r_offset` is a virtual address, gABI 6.1). Symbol placement in an executable
  is bld's own tests' subject, not elfcheck's.
- The other checks above apply unchanged.

### Program header table (gABI 1.1, 1.2, 2.1, 7.1; psABI 4.1.2)

| Check | Rule |
| --- | --- |
| `phdr.table-present` | `e_phoff` and `e_phnum` are nonzero: "Files used to build a process image (execute a program) must have a program header table" (gABI 1.1). The checks below are skipped otherwise. |
| `phdr.entsize` | `e_phentsize` is 56, the Elf64_Phdr size (Listing 7.1). The table is not read otherwise. |
| `phdr.table-align` | `e_phoff` is a multiple of 8, the Elf64_Phdr alignment (gABI 1.2). |
| `phdr.table-bounds` | The table starts at or after the 64-byte ELF header and `e_phoff + e_phnum * 56` lies inside the file without overflow. The table is not read otherwise. |
| `limit.extended-numbering` | Also reported for `e_phnum` PN_XNUM, which moves the count into section header 0 (psABI 4.1.2). |

### Program header entries (gABI 7.1-7.4; psABI 5.1)

A PT_NULL entry is skipped: its "other members' values are undefined" (gABI 7.2).

| Check | Rule |
| --- | --- |
| `phdr.type` | `p_type` is PT_LOAD, PT_NOTE, PT_PHDR or PT_TLS (Table 7.1), PT_GNU_EH_FRAME or PT_SUNW_UNWIND (psABI Table 5.1), or PT_GNU_STACK. Rejected: PT_SHLIB ("Programs that contain an array element of this type do not conform to the ABI"); reserved values; OS and processor values neither document assigns. *Profile*: PT_DYNAMIC and PT_INTERP, which serve dynamic linking (gABI 8), are rejected in a static executable. An entry that fails this has no further checks. |
| `phdr.flags` | `p_flags` sets only PF_X, PF_W and PF_R (Table 7.2). *Profile*: PF_MASKOS and PF_MASKPROC bits are rejected; under ELFOSABI_NONE no OS meaning applies, and psABI 1.0 assigns no processor bits. |
| `phdr.align` | `p_align` is 0 or a power of two (gABI 7.1). |
| `phdr.bounds` | `p_offset + p_filesz` does not overflow and does not pass the end of the file (gABI 7.1: `p_offset` and `p_filesz` locate "the file image of the segment"). |
| `phdr.phdr` | PT_PHDR "may not occur more than once", "must precede any loadable segment entry", and describes "the program header table itself": `p_offset` is `e_phoff` and `p_filesz` and `p_memsz` are the table size. It "may occur only if the program header table is part of the memory image", so a PT_LOAD holds its bytes at its `p_vaddr` (gABI 7.2). |
| `phdr.gnu-stack` | Exactly one PT_GNU_STACK, and its `p_flags` has no PF_X. *Profile*: the segment asks for a stack without execute permission; `linker.ld` emits it with FLAGS(6), and the toolchain design's baseline requires it (design §5). |

### Loadable segments (gABI 7.1-7.5; psABI 3.3.3)

| Check | Rule |
| --- | --- |
| `load.filesz` | `p_filesz` is no larger than `p_memsz` ("The file size may not be larger than the memory size", gABI 7.2). |
| `load.order` | PT_LOAD entries "appear in ascending order, sorted on the p_vaddr member" (gABI 7.2). |
| `load.align` | `p_align` is a power of two of at least 4096. Loadable segments are congruent "modulo the page size" (gABI 7.1), and a conforming system's page size is a power of two from 4 KB to 64 KB (psABI 3.3.3), so a smaller `p_align` cannot carry that congruence. |
| `load.congruent` | `p_vaddr` equals `p_offset` modulo `p_align` (gABI 7.1). |
| `load.headers` | The first PT_LOAD has `p_offset` 0 and a file image that covers the ELF header and the program header table. *Profile*: Breenix's C library finds the TLS template through the program headers in memory, by AT_PHDR or through the loaded ELF header (`libs/libbreenix-libc/src/pthread.rs`), and `linker.ld` maps both headers with FILEHDR PHDRS in the text load. With no PT_LOAD at all the check fails too ("A program to be loaded by the system must have at least one loadable segment", gABI 7.4). |
| `load.entry` | `e_entry`, "the virtual address to which the system first transfers control" (gABI 2.1), lies inside the file image of a PT_LOAD with PF_X. *Profile*: the file image, not the zero-filled tail, since the first instruction comes from the file. |
| `load.sections` | Each SHF_ALLOC section with a nonzero size is inside a PT_LOAD's memory image at its `sh_addr`. A section with file bytes also lies inside that load's file image at the matching offset (`sh_offset - p_offset` equals `sh_addr - p_vaddr`). A section with SHF_WRITE is in a load with PF_W ("In no case, however, will a segment have write permission unless it is specified explicitly", gABI 7.4), and one with SHF_EXECINSTR in a load with PF_X. A TLS SHT_NOBITS section is exempt: it occupies template space, not load space (gABI 7.7). |
| `load.wx` | No PT_LOAD is both writable and executable. *Profile*: `linker.ld` separates the RX, R and RW loads, as the toolchain design's baseline requires (design §5). |

### Thread-local storage (gABI 3.10, 7.7)

"The TLS template is formed from the combination of all sections with the flag
SHF_TLS. The portion of the TLS template that holds initialized data is the TLS
initialization image. (The remaining portion of the TLS template is one or more
sections of type SHT_NOBITS.)" <!-- claim-lint:ok: quoted from gABI 4.3 section 7.7, pinned in toolchain/docs/spec-ledger.md. -->

| Check | Rule |
| --- | --- |
| `tls.segment` | A PT_TLS exists exactly when an SHF_TLS section does, there is at most one, its `p_flags` is PF_R and its `p_align` is the largest SHF_TLS section alignment, the alignment of the template (Table 7.4). |
| `tls.size` | With one PT_TLS and its sections: `p_vaddr` is the lowest SHF_TLS `sh_addr`; `p_filesz`, the size of the initialization image, runs from there to the end of the last SHF_TLS section with file bytes (.tdata); `p_memsz`, the total template size, runs to the end of the last SHF_TLS section, adding the SHT_NOBITS sections (.tbss) and the padding their alignment needs; and when `p_filesz` is nonzero, `p_offset` is the file offset of `p_vaddr` in the initialized sections (Table 7.4). |
| `tls.sections` | A section named `.tdata` is SHT_PROGBITS and one named `.tbss` SHT_NOBITS, each with SHF_ALLOC+SHF_WRITE+SHF_TLS (gABI Table 3.8). A `.tbss` given file bytes fails this even when PT_TLS describes it consistently: ".tbss ... occupies no file space" (gABI 3.10). |

### Where PT_GNU_STACK's value comes from

PT_GNU_STACK is not in gABI 4.3, psABI 1.0 or the pinned GNU ld manual: gABI
Table 7.1 reserves 0x60000000-0x6fffffff for operating systems, psABI Table 5.1
assigns only PT_GNU_EH_FRAME (0x6474e550) and PT_SUNW_UNWIND in that range, and
the ld manual's PHDRS section gives numbers for PT_NULL through PT_TLS only.
elfcheck uses 0x6474e551, recorded from black-box behaviour: given
`linker.ld`'s `stack PT_GNU_STACK FLAGS(6)`, lld writes that `p_type`, and
llvm-readelf names the entry GNU_STACK. The runner's lld executables pin it: a
different value would make elfcheck reject them. The spec ledger records this
gap for the E25 coordinator.

### What `--exec` does not check

- Whether the file bytes under an SHT_NOBITS section inside a load's file image hold 0.
- Overlap between the memory images of different loads.
- Symbol values and sizes in an executable, and relocation sections in one.
- Dynamic linking structures, which the profile rejects instead.

## What ARM64 lacks

elfcheck checks EM_X86_64 objects and executables only. An AArch64 file fails
`ehdr.machine`. The AArch64 relocation table, field widths and TLS relocation
set belong to the ARM backend's A1 step, and the AArch64 executable rules (the
variant I TLS layout, the 64 KiB page sizes AArch64 systems may use, the load
alignment that implies) to A2. Those steps add them here, with their own
controls, from Arm's ELF supplement for AArch64, which the spec ledger does not
pin yet.
