# elfcheck

`elfcheck` checks that an ELF64 relocatable object for x86-64 is well formed.
It is written from two pinned documents alone (digests in
`toolchain/docs/spec-ledger.md`):

- gABI: Xinuos ELF Object File Format, version 4.3 draft, 2025-09-04;
- psABI: System V AMD64 ABI, Draft 1.0, 2018-01-28.

No assembler, linker or ELF library source was read to write it, its fixtures or
its runner. bas (plan step 25.3) and bld (25.4) are checked against it; it does
not share code with either.

```text
elfcheck FILE...        check each file
elfcheck --list         print every check id, one per line
```

Each failed rule prints one line on stdout, `FILE: CHECK-ID: detail`. A file
with no finding prints `FILE: ok`. The exit status is 0 when no file has a
finding, 1 when a rule fails and 2 for a usage error or an unreadable file.

## Running the checks

```sh
python3 toolchain/tests/elfcheck/run.py --clang /absolute/path/to/clang \
    --host-arch arm64 --host-arch x86_64
```

The runner builds `elfcheck.c` under `-std=c11 -Wall -Wextra -Werror
-pedantic`, plainly and under ASan and UBSan, and on macOS once per
`--host-arch`. Each build must:

- list exactly the check ids below;
- accept the hand-built baseline object from `fixtures.py`;
- reject each negative control with exit 1 and exactly the check ids the
  control names, no more and no fewer.

Each of the 64 check ids below has at least one of the 106 controls, which the
runner's `every_check_has_a_control` test enforces. <!-- claim-lint:ok: 64 of 64 ids covered, toolchain/tests/elfcheck/run.py every_check_has_a_control. --> The runner also compiles
`blackbox/objects.c` and `blackbox/comdat.cc` with the given clang for
`x86_64-unknown-linux-gnu` and requires elfcheck to accept each object. clang
is a black-box producer here, never the definition of a rule. Those objects are
built with `-fno-addrsig` because clang marks `.llvm_addrsig` with sh_flags bit
`0x80000000`, a processor-specific bit psABI 1.0 does not assign, which
`shdr.flags` rejects.

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

## What ARM64 lacks

elfcheck checks EM_X86_64 objects only. An AArch64 object fails `ehdr.machine`.
The AArch64 relocation table, field widths and TLS relocation set belong to the
ARM backend's A1 step. That step adds them here, with their own controls, from
Arm's ELF supplement for AArch64, which the spec ledger does not pin yet.
