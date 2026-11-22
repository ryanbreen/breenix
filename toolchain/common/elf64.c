/* ELF gABI Listings 2.1, 3.1, 5.1, §§3.5–3.6, 4, 5.1–5.3, 5.6–5.7.
 * AMD64 psABI §4.1 fixes the class, endianness and machine.
 */
#include "elf64.h"

#include <errno.h>
#include <stdlib.h>
#include <string.h>

enum {
    ELF_HEADER_SIZE = 64,
    SECTION_HEADER_SIZE = 64,
    SYMBOL_SIZE = 24,
    SHT_SYMTAB = 2,
    SHT_STRTAB = 3,
    SHN_LORESERVE = 0xff00,
    STB_LOCAL = 0,
    STB_GLOBAL = 1,
    ET_REL = 1,
    EM_X86_64 = 62,
    ELFCLASS64 = 2,
    ELFDATA2LSB = 1,
    EV_CURRENT = 1
};

struct section_header {
    const char *name;
    uint32_t name_offset;
    uint32_t type;
    uint64_t flags;
    uint64_t offset;
    uint64_t size;
    uint32_t link;
    uint32_t info;
    uint64_t alignment;
    uint64_t entry_size;
};

static void store_le(unsigned char *dest, size_t width, uint64_t value)
{
    for (size_t i = 0; i < width; ++i) {
        dest[i] = (unsigned char)value;
        value >>= 8;
    }
}

static bool write_bytes(FILE *out, const void *data, size_t size)
{
    return size == 0 || fwrite(data, 1, size, out) == size;
}

static bool pad(FILE *out, uint64_t length)
{
    const unsigned char zeros[64] = {0};
    while (length > 0) {
        size_t chunk = length > sizeof zeros ? sizeof zeros : (size_t)length;
        if (!write_bytes(out, zeros, chunk))
            return false;
        length -= chunk;
    }
    return true;
}

static bool write_name(FILE *out, const char *name)
{
    return write_bytes(out, name, strlen(name) + 1);
}

static bool add_name(uint32_t *size, const char *name)
{
    size_t length = strlen(name);
    if (length >= UINT32_MAX - *size)
        return false;
    *size += (uint32_t)length + 1;
    return true;
}

/* Offsets are aligned even for NOBITS, which reserves no file bytes. */
static bool place(struct section_header *section, uint64_t *position)
{
    uint64_t mask = section->alignment == 0 ? 0 : section->alignment - 1;
    if ((section->alignment & mask) != 0)
        return false;
    uint64_t padding = (0 - *position) & mask;
    uint64_t size = section->type == ELF64_NOBITS ? 0 : section->size;
    if (padding > UINT64_MAX - *position ||
        size > UINT64_MAX - *position - padding)
        return false;
    section->offset = *position + padding;
    *position = section->offset + size;
    return true;
}

static bool write_header(FILE *out, uint64_t headers, uint16_t count, uint16_t strings)
{
    unsigned char bytes[ELF_HEADER_SIZE] = {
        0x7f, 'E', 'L', 'F', ELFCLASS64, ELFDATA2LSB, EV_CURRENT
    };
    store_le(bytes + 16, 2, ET_REL);
    store_le(bytes + 18, 2, EM_X86_64);
    store_le(bytes + 20, 4, EV_CURRENT);
    store_le(bytes + 40, 8, headers);
    store_le(bytes + 52, 2, ELF_HEADER_SIZE);
    store_le(bytes + 58, 2, SECTION_HEADER_SIZE);
    store_le(bytes + 60, 2, count);
    store_le(bytes + 62, 2, strings);
    return write_bytes(out, bytes, sizeof bytes);
}

static bool write_section_header(FILE *out, const struct section_header *section)
{
    unsigned char bytes[SECTION_HEADER_SIZE] = {0};
    store_le(bytes, 4, section->name_offset);
    store_le(bytes + 4, 4, section->type);
    store_le(bytes + 8, 8, section->flags);
    store_le(bytes + 24, 8, section->offset);
    store_le(bytes + 32, 8, section->size);
    store_le(bytes + 40, 4, section->link);
    store_le(bytes + 44, 4, section->info);
    store_le(bytes + 48, 8, section->alignment);
    store_le(bytes + 56, 8, section->entry_size);
    return write_bytes(out, bytes, sizeof bytes);
}

static bool write_symbols(FILE *out, const struct elf64_symbol *symbols, size_t count)
{
    unsigned char bytes[SYMBOL_SIZE] = {0};
    if (!write_bytes(out, bytes, sizeof bytes))
        return false;
    /* gABI §5.2 orders locals before globals. Names remain in input order. */
    for (unsigned binding = STB_LOCAL; binding <= STB_GLOBAL; ++binding) {
        uint32_t offset = 1;
        for (size_t i = 0; i < count; ++i) {
            const struct elf64_symbol *symbol = &symbols[i];
            if (symbol->global == (binding == STB_GLOBAL)) {
                store_le(bytes, 4, offset);
                bytes[4] = (unsigned char)((binding << 4) | symbol->type);
                store_le(bytes + 6, 2, symbol->section);
                store_le(bytes + 8, 8, symbol->value);
                store_le(bytes + 16, 8, symbol->size);
                if (!write_bytes(out, bytes, sizeof bytes))
                    return false;
            }
            offset += (uint32_t)strlen(symbol->name) + 1;
        }
    }
    return true;
}

