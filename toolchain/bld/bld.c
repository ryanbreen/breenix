/* ELF gABI §§2–7 and AMD64 psABI §§4.1, 4.4.1 (spec-ledger.md).
 * This slice links one AMD64 LP64 ET_REL to a static ET_EXEC.
 */
#define _POSIX_C_SOURCE 200809L

#include "../common/elf64.h"

#include <inttypes.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

enum {
    EHDR_SIZE = 64, SHDR_SIZE = 64, PHDR_SIZE = 56, SYM_SIZE = 24, RELA_SIZE = 24,
    ET_REL = 1, ET_EXEC = 2, EM_X86_64 = 62,
    SHT_SYMTAB = 2, SHT_STRTAB = 3, SHT_RELA = 4, SHT_REL = 9, SHT_GROUP = 17,
    SHT_X86_64_UNWIND = 0x70000001,
    SHF_MERGE = 0x10, SHF_STRINGS = 0x20,
    SHN_LORESERVE = 0xff00, SHN_ABS = 0xfff1, STB_WEAK = 2,
    R_X86_64_NONE = 0, R_X86_64_PC64 = 24,
    PT_LOAD = 1, PF_X = 1, PF_W = 2, PF_R = 4,
    /* Linux Standard Base Core Specification 5.0, Generic Part, Table 11-1. */
    PT_GNU_STACK = 0x6474e551,
    PAGE_SIZE = 4096, IMAGE_BASE = 0x40000000,
    RX_LOAD = 0, R_LOAD = 1, RW_LOAD = 2, LOAD_COUNT = 3, PHDR_COUNT = LOAD_COUNT + 1
};

struct input_section {
    const char *name;
    uint32_t type, link, info;
    uint64_t flags, offset, size, align, entsize;
    uint64_t output_offset;
    uint32_t output_name;
};

struct object {
    const char *path;
    unsigned char *bytes;
    size_t size;
    struct input_section *sections;
    unsigned count, symtab;
};

struct load {
    uint64_t offset, file_end, memory_end;
};

struct executable {
    struct load loads[LOAD_COUNT];
    uint64_t strings, strings_size, headers, entry;
    unsigned section_count;
    size_t size;
    unsigned char *bytes;
};

static uint64_t load_le(const unsigned char *bytes, size_t width)
{
    uint64_t value = 0;
    for (size_t i = width; i > 0; --i)
        value = (value << 8) | bytes[i - 1];
    return value;
}

static void store_le(unsigned char *bytes, size_t width, uint64_t value)
{
    for (size_t i = 0; i < width; ++i) {
        bytes[i] = (unsigned char)value;
        value >>= 8;
    }
}

static bool fits(uint64_t offset, uint64_t size, uint64_t limit)
{
    return offset <= limit && size <= limit - offset;
}

static bool object_error(const struct object *object, const char *reason)
{
    fprintf(stderr, "%s: %s\n", object->path, reason);
    return false;
}

static bool section_error(const struct object *object, const struct input_section *section,
                          const char *reason)
{
    fprintf(stderr, "%s: %s: %s\n", object->path, section->name, reason);
    return false;
}

static const char *string_at(const struct object *object, const struct input_section *strings,
                             uint64_t index)
{
    if (strings->type != SHT_STRTAB || index >= strings->size)
        return NULL;
    const char *start = (const char *)object->bytes + strings->offset + index;
    return memchr(start, 0, strings->size - index) != NULL ? start : NULL;
}

