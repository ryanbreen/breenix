/* ELF gABI §§2–7 and AMD64 psABI §§4.1, 4.4.1 (spec-ledger.md).
 * Static AMD64 LP64 links from ET_REL objects and common archives.
 * GNU ld 2.45 manual §2.1: archive order, groups and whole-archive.
 */
#define _POSIX_C_SOURCE 200809L

#include "../common/ar.h"
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
    SHN_LORESERVE = 0xff00, SHN_ABS = 0xfff1, SHN_COMMON = 0xfff2,
    STB_LOCAL = 0, STB_GLOBAL = 1, STB_WEAK = 2,
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
    const unsigned char *bytes;
    size_t size;
    struct input_section *sections;
    unsigned count, symtab;
    char *member_path;
};

struct link_input {
    const char *path;
    size_t group_end;
    bool whole_archive, is_archive;
    unsigned char *bytes;
    struct ar_archive archive;
    bool *selected;
};

enum definition { UNDEFINED, WEAK, COMMON, STRONG };

struct global_symbol {
    const char *name;
    enum definition definition;
    bool required;
    const struct object *object;
    const unsigned char *symbol;
    uint64_t common_size, common_align, common_offset;
};

struct link {
    struct object **objects;
    size_t count, capacity;
    struct global_symbol *globals;
    size_t global_count, global_capacity;
    struct input_section *common;
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

static bool read_file(struct link_input *input, size_t *length)
{
    FILE *file = fopen(input->path, "rb");
    if (file == NULL) {
        perror(input->path);
        return false;
    }
    long size = fseek(file, 0, SEEK_END) == 0 ? ftell(file) : -1;
    if (size >= 0 && (uint64_t)size <= SIZE_MAX && fseek(file, 0, SEEK_SET) == 0) {
        *length = (size_t)size;
        input->bytes = malloc(size == 0 ? 1 : *length);
    }
    bool read_ok = input->bytes != NULL && fread(input->bytes, 1, *length, file) == *length;
    if (fclose(file) != 0 || !read_ok) {
        fprintf(stderr, "%s: cannot read link input\n", input->path);
        return false;
    }
    return true;
}

static bool read_object(struct object *object)
{
    const unsigned char ident[] = {0x7f, 'E', 'L', 'F', 2, 1, 1};
    const unsigned char *bytes = object->bytes;
    if (object->size < EHDR_SIZE || memcmp(bytes, ident, sizeof ident) != 0 ||
        load_le(bytes + 16, 2) != ET_REL ||
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

static struct global_symbol *symbol_slot(struct global_symbol *table, size_t capacity,
                                         const char *name)
{
    uint64_t hash = UINT64_C(14695981039346656037);
    for (const unsigned char *p = (const unsigned char *)name; *p != '\0'; ++p)
        hash = (hash ^ *p) * UINT64_C(1099511628211);
    size_t i = (size_t)hash & (capacity - 1);
    while (table[i].name != NULL && strcmp(table[i].name, name) != 0)
        i = (i + 1) & (capacity - 1);
    return &table[i];
}

static const struct global_symbol *find_global(const struct link *link, const char *name)
{
    struct global_symbol *slot = symbol_slot(link->globals, link->global_capacity, name);
    return slot->name == NULL ? NULL : slot;
}

static struct global_symbol *add_global(struct link *link, const char *name)
{
    if (link->global_capacity != 0) {
        struct global_symbol *slot = symbol_slot(link->globals, link->global_capacity, name);
        if (slot->name != NULL)
            return slot;
    }
    if (link->global_count == link->global_capacity / 2) {
        if (link->global_capacity > SIZE_MAX / sizeof *link->globals / 2)
            return NULL;
        size_t capacity = link->global_capacity == 0 ? 64 : link->global_capacity * 2;
        struct global_symbol *table = calloc(capacity, sizeof *table);
        if (table == NULL)
            return NULL;
        for (size_t i = 0; i < link->global_capacity; ++i) {
            const struct global_symbol *old = &link->globals[i];
            if (old->name != NULL)
                *symbol_slot(table, capacity, old->name) = *old;
        }
        free(link->globals);
        link->globals = table;
        link->global_capacity = capacity;
    }
    struct global_symbol *slot = symbol_slot(link->globals, link->global_capacity, name);
    slot->name = name;
    ++link->global_count;
    return slot;
}

static bool register_symbols(struct link *link, const struct object *object, const char *indexed)
{
    const struct input_section *table = &object->sections[object->symtab];
    const struct input_section *strings = &object->sections[table->link];
    bool found = indexed == NULL;
    for (uint64_t i = SYM_SIZE; i < table->size; i += SYM_SIZE) {
        const unsigned char *symbol = object->bytes + table->offset + i;
        unsigned bind = symbol[4] >> 4;
        if (bind == STB_LOCAL)
            continue;
        if (bind != STB_GLOBAL && bind != STB_WEAK)
            return object_error(object, "unsupported symbol binding");
        const char *name = string_at(object, strings, load_le(symbol, 4));
        if (name == NULL || *name == '\0')
            return object_error(object, "invalid global symbol name");
        struct global_symbol *global = add_global(link, name);
        if (global == NULL)
            return object_error(object, "cannot allocate global symbol table");
        unsigned index = (unsigned)load_le(symbol + 6, 2);
        if (index == 0) {
            if (bind == STB_GLOBAL)
                global->required = true;
            continue;
        }
        if (!found && strcmp(name, indexed) == 0)
            found = true;
        /* ELF gABI §§5.2, 5.5. A strong definition overrides common or weak.
         * Coalesced commons need the maximum size and alignment independently. */
        enum definition definition = bind == STB_WEAK ? WEAK : STRONG;
        if (index == SHN_COMMON) {
            definition = COMMON;
            uint64_t align = load_le(symbol + 8, 8);
            uint64_t size = load_le(symbol + 16, 8);
            if (align == 0 || (align & (align - 1)) != 0 || align > IMAGE_BASE) {
                fprintf(stderr, "%s: %s: invalid common symbol alignment\n", object->path, name);
                return false;
            }
            if (align > global->common_align)
                global->common_align = align;
            if (size > global->common_size)
                global->common_size = size;
        }
        if (definition == STRONG && global->definition == STRONG) {
            fprintf(stderr, "%s: %s: duplicate definition (first defined in %s)\n",
                    object->path, name, global->object->path);
            return false;
        }
        if (definition > global->definition) {
            global->definition = definition;
            global->object = object;
            global->symbol = symbol;
        }
    }
    if (!found) {
        fprintf(stderr, "%s: archive index names missing definition %s\n", object->path, indexed);
        return false;
    }
    return true;
}

static struct object *append_object(struct link *link)
{
    if (link->count == link->capacity) {
        if (link->capacity > SIZE_MAX / sizeof *link->objects / 2)
            return NULL;
        size_t capacity = link->capacity == 0 ? 16 : link->capacity * 2;
        struct object **objects = realloc(link->objects, capacity * sizeof *objects);
        if (objects == NULL)
            return NULL;
        link->objects = objects;
        link->capacity = capacity;
    }
    struct object *object = calloc(1, sizeof *object);
    if (object != NULL)
        link->objects[link->count++] = object;
    return object;
}

static bool include_member(struct link *link, struct link_input *input, size_t member_index,
                           const char *indexed)
{
    const struct ar_member *member = &input->archive.members[member_index];
    size_t length = strlen(input->path);
    struct object *object = append_object(link);
    if (object == NULL || length > SIZE_MAX - 3 || member->name_length > SIZE_MAX - length - 3 ||
        (object->member_path = malloc(length + member->name_length + 3)) == NULL) {
        fprintf(stderr, "%s: cannot allocate archive member\n", input->path);
        return false;
    }
    char *path = object->member_path;
    memcpy(path, input->path, length);
    path[length++] = '(';
    memcpy(path + length, member->name, member->name_length);
    memcpy(path + length + member->name_length, ")", 2);
    object->path = path;
    object->bytes = member->bytes;
    object->size = member->size;
    input->selected[member_index] = true;
    return read_object(object) && register_symbols(link, object, indexed);
}

static bool search_archive(struct link *link, struct link_input *input)
{
    const struct ar_archive *archive = &input->archive;
    if (input->whole_archive) {
        for (size_t i = 0; i < archive->member_count; ++i) {
            if (!input->selected[i] && !include_member(link, input, i, NULL))
                return false;
        }
        return true;
    }

    /* An extraction can create references to earlier entries. Revisit this
     * archive until it adds no members. Weak undefineds do not trigger loads
     * (ELF gABI §5.2). */
    size_t before;
    do {
        before = link->count;
        for (size_t i = 0; i < archive->symbol_count; ++i) {
            const struct ar_symbol *symbol = &archive->symbols[i];
            if (input->selected[symbol->member])
                continue;
            const struct global_symbol *global = find_global(link, symbol->name);
            if (global != NULL && global->required && global->definition == UNDEFINED &&
                !include_member(link, input, symbol->member, symbol->name))
                return false;
        }
    } while (link->count != before);
    return true;
}

static bool include_input(struct link *link, struct link_input *input)
{
    size_t size = 0;
    if (!read_file(input, &size))
        return false;
    input->is_archive = ar_has_magic(input->bytes, size);
    if (!input->is_archive) {
        struct object *object = append_object(link);
        if (object == NULL) {
            fprintf(stderr, "%s: cannot allocate input object\n", input->path);
            return false;
        }
        object->path = input->path;
        object->bytes = input->bytes;
        object->size = size;
        return read_object(object) && register_symbols(link, object, NULL);
    }
    const char *error = ar_read(&input->archive, input->bytes, size);
    if (error == NULL && input->archive.member_count != 0) {
        if (!input->whole_archive && !input->archive.indexed) {
            error = "archive has no symbol index";
        } else {
            input->selected = calloc(input->archive.member_count, sizeof *input->selected);
            if (input->selected == NULL)
                error = "cannot allocate archive selection table";
        }
    }
    if (error != NULL) {
        fprintf(stderr, "%s: %s\n", input->path, error);
        return false;
    }
    return search_archive(link, input);
}

static bool include_inputs(struct link *link, struct link_input *inputs, size_t count)
{
    struct global_symbol *entry = add_global(link, "_start");
    if (entry == NULL) {
        fputs("bld: cannot allocate global symbol table\n", stderr);
        return false;
    }
    entry->required = true;
    for (size_t i = 0; i < count;) {
        size_t end = inputs[i].group_end;
        if (end == 0) {
            if (!include_input(link, &inputs[i++]))
                return false;
            continue;
        }

        /* A group alone reopens earlier archives when a later input adds a
         * reference. Ordinary object inputs enter the link just once. */
        ++i;
        size_t before = link->count;
        for (size_t j = i; j < end; ++j) {
            if (!include_input(link, &inputs[j]))
                return false;
        }
        while (link->count != before) {
            before = link->count;
            for (size_t j = i; j < end; ++j) {
                if (inputs[j].is_archive && !search_archive(link, &inputs[j]))
                    return false;
            }
        }
        i = end;
    }
    return true;
}

static bool align_up(uint64_t *position, uint64_t align)
{
    uint64_t pad = align == 0 ? 0 : (0 - *position) & (align - 1);
    if (pad > UINT64_MAX - *position)
        return false;
    *position += pad;
    return true;
}

static bool allocate_common(struct link *link)
{
    struct input_section common = {.name = ".bss", .type = ELF64_NOBITS,
                                   .flags = ELF64_ALLOC | ELF64_WRITE};
    for (size_t i = 0; i < link->global_capacity; ++i) {
        struct global_symbol *symbol = &link->globals[i];
        if (symbol->definition != COMMON)
            continue;
        if (!align_up(&common.size, symbol->common_align) ||
            !fits(common.size, symbol->common_size, UINT64_MAX))
            return object_error(symbol->object, "common storage layout overflow");
        symbol->common_offset = common.size;
        common.size += symbol->common_size;
        if (symbol->common_align > common.align)
            common.align = symbol->common_align;
    }
    if (common.align != 0) {
        struct object *object = append_object(link);
        if (object == NULL || (object->sections = calloc(2, sizeof *object->sections)) == NULL) {
            fputs("bld: cannot allocate common storage section\n", stderr);
            return false;
        }
        object->path = "bld (common storage)";
        object->count = 2;
        object->sections[1] = common;
        link->common = &object->sections[1];
    }
    return true;
}

static unsigned section_load(const struct input_section *section)
{
    if ((section->flags & ELF64_EXECINSTR) != 0)
        return RX_LOAD;
    return (section->flags & ELF64_WRITE) != 0 ? RW_LOAD : R_LOAD;
}

static bool select_sections(struct object *object, struct executable *exe)
{
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
        if (exe->section_count == SHN_LORESERVE - 1)
            return section_error(object, section, "executable section count exceeds limit");
        section->output_name = (uint32_t)exe->strings_size;
        exe->strings_size += length;
        ++exe->section_count;
    }
    return true;
}

static bool layout(const struct link *link, struct executable *exe)
{
    exe->strings_size = sizeof "\0.shstrtab";
    exe->section_count = 2; /* Null section and section-name table. */
    for (size_t i = 0; i < link->count; ++i) {
        if (!select_sections(link->objects[i], exe))
            return false;
    }
    uint64_t position = EHDR_SIZE + PHDR_COUNT * PHDR_SIZE;
    for (unsigned load_index = 0; load_index < LOAD_COUNT; ++load_index) {
        struct load *load = &exe->loads[load_index];
        if (!align_up(&position, PAGE_SIZE)) {
            fputs("bld: load layout overflow\n", stderr);
            return false;
        }
        load->offset = load_index == RX_LOAD ? 0 : position;

        /* NOBITS comes last so the gap through the end of BSS is memory only. */
        for (unsigned nobits = 0; nobits <= 1; ++nobits) {
            for (size_t i = 0; i < link->count; ++i) {
                const struct object *object = link->objects[i];
                for (unsigned j = 1; j < object->count; ++j) {
                    struct input_section *section = &object->sections[j];
                    if (section->output_name == 0 || section_load(section) != load_index ||
                        (section->type == ELF64_NOBITS) != (nobits != 0))
                        continue;
                    if (section->align > IMAGE_BASE || !align_up(&position, section->align) ||
                        !fits(position, section->size, UINT64_MAX - IMAGE_BASE))
                        return section_error(object, section, "section layout overflow");
                    section->output_offset = position;
                    position += section->size;
                }
            }
            if (nobits == 0)
                load->file_end = position;
        }
        load->memory_end = position;
    }
    exe->strings = exe->loads[RW_LOAD].file_end;
    if (!fits(exe->strings, exe->strings_size, SIZE_MAX)) {
        fputs("bld: executable string table overflow\n", stderr);
        return false;
    }
    exe->headers = exe->strings + exe->strings_size;
    if (!align_up(&exe->headers, 8) ||
        !fits(exe->headers, exe->section_count * SHDR_SIZE, SIZE_MAX)) {
        fputs("bld: executable section table overflow\n", stderr);
        return false;
    }
    exe->size = (size_t)(exe->headers + exe->section_count * SHDR_SIZE);
    exe->bytes = calloc(exe->size, 1);
    if (exe->bytes == NULL) {
        fputs("bld: cannot allocate executable\n", stderr);
        return false;
    }
    for (size_t i = 0; i < link->count; ++i) {
        const struct object *object = link->objects[i];
        for (unsigned j = 1; j < object->count; ++j) {
            const struct input_section *section = &object->sections[j];
            if (section->output_name != 0 && section->type != ELF64_NOBITS)
                memcpy(exe->bytes + section->output_offset, object->bytes + section->offset,
                       section->size);
        }
    }
    return true;
}

static bool entry_address(const struct link *link, uint64_t *entry)
{
    const struct global_symbol *start = find_global(link, "_start");
    if (start->definition == UNDEFINED) {
        fputs("bld: missing _start definition\n", stderr);
        return false;
    }
    const struct object *object = start->object;
    unsigned index = (unsigned)load_le(start->symbol + 6, 2);
    uint64_t value = load_le(start->symbol + 8, 8);
    if (index >= object->count || object->sections[index].output_name == 0 ||
        section_load(&object->sections[index]) != RX_LOAD || value >= object->sections[index].size)
        return object_error(object, "_start is outside executable code");
    *entry = IMAGE_BASE + object->sections[index].output_offset + value;
    return true;
}

static const char *symbol_address(const struct link *link, const struct object *object,
                                  const unsigned char *symbol, const char *name, uint64_t *value)
{
    if (symbol == NULL) {
        *value = 0; /* ELF gABI §6.1: STN_UNDEF uses symbol value zero. */
        return NULL;
    }
    unsigned bind = symbol[4] >> 4;
    if (bind != STB_LOCAL) {
        const struct global_symbol *global = find_global(link, name);
        if (global == NULL || global->definition == UNDEFINED) {
            if (bind != STB_WEAK)
                return "undefined symbol";
            *value = 0; /* ELF gABI §5.2: unresolved weak references have value zero. */
            return NULL;
        }
        if (global->definition == COMMON) {
            *value = IMAGE_BASE + link->common->output_offset + global->common_offset;
            return NULL;
        }
        object = global->object;
        symbol = global->symbol;
    }
    unsigned index = (unsigned)load_le(symbol + 6, 2);
    *value = load_le(symbol + 8, 8);
    if (index == SHN_ABS)
        return NULL;
    if (index == 0 || index >= object->count || object->sections[index].output_name == 0)
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

static const char *apply_relocation(const struct link *link, const struct object *object,
                                    struct executable *exe,
                                    const struct input_section *target, const unsigned char *rela,
                                    const unsigned char *symbol, const char *name, uint32_t type)
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
    const char *error = symbol_address(link, object, symbol, name, &value);
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

static bool relocate(const struct link *link, const struct object *object, struct executable *exe)
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
            const char *error = apply_relocation(link, object, exe, target, rela,
                                                 index == 0 ? NULL : symbol, name, type);
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

static void write_headers(const struct link *link, struct executable *exe)
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
    for (size_t i = 0; i < link->count; ++i) {
        const struct object *object = link->objects[i];
        for (unsigned j = 1; j < object->count; ++j) {
            const struct input_section *section = &object->sections[j];
            if (section->output_name == 0)
                continue;
            memcpy(bytes + exe->strings + section->output_name, section->name,
                   strlen(section->name) + 1);
            section_header(header, section->output_name, section->type, section->flags,
                           IMAGE_BASE + section->output_offset, section->output_offset,
                           section->size, section->align, section->entsize);
            header += SHDR_SIZE;
        }
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

static bool parse_args(int argc, char **argv, struct link_input *inputs,
                       size_t *count, const char **output)
{
    bool whole_archive = false;
    size_t group = SIZE_MAX, files = 0;
    for (int i = 1; i < argc; ++i) {
        const char *arg = argv[i];
        if (strcmp(arg, "-o") == 0) {
            if (*output != NULL || i + 1 == argc || argv[i + 1][0] == '\0')
                return false;
            *output = argv[++i];
        } else if (strcmp(arg, "--whole-archive") == 0) {
            whole_archive = true;
        } else if (strcmp(arg, "--no-whole-archive") == 0) {
            whole_archive = false;
        } else if (strcmp(arg, "--start-group") == 0 || strcmp(arg, "-(") == 0) {
            if (group != SIZE_MAX)
                return false;
            group = (*count)++;
        } else if (strcmp(arg, "--end-group") == 0 || strcmp(arg, "-)") == 0) {
            if (group == SIZE_MAX)
                return false;
            inputs[group].group_end = *count;
            group = SIZE_MAX;
        } else if (arg[0] == '-' || arg[0] == '\0') {
            fprintf(stderr, "bld: invalid link input or option: %s\n", arg);
            return false;
        } else {
            inputs[*count].path = arg;
            inputs[*count].whole_archive = whole_archive;
            ++*count;
            ++files;
        }
    }
    return *output != NULL && group == SIZE_MAX && files != 0;
}

int main(int argc, char **argv)
{
    struct link_input *inputs = calloc((size_t)argc, sizeof *inputs);
    if (inputs == NULL) {
        fputs("bld: cannot allocate link inputs\n", stderr);
        return 1;
    }
    struct link link = {0};
    struct executable exe = {0};
    const char *output = NULL;
    size_t count = 0;
    bool ok = parse_args(argc, argv, inputs, &count, &output);
    if (!ok)
        fputs("usage: bld -o OUTPUT [--whole-archive | --no-whole-archive |\n"
              "       --start-group | --end-group | OBJECT | ARCHIVE] ...\n", stderr);
    ok = ok && include_inputs(&link, inputs, count) && allocate_common(&link) &&
         layout(&link, &exe) && entry_address(&link, &exe.entry);
    for (size_t i = 0; ok && i < link.count; ++i) {
        const struct object *object = link.objects[i];
        if (object->symtab != 0)
            ok = relocate(&link, object, &exe);
    }
    if (ok) {
        write_headers(&link, &exe);
        if (!write_executable(output, &exe)) {
            fprintf(stderr, "%s: cannot write executable\n", output);
            ok = false;
        }
    }
    free(exe.bytes);
    for (size_t i = 0; i < link.count; ++i) {
        free(link.objects[i]->member_path);
        free(link.objects[i]->sections);
        free(link.objects[i]);
    }
    free(link.objects);
    free(link.globals);
    for (size_t i = 0; i < count; ++i) {
        free(inputs[i].selected);
        ar_dispose(&inputs[i].archive);
        free(inputs[i].bytes);
    }
    free(inputs);
    return ok ? 0 : 1;
}
