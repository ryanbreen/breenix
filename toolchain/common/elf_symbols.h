#ifndef BREENIX_TOOLCHAIN_ELF_SYMBOLS_H
#define BREENIX_TOOLCHAIN_ELF_SYMBOLS_H

/* ELF gABI §§2–5. This view borrows the input bytes. */
#include <stddef.h>
#include <stdint.h>

enum {
    SYMBOL_LOCAL = 0, SYMBOL_GLOBAL = 1, SYMBOL_WEAK = 2,
    SYMBOL_OBJECT = 1, SYMBOL_SECTION = 3, SYMBOL_FILE = 4,
    SYMBOL_UNDEFINED = 0, SYMBOL_ABSOLUTE = 0xfff1, SYMBOL_COMMON = 0xfff2,
    SYMBOL_NOBITS = 8, SYMBOL_WRITE = 1, SYMBOL_ALLOC = 2, SYMBOL_EXEC = 4
};

struct elf_symbols {
    const unsigned char *sections;
    size_t section_count;
    const unsigned char *table;
    size_t count;
    const unsigned char *strings;
    size_t strings_size;
};

struct elf_symbol {
    const char *name;
    uint64_t value, size, flags;
    unsigned binding, type, section, section_type;
};

const char *elf_symbols_read(struct elf_symbols *view, const unsigned char *data,
                             size_t size);
const char *elf_symbol_at(const struct elf_symbols *view, size_t index,
                          struct elf_symbol *sym);

/* The caller owns the result. */
unsigned char *tool_read_file(const char *path, size_t *size);

#endif