static bool read_sections(struct object *object, uint64_t shoff, unsigned names)
{
    object->sections = calloc(object->count, sizeof *object->sections);
    if (object->sections == NULL)
        return object_error(object, "cannot allocate section table");
    for (unsigned i = 1; i < object->count; ++i) {
        const unsigned char *raw = object->bytes + shoff + i * SHDR_SIZE;
        struct input_section *section = &object->sections[i];
        section->type = (uint32_t)load_le(raw + 4, 4);
        section->flags = load_le(raw + 8, 8);
        section->offset = load_le(raw + 24, 8);
        section->size = load_le(raw + 32, 8);
        section->link = (uint32_t)load_le(raw + 40, 4);
        section->info = (uint32_t)load_le(raw + 44, 4);
        section->align = load_le(raw + 48, 8);
        section->entsize = load_le(raw + 56, 8);
        if (section->type != ELF64_NOBITS &&
            !fits(section->offset, section->size, object->size))
            return object_error(object, "section data outside object");
        if (section->align != 0 && (section->align & (section->align - 1)) != 0)
            return object_error(object, "section alignment is not a power of two");
        if (section->type == SHT_SYMTAB) {
            if (object->symtab != 0 || section->entsize != SYM_SIZE ||
                section->size == 0 || section->size % SYM_SIZE != 0 ||
                section->link == 0 || section->link >= object->count)
                return object_error(object, "invalid symbol table");
            object->symtab = i;
        }
    }
    for (unsigned i = 1; i < object->count; ++i) {
        struct input_section *section = &object->sections[i];
        uint64_t name = load_le(object->bytes + shoff + i * SHDR_SIZE, 4);
        section->name = string_at(object, &object->sections[names], name);
        if (section->name == NULL)
            return object_error(object, "invalid section name");
    }
    if (object->symtab == 0)
        return object_error(object, "missing symbol table");
    return true;
}

static bool read_object(struct object *object)
{
    FILE *input = fopen(object->path, "rb");
    if (input == NULL) {
        perror(object->path);
        return false;
    }
    long size = fseek(input, 0, SEEK_END) == 0 ? ftell(input) : -1;
    if (size >= EHDR_SIZE && fseek(input, 0, SEEK_SET) == 0) {
        object->size = (size_t)size;
        object->bytes = malloc(object->size);
    }
    bool read_ok = object->bytes != NULL &&
                   fread(object->bytes, 1, object->size, input) == object->size;
    if (fclose(input) != 0 || !read_ok)
        return object_error(object, "cannot read ELF object");

    const unsigned char ident[] = {0x7f, 'E', 'L', 'F', 2, 1, 1};
    const unsigned char *bytes = object->bytes;
    if (memcmp(bytes, ident, sizeof ident) != 0 || load_le(bytes + 16, 2) != ET_REL ||
        load_le(bytes + 18, 2) != EM_X86_64 || load_le(bytes + 20, 4) != 1 ||
        load_le(bytes + 32, 8) != 0 || load_le(bytes + 48, 4) != 0 ||
        load_le(bytes + 56, 2) != 0 ||
        load_le(bytes + 52, 2) != EHDR_SIZE)
        return object_error(object, "expected a little-endian AMD64 ELF64 relocatable object");
    uint64_t shoff = load_le(bytes + 40, 8);
    unsigned names = (unsigned)load_le(bytes + 62, 2);
    object->count = (unsigned)load_le(bytes + 60, 2);
    if (shoff < EHDR_SIZE || shoff % 8 != 0 || load_le(bytes + 58, 2) != SHDR_SIZE ||
        object->count >= SHN_LORESERVE || names == 0 || names >= object->count ||
        !fits(shoff, object->count * SHDR_SIZE, object->size))
        return object_error(object, "invalid or unsupported section table");
    return read_sections(object, shoff, names);
}

static bool align_up(uint64_t *position, uint64_t align)
{
    uint64_t pad = align == 0 ? 0 : (0 - *position) & (align - 1);
    if (pad > UINT64_MAX - *position)
        return false;
    *position += pad;
    return true;
}

static unsigned section_load(const struct input_section *section)
{
    if ((section->flags & ELF64_EXECINSTR) != 0)
        return RX_LOAD;
    return (section->flags & ELF64_WRITE) != 0 ? RW_LOAD : R_LOAD;
}

