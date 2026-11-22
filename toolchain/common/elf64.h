#ifndef BREENIX_TOOLCHAIN_ELF64_H
#define BREENIX_TOOLCHAIN_ELF64_H

/* Pinned ELF gABI §§3–5 and AMD64 psABI §4.1 (toolchain/docs/spec-ledger.md). */
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>

enum {
    ELF64_PROGBITS = 1,
    ELF64_NOBITS = 8,
    ELF64_WRITE = 1,
    ELF64_ALLOC = 2,
    ELF64_EXECINSTR = 4,
    ELF64_TLS = 0x400,
    ELF64_NOTYPE = 0,
    ELF64_OBJECT = 1,
    ELF64_FUNC = 2,
    ELF64_TLS_SYMBOL = 6
};

/* AMD64 psABI Table 4.9. Zero means that a source operand has no modifier. */
enum elf64_reloc_type {
    ELF64_R_64 = 1,
    ELF64_R_PC32 = 2,
    ELF64_R_PLT32 = 4,
    ELF64_R_GOTPCREL = 9,
    ELF64_R_32 = 10,
    ELF64_R_32S = 11,
    ELF64_R_TPOFF32 = 23
};

struct elf64_relocation {
    uint64_t offset;
    size_t symbol;  /* Zero-based index in the caller's unsorted symbol array. */
    enum elf64_reloc_type type;
    int64_t addend;
};

struct elf64_section {
    const char *name;
    uint32_t type;
    uint64_t flags;
    uint64_t alignment;
    const unsigned char *data;
    size_t size;
    const struct elf64_relocation *relocations;
    size_t relocation_count;
};

struct elf64_symbol {
    const char *name;
    uint16_t section;  /* One-based index into sections, or SHN_UNDEF. */
    unsigned char type;
    bool global;
    uint64_t value;
    uint64_t size;
};

/* Write a little-endian AMD64 relocatable object to an empty stream. */
bool elf64_write_rel(FILE *out, const struct elf64_section *sections,
                     size_t section_count, const struct elf64_symbol *symbols,
                     size_t symbol_count);

#endif
