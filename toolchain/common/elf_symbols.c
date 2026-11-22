/* ELF gABI §§2–5: ELF64 little-endian headers, sections and static symbols. */
#include "elf_symbols.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

enum { ELF_HEADER = 64, SECTION_HEADER = 64, SYMBOL_SIZE = 24 };

static uint64_t little(const unsigned char *p, unsigned width)
{
    uint64_t value = 0;
    for (unsigned i = 0; i < width; ++i)
        value |= (uint64_t)p[i] << (8 * i);
    return value;
}

static const unsigned char *contents(const unsigned char *data, size_t size,
                                    const unsigned char *section, size_t *length)
{
    uint64_t offset = little(section + 24, 8);
    uint64_t bytes = little(section + 32, 8);
    if (offset > size || bytes > size - offset)
        return NULL;
    *length = (size_t)bytes;
    return data + offset;
}

const char *elf_symbols_read(struct elf_symbols *view, const unsigned char *data, size_t size)
{
    *view = (struct elf_symbols){0};
    if (size < ELF_HEADER || memcmp(data, "\177ELF\002\001\001", 7) != 0 ||
        little(data + 20, 4) != 1 || little(data + 52, 2) != ELF_HEADER)
        return "expected a little-endian ELF64 file";
    unsigned type = (unsigned)little(data + 16, 2);
    if (type != 1 && type != 2 && type != 3)
        return "expected an ELF relocatable, executable or shared object";
    uint64_t start = little(data + 40, 8);
    uint64_t count = little(data + 60, 2);
    if (start == 0 && count == 0)
        return NULL;
    if (start < ELF_HEADER || start > size || (size - start) < SECTION_HEADER ||
        little(data + 58, 2) != SECTION_HEADER)
        return "invalid ELF section table";
    view->sections = data + start;
    if (count == 0)
        count = little(view->sections + 32, 8);
    if (count == 0 || count > (size - start) / SECTION_HEADER)
        return "ELF section count exceeds bounds";
    view->section_count = (size_t)count;
    for (size_t i = 1; i < view->section_count; ++i) {
        const unsigned char *section = view->sections + i * SECTION_HEADER;
        if (little(section + 4, 4) != 2) /* SHT_SYMTAB */
            continue;
        size_t length;
        size_t strtab = (size_t)little(section + 40, 4);
        if (view->table != NULL || strtab >= view->section_count ||
            little(section + 56, 8) != SYMBOL_SIZE)
            return "invalid ELF symbol table";
        view->table = contents(data, size, section, &length);
        if (view->table == NULL || length < SYMBOL_SIZE || length % SYMBOL_SIZE != 0)
            return "ELF symbol table exceeds bounds";
        view->count = length / SYMBOL_SIZE;
        const unsigned char *strings = view->sections + strtab * SECTION_HEADER;
        view->strings = contents(data, size, strings, &view->strings_size);
        if (little(strings + 4, 4) != 3 || view->strings == NULL ||
            view->strings_size == 0 || view->strings[0] != 0 ||
            view->strings[view->strings_size - 1] != 0)
            return "invalid ELF symbol strings";
    }
    return NULL;
}

const char *elf_symbol_at(const struct elf_symbols *view, size_t index, struct elf_symbol *sym)
{
    if (index >= view->count)
        return "ELF symbol index exceeds bounds";
    const unsigned char *entry = view->table + index * SYMBOL_SIZE;
    size_t name = (size_t)little(entry, 4);
    if (name >= view->strings_size)
        return "ELF symbol name exceeds bounds";
    *sym = (struct elf_symbol){
        .name = (const char *)view->strings + name, .binding = entry[4] >> 4,
        .type = entry[4] & 15, .section = (unsigned)little(entry + 6, 2),
        .value = little(entry + 8, 8), .size = little(entry + 16, 8)
    };
    if (sym->section == SYMBOL_UNDEFINED || sym->section == SYMBOL_ABSOLUTE ||
        sym->section == SYMBOL_COMMON)
        return NULL;
    if (sym->section >= 0xff00 || sym->section >= view->section_count)
        return "unsupported ELF symbol section index";
    const unsigned char *section = view->sections + sym->section * SECTION_HEADER;
    sym->section_type = (unsigned)little(section + 4, 4);
    sym->flags = little(section + 8, 8);
    return NULL;
}

unsigned char *tool_read_file(const char *path, size_t *size)
{
    FILE *input = fopen(path, "rb");
    if (input == NULL)
        return NULL;
    long end = fseek(input, 0, SEEK_END) == 0 ? ftell(input) : -1;
    unsigned char *data = NULL;
    if (end >= 0 && (uintmax_t)end <= SIZE_MAX && fseek(input, 0, SEEK_SET) == 0) {
        *size = (size_t)end;
        data = malloc(*size == 0 ? 1 : *size);
        if (data != NULL && fread(data, 1, *size, input) != *size) {
            free(data);
            data = NULL;
        }
    }
    fclose(input);
    return data;
}