static bool layout(struct object *object, struct executable *exe)
{
    exe->strings_size = sizeof "\0.shstrtab";
    exe->section_count = 2; /* Null section and section-name table. */
    for (unsigned i = 1; i < object->count; ++i) {
        struct input_section *section = &object->sections[i];
        if (section->type == SHT_GROUP)
            return section_error(object, section, "section groups require COMDAT support");
        if ((section->flags & ELF64_ALLOC) == 0)
            continue;
        if ((section->flags & ~(uint64_t)(ELF64_ALLOC | ELF64_WRITE | ELF64_EXECINSTR |
                                          SHF_MERGE | SHF_STRINGS)) != 0 ||
            (section->type != ELF64_PROGBITS && section->type != ELF64_NOBITS &&
             section->type != SHT_X86_64_UNWIND) || section->link != 0 || section->info != 0 ||
            (section->type == ELF64_NOBITS && section_load(section) != RW_LOAD) ||
            (section->flags & (ELF64_WRITE | ELF64_EXECINSTR)) == (ELF64_WRITE | ELF64_EXECINSTR))
            return section_error(object, section, "unsupported allocated section");
        if ((section->flags & (SHF_MERGE | SHF_STRINGS)) != 0 &&
            (section->entsize == 0 || section->size % section->entsize != 0))
            return section_error(object, section, "invalid mergeable section entry size");
        size_t length = strlen(section->name) + 1;
        if (!fits(exe->strings_size, length, UINT32_MAX))
            return section_error(object, section, "section names exceed string table limit");
        section->output_name = (uint32_t)exe->strings_size;
        exe->strings_size += length;
        ++exe->section_count;
    }

    uint64_t position = EHDR_SIZE + PHDR_COUNT * PHDR_SIZE;
    for (unsigned load_index = 0; load_index < LOAD_COUNT; ++load_index) {
        struct load *load = &exe->loads[load_index];
        if (!align_up(&position, PAGE_SIZE))
            return object_error(object, "load layout overflow");
        load->offset = load_index == RX_LOAD ? 0 : position;

        /* NOBITS comes last so the gap through the end of BSS is memory only. */
        for (unsigned nobits = 0; nobits <= 1; ++nobits) {
            for (unsigned i = 1; i < object->count; ++i) {
                struct input_section *section = &object->sections[i];
                if (section->output_name == 0 || section_load(section) != load_index ||
                    (section->type == ELF64_NOBITS) != (nobits != 0))
                    continue;
                if (section->align > IMAGE_BASE || !align_up(&position, section->align) ||
                    !fits(position, section->size, UINT64_MAX - IMAGE_BASE))
                    return section_error(object, section, "section layout overflow");
                section->output_offset = position;
                position += section->size;
            }
            if (nobits == 0)
                load->file_end = position;
        }
        load->memory_end = position;
    }
    exe->strings = exe->loads[RW_LOAD].file_end;
    if (!fits(exe->strings, exe->strings_size, SIZE_MAX))
        return object_error(object, "executable string table overflow");
    exe->headers = exe->strings + exe->strings_size;
    if (!align_up(&exe->headers, 8) ||
        !fits(exe->headers, exe->section_count * SHDR_SIZE, SIZE_MAX))
        return object_error(object, "executable section table overflow");
    exe->size = (size_t)(exe->headers + exe->section_count * SHDR_SIZE);
    exe->bytes = calloc(exe->size, 1);
    if (exe->bytes == NULL)
        return object_error(object, "cannot allocate executable");
    for (unsigned i = 1; i < object->count; ++i) {
        const struct input_section *section = &object->sections[i];
        if (section->output_name != 0 && section->type != ELF64_NOBITS)
            memcpy(exe->bytes + section->output_offset, object->bytes + section->offset, section->size);
    }
    return true;
}