static bool write_object(FILE *out, const struct elf64_section *sections,
                         const struct section_header *headers, uint16_t count,
                         const struct elf64_symbol *symbols, size_t symbol_count,
                         uint64_t header_offset)
{
    uint16_t shstrtab = count - 1;
    uint16_t strtab = count - 2;
    uint16_t symtab = count - 3;
    if (!write_header(out, header_offset, count, shstrtab))
        return false;

    uint64_t position = ELF_HEADER_SIZE;
    for (uint16_t i = 1; i < count; ++i) {
        const struct section_header *header = &headers[i];
        if (!pad(out, header->offset - position))
            return false;
        position = header->offset;
        if (header->type == ELF64_NOBITS)
            continue;
        if (i < symtab) {
            const struct elf64_section *section = &sections[i - 1];
            if (!write_bytes(out, section->data, section->size))
                return false;
        } else if (i == symtab) {
            if (!write_symbols(out, symbols, symbol_count))
                return false;
        } else if (i == strtab) {
            if (fputc(0, out) == EOF)
                return false;
            for (size_t j = 0; j < symbol_count; ++j)
                if (!write_name(out, symbols[j].name))
                    return false;
        } else {
            if (fputc(0, out) == EOF)
                return false;
            for (uint16_t j = 1; j < count; ++j)
                if (!write_name(out, headers[j].name))
                    return false;
        }
        position += header->size;
    }

    if (!pad(out, header_offset - position))
        return false;
    for (uint16_t i = 0; i < count; ++i)
        if (!write_section_header(out, &headers[i]))
            return false;
    return true;
}

static bool layout(struct section_header *headers, uint16_t count,
                   const struct elf64_section *sections,
                   const struct elf64_symbol *symbols, size_t symbol_count,
                   uint64_t *header_offset)
{
    uint16_t shstrtab = count - 1;
    uint16_t strtab = count - 2;
    uint16_t symtab = count - 3;
    uint32_t strings_size = 1;
    uint32_t first_global = 1; /* Skip the reserved null symbol. */
    for (size_t i = 0; i < symbol_count; ++i) {
        if (!add_name(&strings_size, symbols[i].name))
            return false;
        if (!symbols[i].global)
            ++first_global;
    }

    for (uint16_t i = 1; i < symtab; ++i) {
        const struct elf64_section *section = &sections[i - 1];
        headers[i] = (struct section_header){
            .name = section->name, .type = section->type, .flags = section->flags,
            .size = section->size, .alignment = section->alignment
        };
    }
    headers[symtab] = (struct section_header){
        .name = ".symtab", .type = SHT_SYMTAB, .alignment = 8, .entry_size = SYMBOL_SIZE,
        .link = strtab, .info = first_global, .size = (symbol_count + 1) * SYMBOL_SIZE
    };
    headers[strtab] = (struct section_header){
        .name = ".strtab", .type = SHT_STRTAB, .alignment = 1, .size = strings_size
    };
    headers[shstrtab] = (struct section_header){
        .name = ".shstrtab", .type = SHT_STRTAB, .alignment = 1
    };

    uint32_t section_names_size = 1;
    for (uint16_t i = 1; i < count; ++i) {
        headers[i].name_offset = section_names_size;
        if (!add_name(&section_names_size, headers[i].name))
            return false;
    }
    headers[shstrtab].size = section_names_size;

    uint64_t position = ELF_HEADER_SIZE;
    for (uint16_t i = 1; i < count; ++i)
        if (!place(&headers[i], &position))
            return false;
    struct section_header table = {
        .alignment = 8, .size = (uint64_t)count * SECTION_HEADER_SIZE
    };
    if (!place(&table, &position))
        return false;
    *header_offset = table.offset;
    return true;
}

bool elf64_write_rel(FILE *out, const struct elf64_section *sections,
                     size_t section_count, const struct elf64_symbol *symbols,
                     size_t symbol_count)
{
    /* The null section and the three tables also occupy section indices. */
    if (section_count >= SHN_LORESERVE - 4 || symbol_count >= SIZE_MAX / SYMBOL_SIZE ||
        symbol_count >= UINT32_MAX) {
        errno = EFBIG;
        return false;
    }
    uint16_t count = (uint16_t)(section_count + 4);
    struct section_header *headers = calloc(count, sizeof *headers);
    if (headers == NULL)
        return false;

    uint64_t header_offset;
    bool ok = layout(headers, count, sections, symbols, symbol_count, &header_offset);
    if (ok)
        ok = write_object(out, sections, headers, count, symbols, symbol_count, header_offset);
    else
        errno = EFBIG;
    free(headers);
    return ok;
}