static bool entry_address(const struct object *object, uint64_t *entry)
{
    const struct input_section *table = &object->sections[object->symtab];
    for (uint64_t i = SYM_SIZE; i < table->size; i += SYM_SIZE) {
        const unsigned char *symbol = object->bytes + table->offset + i;
        const char *name = string_at(object, &object->sections[table->link], load_le(symbol, 4));
        if (name == NULL)
            return object_error(object, "invalid symbol name");
        uint64_t index = load_le(symbol + 6, 2);
        if (index == 0 || strcmp(name, "_start") != 0 || (symbol[4] >> 4) == 0)
            continue;
        uint64_t value = load_le(symbol + 8, 8);
        if (index >= object->count || object->sections[index].output_name == 0 ||
            section_load(&object->sections[index]) != RX_LOAD ||
            value >= object->sections[index].size)
            return object_error(object, "_start is outside executable code");
        if (*entry != 0)
            return object_error(object, "duplicate _start definition");
        *entry = IMAGE_BASE + object->sections[index].output_offset + value;
    }
    return *entry != 0 || object_error(object, "missing _start definition");
}

static const char *symbol_address(const struct object *object, const unsigned char *symbol,
                                  uint64_t *value)
{
    if (symbol == NULL) {
        *value = 0; /* ELF gABI §6.1: STN_UNDEF uses symbol value zero. */
        return NULL;
    }
    unsigned index = (unsigned)load_le(symbol + 6, 2);
    *value = load_le(symbol + 8, 8);
    if (index == SHN_ABS)
        return NULL;
    if (index == 0) {
        if ((symbol[4] >> 4) != STB_WEAK)
            return "undefined symbol";
        *value = 0; /* ELF gABI §5.2: unresolved weak references have value zero. */
        return NULL;
    }
    if (index >= object->count || object->sections[index].output_name == 0)
        return "symbol has no allocated section";
    if (*value > object->sections[index].size)
        return "symbol offset outside section";
    *value += IMAGE_BASE + object->sections[index].output_offset;
    return NULL;
}

static const char *relocation_name(uint32_t type)
{
    switch (type) {
    case ELF64_R_64: return "R_X86_64_64";
    case ELF64_R_PC32: return "R_X86_64_PC32";
    case ELF64_R_PLT32: return "R_X86_64_PLT32";
    case ELF64_R_GOTPCREL: return "R_X86_64_GOTPCREL";
    case ELF64_R_32: return "R_X86_64_32";
    case ELF64_R_32S: return "R_X86_64_32S";
    case 18: return "R_X86_64_TPOFF64";
    case ELF64_R_TPOFF32: return "R_X86_64_TPOFF32";
    case R_X86_64_PC64: return "R_X86_64_PC64";
    case 41: return "R_X86_64_GOTPCRELX";
    case 42: return "R_X86_64_REX_GOTPCRELX";
    default: return "unknown AMD64 relocation";
    }
}

static const char *apply_relocation(const struct object *object, struct executable *exe,
                                    const struct input_section *target, const unsigned char *rela,
                                    const unsigned char *symbol, uint32_t type)
{
    /* psABI Table 4.9. For a symbol bound in the static executable the
     * PLT32 call goes directly to S instead of through a PLT entry at L.
     * Use unsigned arithmetic for the two's-complement ELF64 addend.
     */
    unsigned width;
    switch (type) {
    case R_X86_64_NONE: return NULL;
    case ELF64_R_64:
    case R_X86_64_PC64:
        width = 8;
        break;
    case ELF64_R_PC32:
    case ELF64_R_PLT32:
    case ELF64_R_32:
    case ELF64_R_32S:
        width = 4;
        break;
    default:
        return "unsupported relocation";
    }

    uint64_t offset = load_le(rela, 8);
    if (target->type == ELF64_NOBITS || !fits(offset, width, target->size))
        return "relocation field outside section bytes";
    uint64_t value;
    const char *error = symbol_address(object, symbol, &value);
    if (error != NULL)
        return error;
    uint64_t field = target->output_offset + offset;
    value += load_le(rela + 16, 8);
    if (type == ELF64_R_PC32 || type == ELF64_R_PLT32 || type == R_X86_64_PC64)
        value -= IMAGE_BASE + field;
    if (type == ELF64_R_32 && value > UINT32_MAX)
        return "relocation overflow (unsigned 32-bit)";
    if (width == 4 && type != ELF64_R_32 &&
        value > INT32_MAX && value < UINT64_MAX - INT32_MAX)
        return "relocation overflow (signed 32-bit)";
    store_le(exe->bytes + field, width, value);
    return NULL;
}

static bool relocate(const struct object *object, struct executable *exe)
{
    const struct input_section *symbols = &object->sections[object->symtab];
    const struct input_section *strings = &object->sections[symbols->link];
    for (unsigned i = 1; i < object->count; ++i) {
        const struct input_section *section = &object->sections[i];
        if (section->type != SHT_RELA && section->type != SHT_REL)
            continue;
        if (section->type != SHT_RELA || section->entsize != RELA_SIZE ||
            section->size % RELA_SIZE != 0 || section->link != object->symtab ||
            section->info == 0 || section->info >= object->count)
            return section_error(object, section, "invalid or unsupported relocation table");
        const struct input_section *target = &object->sections[section->info];
        if (target->output_name == 0)
            continue;
        for (uint64_t pos = 0; pos < section->size; pos += RELA_SIZE) {
            const unsigned char *rela = object->bytes + section->offset + pos;
            uint32_t type = (uint32_t)load_le(rela + 8, 4);
            uint64_t index = load_le(rela + 12, 4);
            if (index >= symbols->size / SYM_SIZE)
                return section_error(object, section, "relocation symbol index outside table");
            const unsigned char *symbol = object->bytes + symbols->offset + index * SYM_SIZE;
            const char *name = string_at(object, strings, load_le(symbol, 4));
            if (name == NULL)
                return section_error(object, section, "invalid relocation symbol name");
            const char *error = apply_relocation(object, exe, target, rela,
                                                 index == 0 ? NULL : symbol, type);
            if (error != NULL) {
                fprintf(stderr, "%s: %s+0x%" PRIx64 ": %s (%" PRIu32 ") %s: %s\n",
                        object->path, target->name, load_le(rela, 8), relocation_name(type),
                        type, *name != '\0' ? name : "<unnamed>", error);
                return false;
            }
        }
    }
    return true;
}

static void section_header(unsigned char *header, uint32_t name, uint32_t type,
                           uint64_t flags, uint64_t address, uint64_t offset,
                           uint64_t size, uint64_t align, uint64_t entsize)
{
    store_le(header, 4, name);
    store_le(header + 4, 4, type);
    store_le(header + 8, 8, flags);
    store_le(header + 16, 8, address);
    store_le(header + 24, 8, offset);
    store_le(header + 32, 8, size);
    store_le(header + 48, 8, align);
    store_le(header + 56, 8, entsize);
}

static void write_headers(const struct object *object, struct executable *exe)
{
    /* ELF gABI Listings 2.1, 3.1 and 7.1. The buffer starts zero-filled. */
    unsigned char *bytes = exe->bytes;
    const unsigned char ident[] = {0x7f, 'E', 'L', 'F', 2, 1, 1};
    memcpy(bytes, ident, sizeof ident);
    store_le(bytes + 16, 2, ET_EXEC);
    store_le(bytes + 18, 2, EM_X86_64);
    store_le(bytes + 20, 4, 1);
    store_le(bytes + 24, 8, exe->entry);
    store_le(bytes + 32, 8, EHDR_SIZE);
    store_le(bytes + 40, 8, exe->headers);
    store_le(bytes + 52, 2, EHDR_SIZE);
    store_le(bytes + 54, 2, PHDR_SIZE);
    store_le(bytes + 56, 2, PHDR_COUNT);
    store_le(bytes + 58, 2, SHDR_SIZE);
    store_le(bytes + 60, 2, exe->section_count);
    store_le(bytes + 62, 2, exe->section_count - 1);

    const unsigned flags[] = {PF_R | PF_X, PF_R, PF_R | PF_W};
    for (unsigned i = 0; i < LOAD_COUNT; ++i) {
        const struct load *load = &exe->loads[i];
        unsigned char *phdr = bytes + EHDR_SIZE + i * PHDR_SIZE;
        store_le(phdr, 4, PT_LOAD);
        store_le(phdr + 4, 4, flags[i]);
        store_le(phdr + 8, 8, load->offset);
        store_le(phdr + 16, 8, IMAGE_BASE + load->offset);
        store_le(phdr + 24, 8, IMAGE_BASE + load->offset);
        store_le(phdr + 32, 8, load->file_end - load->offset);
        store_le(phdr + 40, 8, load->memory_end - load->offset);
        store_le(phdr + 48, 8, PAGE_SIZE);
    }

    /* LSB Core 5.0 §11.2: p_flags specifies stack permissions. */
    unsigned char *stack = bytes + EHDR_SIZE + LOAD_COUNT * PHDR_SIZE;
    store_le(stack, 4, PT_GNU_STACK);
    store_le(stack + 4, 4, PF_R | PF_W);

    memcpy(bytes + exe->strings + 1, ".shstrtab", sizeof ".shstrtab");
    unsigned char *header = bytes + exe->headers + SHDR_SIZE;
    for (unsigned i = 1; i < object->count; ++i) {
        const struct input_section *section = &object->sections[i];
        if (section->output_name == 0)
            continue;
        memcpy(bytes + exe->strings + section->output_name, section->name, strlen(section->name) + 1);
        section_header(header, section->output_name, section->type, section->flags,
                       IMAGE_BASE + section->output_offset, section->output_offset,
                       section->size, section->align, section->entsize);
        header += SHDR_SIZE;
    }
    section_header(header, 1, SHT_STRTAB, 0, 0, exe->strings, exe->strings_size, 1, 0);
}

static bool write_executable(const char *path, const struct executable *exe)
{
    const char suffix[] = ".XXXXXX";
    size_t length = strlen(path);
    if (length > SIZE_MAX - sizeof suffix)
        return false;
    char *temp = malloc(length + sizeof suffix);
    if (temp == NULL)
        return false;
    memcpy(temp, path, length);
    memcpy(temp + length, suffix, sizeof suffix);
    int fd = mkstemp(temp);
    if (fd == -1) {
        free(temp);
        return false;
    }

    FILE *out = fdopen(fd, "wb");
    mode_t mask = umask(0);
    umask(mask);
    bool ok = out != NULL && fwrite(exe->bytes, 1, exe->size, out) == exe->size &&
              fchmod(fd, 0777 & ~mask) == 0;
    if ((out != NULL ? fclose(out) : close(fd)) != 0)
        ok = false;
    if (ok)
        ok = rename(temp, path) == 0;
    if (!ok)
        unlink(temp);
    free(temp);
    return ok;
}

int main(int argc, char **argv)
{
    if (argc != 4 || strcmp(argv[1], "-o") != 0 || argv[2][0] == '\0' ||
        argv[3][0] == '\0' || argv[3][0] == '-') {
        fputs("usage: bld -o OUTPUT OBJECT\n", stderr);
        return 1;
    }
    struct object object = {.path = argv[3]};
    struct executable exe = {0};
    bool ok = read_object(&object) && layout(&object, &exe) &&
              entry_address(&object, &exe.entry) && relocate(&object, &exe);
    if (ok) {
        write_headers(&object, &exe);
        if (!write_executable(argv[2], &exe)) {
            fprintf(stderr, "%s: cannot write executable\n", argv[2]);
            ok = false;
        }
    }
    free(exe.bytes);
    free(object.sections);
    free(object.bytes);
    return ok ? 0 : 1;
}
