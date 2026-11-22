/* ELF gABI §§2–7 and AMD64 psABI §§4.1, 4.4.1 (spec-ledger.md).
 * Static AMD64 LP64 links from ET_REL objects and common archives.
 * GNU ld 2.45 manual §2.1: archive order, groups and whole-archive.
 */
#define _POSIX_C_SOURCE 200809L

#include "../common/ar.h"
#include "../common/elf64.h"
#include "script.h"

#include <errno.h>
#include <fcntl.h>
#include <fnmatch.h>
#include <inttypes.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

enum {
    EHDR_SIZE = 64, SHDR_SIZE = 64, PHDR_SIZE = 56, SYM_SIZE = 24, RELA_SIZE = 24,
    ET_REL = 1, ET_EXEC = 2, EM_X86_64 = 62,
    SHT_SYMTAB = 2, SHT_STRTAB = 3, SHT_RELA = 4, SHT_REL = 9, SHT_GROUP = 17,
    SHT_X86_64_UNWIND = 0x70000001, /* AMD64 psABI §4.2.2, Table 4.2. */
    SHF_MERGE = 0x10, SHF_STRINGS = 0x20, SHF_GROUP = 0x200, GRP_COMDAT = 1,
    SHN_LORESERVE = 0xff00, SHN_ABS = 0xfff1, SHN_COMMON = 0xfff2,
    STB_LOCAL = 0, STB_GLOBAL = 1, STB_WEAK = 2,
    R_X86_64_NONE = 0, R_X86_64_TPOFF64 = 18, R_X86_64_PC64 = 24,
    R_X86_64_GOTPCRELX = 41, R_X86_64_REX_GOTPCRELX = 42,
    PT_LOAD = 1, PT_TLS = 7, PF_X = 1, PF_W = 2, PF_R = 4,
    /* Linux Standard Base Core Specification 5.0, Generic Part, Table 11-1. */
    PT_GNU_STACK = 0x6474e551,
    PAGE_SIZE = 4096, IMAGE_BASE = 0x40000000,
    LOAD_COUNT = 3, BASE_PHDR_COUNT = LOAD_COUNT + 1
};

enum phase {
    TEXT_BYTES, RODATA_BYTES, DATA_BYTES, TDATA_BYTES, TBSS_MEMORY, BSS_MEMORY, PHASE_COUNT
};

static const enum segment_role phase_load[] = {
    TEXT, RODATA, DATA, DATA, DATA, DATA
};

struct input_section {
    const char *name;
    uint32_t type, link, info;
    uint64_t flags, offset, size, align, entsize;
    uint64_t output_offset;
    uint32_t output_name;
    unsigned group, first_rela, next_rela;
    size_t rule, selector;
    struct object *owner;
    struct input_section *pending;
    bool live, discarded;
};

struct object {
    const char *path;
    const unsigned char *bytes;
    size_t size;
    struct input_section *sections;
    unsigned count, symtab;
    char *member_path;
    size_t *local_got;
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
    bool required, live_common;
    const struct object *object;
    const unsigned char *symbol;
    uint64_t common_size, common_align, common_offset;
    size_t got_slot;
};

struct link {
    struct link_script script;
    const char *entry;
    uint64_t base;
    bool gc;
    const char **comdats;
    size_t comdat_count, comdat_capacity;
    size_t got_count;
    struct object **objects;
    size_t count, capacity;
    struct global_symbol *globals;
    size_t global_count, global_capacity;
    struct input_section *common, *got;
};

struct load {
    uint64_t offset, file_end, memory_end;
};

struct executable {
    struct load loads[LOAD_COUNT], tls;
    uint64_t tls_align, tls_block_size;
    struct input_section *outputs;
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
        section->owner = object;
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
    int fd = open(input->path, O_RDONLY);
    if (fd < 0) {
        perror(input->path);
        return false;
    }
    off_t size = lseek(fd, 0, SEEK_END);
    if (size >= 0 && (uintmax_t)size < SIZE_MAX && lseek(fd, 0, SEEK_SET) == 0) {
        *length = (size_t)size;
        input->bytes = malloc(*length + 1);
    }
    /* POSIX read permits short transfers and EINTR. A 64 KiB request also
     * lets the Breenix ext2 reader batch contiguous blocks from an archive. */
    const size_t chunk = 64 * 1024;
    size_t received = 0;
    while (input->bytes != NULL && received < *length) {
        size_t remaining = *length - received;
        ssize_t count = read(fd, input->bytes + received, remaining < chunk ? remaining : chunk);
        if (count < 0 && errno == EINTR)
            continue;
        if (count <= 0)
            break;
        received += (size_t)count;
    }
    if (close(fd) != 0 || input->bytes == NULL || received != *length) {
        fprintf(stderr, "%s: cannot read link input\n", input->path);
        return false;
    }
    input->bytes[*length] = 0;
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
    if (link->global_capacity == 0)
        return NULL;
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

static const unsigned char *symbol_at(const struct object *object, uint64_t index)
{
    const struct input_section *table = &object->sections[object->symtab];
    return index < table->size / SYM_SIZE ? object->bytes + table->offset + index * SYM_SIZE : NULL;
}

static const char *symbol_name(const struct object *object, const unsigned char *symbol)
{
    const struct input_section *table = &object->sections[object->symtab];
    return string_at(object, &object->sections[table->link], load_le(symbol, 4));
}

static bool record_comdat(struct link *link, const char *name, bool *duplicate)
{
    for (size_t i = 0; i < link->comdat_count; ++i) {
        if (strcmp(link->comdats[i], name) == 0) {
            *duplicate = true;
            return true;
        }
    }
    if (link->comdat_count == link->comdat_capacity) {
        if (link->comdat_capacity > SIZE_MAX / sizeof *link->comdats / 2)
            return false;
        size_t capacity = link->comdat_count == 0 ? 16 : link->comdat_count * 2;
        const char **names = realloc(link->comdats, capacity * sizeof *names);
        if (names == NULL)
            return false;
        link->comdats = names;
        link->comdat_capacity = capacity;
    }
    link->comdats[link->comdat_count++] = name;
    return true;
}

static bool read_groups(struct link *link, struct object *object)
{
    /* ELF gABI §3.9. The first signature wins and its input group is indivisible. */
    for (unsigned i = 1; i < object->count; ++i) {
        struct input_section *group = &object->sections[i];
        if (group->type != SHT_GROUP)
            continue;
        if (group->link != object->symtab || group->info == 0 ||
            group->entsize != 4 || group->size < 8 || group->size % 4 != 0 || group->flags != 0)
            return section_error(object, group, "invalid section group");
        const unsigned char *symbol = symbol_at(object, group->info);
        const char *name = symbol == NULL ? NULL : symbol_name(object, symbol);
        const unsigned char *words = object->bytes + group->offset;
        uint64_t flags = load_le(words, 4);
        if (name == NULL || ((flags & GRP_COMDAT) != 0 && *name == '\0') ||
            (flags & ~((uint64_t)GRP_COMDAT)) != 0)
            return section_error(object, group, "unsupported group flags or signature");
        bool duplicate = false;
        if ((flags & GRP_COMDAT) != 0 && !record_comdat(link, name, &duplicate))
            return section_error(object, group, "cannot allocate COMDAT signature");
        for (uint64_t pos = 4; pos < group->size; pos += 4) {
            uint64_t index = load_le(words + pos, 4);
            if (index <= i || index >= object->count)
                return section_error(object, group, "invalid section group member");
            struct input_section *member = &object->sections[index];
            if (member->group != 0 || (member->flags & SHF_GROUP) == 0 ||
                member->type == SHT_GROUP || member->type == SHT_SYMTAB)
                return section_error(object, group, "invalid section group member");
            member->group = i;
            member->discarded = duplicate;
        }
    }
    for (unsigned i = 1; i < object->count; ++i) {
        const struct input_section *section = &object->sections[i];
        if ((section->flags & SHF_GROUP) != 0 && section->group == 0)
            return section_error(object, section, "missing section group");
    }
    return true;
}

static bool read_relocations(struct object *object)
{
    for (unsigned i = object->count; i-- > 1;) {
        struct input_section *section = &object->sections[i];
        if (section->type != SHT_RELA && section->type != SHT_REL)
            continue;
        if (section->type != SHT_RELA || section->entsize != RELA_SIZE ||
            section->size % RELA_SIZE != 0 || section->link != object->symtab ||
            section->info == 0 || section->info >= object->count)
            return section_error(object, section, "invalid or unsupported relocation table");
        struct input_section *target = &object->sections[section->info];
        if (section->group != target->group)
            return section_error(object, section, "relocations must share their target's section group");
        section->next_rela = target->first_rela;
        target->first_rela = i;
    }
    return true;
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
        unsigned index = (unsigned)load_le(symbol + 6, 2);
        if (index > 0 && index < SHN_LORESERVE && index >= object->count)
            return object_error(object, "global symbol section index outside table");
        /* An archive index may name a definition in a losing COMDAT. */
        if (index != 0 && !found && strcmp(name, indexed) == 0)
            found = true;
        if (index > 0 && index < object->count && object->sections[index].discarded)
            index = 0;
        struct global_symbol *global = add_global(link, name);
        if (global == NULL)
            return object_error(object, "cannot allocate global symbol table");
        if (index == 0) {
            if (bind == STB_GLOBAL)
                global->required = true;
            continue;
        }
        /* ELF gABI §§5.2, 5.5. A strong definition overrides common or weak.
         * Coalesced commons need the maximum size and alignment independently. */
        enum definition definition = bind == STB_WEAK ? WEAK : STRONG;
        if (index == SHN_COMMON) {
            definition = COMMON;
            uint64_t align = load_le(symbol + 8, 8);
            uint64_t size = load_le(symbol + 16, 8);
            if (align == 0 || (align & (align - 1)) != 0 || align > link->base) {
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
    return read_object(object) && read_groups(link, object) && read_relocations(object) &&
           register_symbols(link, object, indexed);
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
        return read_object(object) && read_groups(link, object) && read_relocations(object) &&
               register_symbols(link, object, NULL);
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
    struct global_symbol *entry = add_global(link, link->entry);
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

static void match_rule(const struct link *link, struct input_section *section)
{
    for (size_t i = 0; i < link->script.count; ++i) {
        const struct output_rule *rule = &link->script.rules[i];
        for (size_t j = 0; j < rule->count; ++j) {
            const struct selector *selector = &rule->selectors[j];
            for (size_t k = 0; k < selector->count; ++k) {
                if (fnmatch(selector->patterns[k], section->name, 0) != 0)
                    continue;
                section->rule = i + 1;
                section->selector = j;
                return;
            }
        }
    }
}

static bool eligible(const struct input_section *section)
{
    return !section->discarded && (section->flags & ELF64_ALLOC) != 0;
}

static bool kept(const struct link *link, const struct input_section *section)
{
    return section->rule != 0 &&
           link->script.rules[section->rule - 1].selectors[section->selector].keep;
}

static void mark_live(struct input_section **pending, struct input_section *section)
{
    if (section->live || !eligible(section))
        return;
    section->live = true;
    section->pending = *pending;
    *pending = section;
}

static void mark_symbol(struct link *link, struct input_section **pending,
                        const struct object *object, const unsigned char *symbol,
                        const char *name)
{
    if ((symbol[4] >> 4) != STB_LOCAL) {
        struct global_symbol *global = symbol_slot(link->globals, link->global_capacity, name);
        if (global->definition == UNDEFINED)
            return;
        if (global->definition == COMMON) {
            global->live_common = true;
            return;
        }
        object = global->object;
        symbol = global->symbol;
    }
    unsigned index = (unsigned)load_le(symbol + 6, 2);
    if (index > 0 && index < object->count)
        mark_live(pending, &object->sections[index]);
}

static bool get_relocation_symbol(const struct object *object, const unsigned char *rela,
                                  const unsigned char **symbol, const char **name)
{
    uint64_t index = load_le(rela + 12, 4);
    *symbol = symbol_at(object, index);
    if (*symbol == NULL)
        return false;
    *name = symbol_name(object, *symbol);
    if (index == 0)
        *symbol = NULL;
    return *name != NULL;
}

static void mark_group(struct input_section **pending, const struct input_section *section)
{
    if (section->group == 0)
        return;
    struct object *object = section->owner;
    struct input_section *group = &object->sections[section->group];
    if (group->live)
        return;
    group->live = true;
    for (uint64_t pos = 4; pos < group->size; pos += 4) {
        uint64_t index = load_le(object->bytes + group->offset + pos, 4);
        mark_live(pending, &object->sections[index]);
    }
}

static bool select_live_sections(struct link *link)
{
    /* GNU ld 2.45 §2.1: mark the ENTRY and KEEP roots, then follow their
     * relocations. Section groups remain indivisible (ELF gABI §3.9). */
    struct input_section *pending = NULL;
    const struct global_symbol *entry = find_global(link, link->entry);
    if (entry->definition != UNDEFINED)
        mark_symbol(link, &pending, entry->object, entry->symbol, link->entry);
    for (size_t i = 0; i < link->count; ++i) {
        struct object *object = link->objects[i];
        for (unsigned j = 1; j < object->count; ++j) {
            struct input_section *section = &object->sections[j];
            if (!eligible(section))
                continue;
            match_rule(link, section);
            if (!link->gc)
                section->live = true;
            else if (kept(link, section))
                mark_live(&pending, section);
        }
    }
    while (link->gc && pending != NULL) {
        struct input_section *section = pending;
        pending = section->pending;
        const struct object *object = section->owner;
        mark_group(&pending, section);
        for (unsigned i = section->first_rela; i != 0; i = object->sections[i].next_rela) {
            const struct input_section *table = &object->sections[i];
            for (uint64_t pos = 0; pos < table->size; pos += RELA_SIZE) {
                const unsigned char *rela = object->bytes + table->offset + pos;
                if (load_le(rela + 8, 4) == R_X86_64_NONE)
                    continue;
                const unsigned char *symbol;
                const char *name;
                if (!get_relocation_symbol(object, rela, &symbol, &name))
                    return section_error(object, table, "invalid relocation symbol");
                if (symbol != NULL)
                    mark_symbol(link, &pending, object, symbol, name);
            }
        }
    }
    return true;
}

static bool add_synthetic(struct link *link, struct input_section section, const char *path,
                          struct input_section **result)
{
    struct object *object = append_object(link);
    if (object == NULL || (object->sections = calloc(2, sizeof *object->sections)) == NULL) {
        fprintf(stderr, "%s: cannot allocate linker section\n", path);
        return false;
    }
    object->path = path;
    object->count = 2;
    section.owner = object;
    section.live = true;
    match_rule(link, &section);
    object->sections[1] = section;
    *result = &object->sections[1];
    return true;
}

static bool allocate_common(struct link *link)
{
    struct input_section common = {.name = ".bss", .type = ELF64_NOBITS,
                                   .flags = ELF64_ALLOC | ELF64_WRITE};
    for (size_t i = 0; i < link->global_capacity; ++i) {
        struct global_symbol *symbol = &link->globals[i];
        if (symbol->definition != COMMON || (link->gc && !symbol->live_common))
            continue;
        if (!align_up(&common.size, symbol->common_align) ||
            !fits(common.size, symbol->common_size, UINT64_MAX))
            return object_error(symbol->object, "common storage layout overflow");
        symbol->common_offset = common.size;
        common.size += symbol->common_size;
        if (symbol->common_align > common.align)
            common.align = symbol->common_align;
    }
    return common.align == 0 || add_synthetic(link, common, "bld (common storage)", &link->common);
}

static enum phase section_phase(const struct link *link, const struct input_section *section)
{
    bool tls = (section->flags & ELF64_TLS) != 0, nobits = section->type == ELF64_NOBITS;
    if (tls)
        return nobits ? TBSS_MEMORY : TDATA_BYTES;
    if (nobits)
        return BSS_MEMORY;
    if (section->rule != 0) {
        enum segment_role load = link->script.rules[section->rule - 1].load;
        if (load == TEXT)
            return TEXT_BYTES;
        return load == RODATA ? RODATA_BYTES : DATA_BYTES;
    }
    if ((section->flags & ELF64_EXECINSTR) != 0)
        return TEXT_BYTES;
    return (section->flags & ELF64_WRITE) != 0 ? DATA_BYTES : RODATA_BYTES;
}

static bool name_output(struct input_section *section, struct executable *exe)
{
    size_t length = strlen(section->name) + 1;
    if (!fits(exe->strings_size, length, UINT32_MAX))
        return section_error(section->owner, section, "section names exceed string table limit");
    if (exe->section_count == SHN_LORESERVE - 1)
        return section_error(section->owner, section, "executable section count exceeds limit");
    section->output_name = (uint32_t)exe->strings_size;
    exe->strings_size += length;
    ++exe->section_count;
    return true;
}

static bool prepare_layout(const struct link *link, struct executable *exe)
{
    exe->strings_size = sizeof "\0.shstrtab";
    exe->section_count = 2; /* Null section and section-name table. */
    if (link->script.count != 0) {
        exe->outputs = calloc(link->script.count, sizeof *exe->outputs);
        if (exe->outputs == NULL) {
            fputs("bld: cannot allocate script output sections\n", stderr);
            return false;
        }
    }
    for (size_t i = 0; i < link->count; ++i) {
        const struct object *object = link->objects[i];
        for (unsigned j = 1; j < object->count; ++j) {
            struct input_section *section = &object->sections[j];
            if (!eligible(section) || !section->live)
                continue;
            if ((section->flags & ~(uint64_t)(ELF64_ALLOC | ELF64_WRITE | ELF64_EXECINSTR |
                                              SHF_MERGE | SHF_STRINGS | SHF_GROUP | ELF64_TLS)) != 0 ||
                (section->type != ELF64_PROGBITS && section->type != ELF64_NOBITS &&
                 section->type != SHT_X86_64_UNWIND) || section->link != 0 || section->info != 0 ||
                (section->type == ELF64_NOBITS && (section->flags & ELF64_WRITE) == 0) ||
                (section->flags & (ELF64_WRITE | ELF64_EXECINSTR)) == (ELF64_WRITE | ELF64_EXECINSTR))
                return section_error(object, section, "unsupported allocated section");
            if ((section->flags & (SHF_MERGE | SHF_STRINGS)) != 0 &&
                (section->entsize == 0 || section->size % section->entsize != 0))
                return section_error(object, section, "invalid mergeable section entry size");
            enum phase phase = section_phase(link, section);
            bool tls = (section->flags & ELF64_TLS) != 0;
            bool exec = (section->flags & ELF64_EXECINSTR) != 0;
            if (exec != (phase == TEXT_BYTES) ||
                (tls && ((section->flags & ELF64_WRITE) == 0 ||
                         (link->script.entry != NULL && !link->script.tls))))
                return section_error(object, section, "section flags conflict with segment assignment");
            if (tls && exe->tls_align == 0)
                exe->tls_align = 1;
            if (section->rule == 0) {
                if (!name_output(section, exe))
                    return false;
                if (tls && section->align > exe->tls_align)
                    exe->tls_align = section->align;
                continue;
            }
            const struct output_rule *rule = &link->script.rules[section->rule - 1];
            struct input_section *output = &exe->outputs[section->rule - 1];
            if (rule->load != phase_load[phase] || rule->tls != tls)
                return section_error(object, section, "section kind conflicts with script segment");
            if (output->owner == NULL) {
                *output = (struct input_section){.name = rule->name, .owner = section->owner,
                    .type = section->type, .align = rule->align, .rule = section->rule,
                    .flags = ELF64_ALLOC | (tls ? ELF64_TLS : 0) | (exec ? ELF64_EXECINSTR : 0) |
                             (rule->load == DATA ? ELF64_WRITE : 0)};
                if (!name_output(output, exe))
                    return false;
            }
            if (output->type != section->type)
                return section_error(object, section, "script output mixes section types");
            if (section->align > output->align)
                output->align = section->align;
            if (tls && output->align > exe->tls_align)
                exe->tls_align = output->align;
        }
    }
    enum phase previous = TEXT_BYTES;
    for (size_t i = 0; i < link->script.count; ++i) {
        const struct input_section *output = &exe->outputs[i];
        if (output->owner == NULL)
            continue;
        enum phase phase = section_phase(link, output);
        if (phase < previous)
            return section_error(output->owner, output, "script outputs must follow load and TLS order");
        previous = phase;
    }
    return true;
}

static bool place_section(const struct link *link, struct input_section *section, uint64_t *position)
{
    uint64_t address = link->base + *position;
    if (section->align > IMAGE_BASE || (section->align > 1 &&
        (link->base & (section->align - 1)) != 0) || !align_up(&address, section->align) ||
        !fits(address, section->size, UINT64_MAX))
        return section_error(section->owner, section, "section layout overflow");
    section->output_offset = address - link->base;
    *position = section->output_offset + section->size;
    return true;
}

static bool place_sections(const struct link *link, enum phase phase, size_t rule, size_t selector,
                           uint64_t *position)
{
    for (size_t i = 0; i < link->count; ++i) {
        const struct object *object = link->objects[i];
        for (unsigned j = 1; j < object->count; ++j) {
            struct input_section *section = &object->sections[j];
            if (eligible(section) && section->live && section->rule == rule &&
                section->selector == selector && section_phase(link, section) == phase &&
                !place_section(link, section, position))
                return false;
        }
    }
    return true;
}

static bool place_phase(const struct link *link, struct executable *exe, enum phase phase,
                        uint64_t *position)
{
    for (size_t i = 0; i < link->script.count; ++i) {
        struct input_section *output = &exe->outputs[i];
        if (output->owner == NULL || section_phase(link, output) != phase)
            continue;
        if (!place_section(link, output, position))
            return false;
        const struct output_rule *rule = &link->script.rules[i];
        for (size_t j = 0; j < rule->count; ++j) {
            if (!place_sections(link, phase, i + 1, j, position))
                return false;
        }
        output->size = *position - output->output_offset;
    }
    return place_sections(link, phase, 0, 0, position);
}

static bool place_tls(const struct link *link, struct executable *exe, uint64_t *position)
{
    if (exe->tls_align == 0)
        return true;
    uint64_t start = link->base + *position;
    if (!align_up(&start, exe->tls_align))
        return false;
    struct load *tls = &exe->tls;
    tls->offset = start - link->base;
    uint64_t cursor = tls->offset;
    if (!place_phase(link, exe, TDATA_BYTES, &cursor))
        return false;
    tls->file_end = cursor;
    if (tls->file_end > tls->offset)
        *position = tls->file_end;
    /* ELF gABI §7.7: .tbss occupies only the TLS template. Keep its
     * alignment padding and size out of both the file and ordinary BSS. */
    if (!place_phase(link, exe, TBSS_MEMORY, &cursor))
        return false;
    tls->memory_end = cursor;
    exe->tls_block_size = cursor - tls->offset;
    return align_up(&exe->tls_block_size, exe->tls_align);
}

static bool layout(const struct link *link, struct executable *exe)
{
    if (!prepare_layout(link, exe))
        return false;
    uint64_t position = EHDR_SIZE + (BASE_PHDR_COUNT + (exe->tls_align != 0)) * PHDR_SIZE;
    if ((link->script.entry == NULL && !align_up(&position, PAGE_SIZE)) ||
        !place_phase(link, exe, TEXT_BYTES, &position))
        return false;
    exe->loads[TEXT].file_end = exe->loads[TEXT].memory_end = position;
    if (!align_up(&position, PAGE_SIZE) || position > UINT64_MAX - link->base)
        return false;
    exe->loads[RODATA].offset = position;
    if (!place_phase(link, exe, RODATA_BYTES, &position))
        return false;
    exe->loads[RODATA].file_end = exe->loads[RODATA].memory_end = position;
    if (!align_up(&position, PAGE_SIZE) || position > UINT64_MAX - link->base)
        return false;
    exe->loads[DATA].offset = position;
    if (!place_phase(link, exe, DATA_BYTES, &position) || !place_tls(link, exe, &position))
        return false;
    exe->loads[DATA].file_end = position;
    if (!place_phase(link, exe, BSS_MEMORY, &position))
        return false;
    exe->loads[DATA].memory_end = position;
    exe->strings = exe->loads[DATA].file_end;
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
            if (eligible(section) && section->live && section->type != ELF64_NOBITS &&
                object->bytes != NULL)
                memcpy(exe->bytes + section->output_offset, object->bytes + section->offset,
                       section->size);
        }
    }
    return true;
}

static bool entry_address(const struct link *link, uint64_t *entry)
{
    const struct global_symbol *start = find_global(link, link->entry);
    if (start->definition == UNDEFINED) {
        fprintf(stderr, "bld: missing entry symbol %s\n", link->entry);
        return false;
    }
    const struct object *object = start->object;
    unsigned index = (unsigned)load_le(start->symbol + 6, 2);
    uint64_t value = load_le(start->symbol + 8, 8);
    if (index == 0 || index >= object->count || !object->sections[index].live ||
        section_phase(link, &object->sections[index]) != TEXT_BYTES ||
        value >= object->sections[index].size) {
        fprintf(stderr, "%s: entry symbol %s is outside executable code\n",
                object->path, link->entry);
        return false;
    }
    *entry = link->base + object->sections[index].output_offset + value;
    return true;
}

static const char *symbol_address(const struct link *link, const struct object *object,
                                  const unsigned char *symbol, const char *name,
                                  bool tls, uint64_t *value)
{
    if (symbol == NULL) {
        *value = 0; /* ELF gABI §6.1: value of STN_UNDEF. */
        return tls ? "TLS relocation requires a TLS definition" : NULL;
    }
    unsigned bind = symbol[4] >> 4;
    if (bind != STB_LOCAL) {
        const struct global_symbol *global = find_global(link, name);
        if (global == NULL || global->definition == UNDEFINED) {
            if (tls)
                return "TLS relocation requires a TLS definition";
            /* Breenix libc's phdr fallback uses this weak name for the ELF
             * header mapped by PHDRS FILEHDR (gABI §7.1; ld manual §3.8). */
            if (strcmp(name, "__ehdr_start") == 0)
                *value = link->base;
            else if (bind == STB_WEAK)
                *value = 0; /* ELF gABI §5.2: value of an unresolved weak reference. */
            else
                return "undefined symbol";
            return NULL;
        }
        if (global->definition == COMMON) {
            if (tls)
                return "TLS relocation requires a TLS definition";
            *value = link->base + link->common->output_offset + global->common_offset;
            return NULL;
        }
        object = global->object;
        symbol = global->symbol;
    }
    unsigned index = (unsigned)load_le(symbol + 6, 2);
    *value = load_le(symbol + 8, 8);
    if (index == SHN_ABS && !tls)
        return NULL;
    if (index == 0 || index >= object->count || !eligible(&object->sections[index]) ||
        !object->sections[index].live)
        return "symbol has no allocated section";
    const struct input_section *section = &object->sections[index];
    if (tls != ((symbol[4] & 15) == ELF64_TLS_SYMBOL) ||
        tls != ((section->flags & ELF64_TLS) != 0))
        return "relocation and symbol have incompatible TLS types";
    if (*value > section->size)
        return "symbol offset outside section";
    *value += link->base + section->output_offset;
    return NULL;
}

static const char *relocation_name(uint32_t type)
{
    /* AMD64 psABI §§4.4.1–4.4.2, Tables 4.9–4.10. Name recognized types even when this
     * static linker has no implementation for them. */
    static const char *const names[] = {
        [0] = "R_X86_64_NONE",
        [1] = "R_X86_64_64",
        [2] = "R_X86_64_PC32",
        [3] = "R_X86_64_GOT32",
        [4] = "R_X86_64_PLT32",
        [5] = "R_X86_64_COPY",
        [6] = "R_X86_64_GLOB_DAT",
        [7] = "R_X86_64_JUMP_SLOT",
        [8] = "R_X86_64_RELATIVE",
        [9] = "R_X86_64_GOTPCREL",
        [10] = "R_X86_64_32",
        [11] = "R_X86_64_32S",
        [12] = "R_X86_64_16",
        [13] = "R_X86_64_PC16",
        [14] = "R_X86_64_8",
        [15] = "R_X86_64_PC8",
        [16] = "R_X86_64_DTPMOD64",
        [17] = "R_X86_64_DTPOFF64",
        [18] = "R_X86_64_TPOFF64",
        [19] = "R_X86_64_TLSGD",
        [20] = "R_X86_64_TLSLD",
        [21] = "R_X86_64_DTPOFF32",
        [22] = "R_X86_64_GOTTPOFF",
        [23] = "R_X86_64_TPOFF32",
        [24] = "R_X86_64_PC64",
        [25] = "R_X86_64_GOTOFF64",
        [26] = "R_X86_64_GOTPC32",
        [27] = "R_X86_64_GOT64",
        [28] = "R_X86_64_GOTPCREL64",
        [29] = "R_X86_64_GOTPC64",
        [31] = "R_X86_64_PLTOFF64",
        [32] = "R_X86_64_SIZE32",
        [33] = "R_X86_64_SIZE64",
        [34] = "R_X86_64_GOTPC32_TLSDESC",
        [35] = "R_X86_64_TLSDESC_CALL",
        [36] = "R_X86_64_TLSDESC",
        [37] = "R_X86_64_IRELATIVE",
        [38] = "R_X86_64_RELATIVE64",
        [41] = "R_X86_64_GOTPCRELX",
        [42] = "R_X86_64_REX_GOTPCRELX",
    };
    return type < sizeof names / sizeof names[0] && names[type] != NULL
        ? names[type] : "unknown AMD64 relocation";
}

static bool is_gotpcrel(uint32_t type)
{
    return type == ELF64_R_GOTPCREL || type == R_X86_64_GOTPCRELX ||
           type == R_X86_64_REX_GOTPCRELX;
}

static bool relocation_error(const struct object *object, const struct input_section *target,
                             const unsigned char *rela, const char *name, const char *error)
{
    uint32_t type = (uint32_t)load_le(rela + 8, 4);
    fprintf(stderr, "%s: %s+0x%" PRIx64 ": %s (%" PRIu32 ") %s: %s\n",
            object->path, target->name, load_le(rela, 8), relocation_name(type),
            type, *name != '\0' ? name : "<unnamed>", error);
    return false;
}

static size_t *got_slot(const struct link *link, const struct object *object,
                        const unsigned char *symbol, size_t index, const char *name)
{
    if ((symbol[4] >> 4) == STB_LOCAL)
        return &object->local_got[index];
    return &symbol_slot(link->globals, link->global_capacity, name)->got_slot;
}

static const char *add_got_entry(struct link *link, struct object *object,
                                 const unsigned char *rela, const unsigned char *symbol,
                                 const char *name)
{
    if (symbol == NULL)
        return "GOT relocation requires a symbol";
    if ((symbol[4] >> 4) == STB_LOCAL && object->local_got == NULL) {
        size_t count = object->sections[object->symtab].size / SYM_SIZE;
        object->local_got = calloc(count, sizeof *object->local_got);
        if (object->local_got == NULL)
            return "cannot allocate local GOT indexes";
    }
    size_t *slot = got_slot(link, object, symbol, (size_t)load_le(rela + 12, 4), name);
    if (*slot != 0)
        return NULL;
    if (link->got_count == SIZE_MAX / 8)
        return "GOT size overflow";
    *slot = ++link->got_count; /* One-based. */
    return NULL;
}

static bool allocate_got(struct link *link)
{
    for (size_t i = 0; i < link->count; ++i) {
        struct object *object = link->objects[i];
        for (unsigned j = 1; j < object->count; ++j) {
            const struct input_section *target = &object->sections[j];
            if (!target->live || !eligible(target))
                continue;
            for (unsigned k = target->first_rela; k != 0; k = object->sections[k].next_rela) {
                const struct input_section *table = &object->sections[k];
                for (uint64_t pos = 0; pos < table->size; pos += RELA_SIZE) {
                    const unsigned char *rela = object->bytes + table->offset + pos;
                    if (!is_gotpcrel((uint32_t)load_le(rela + 8, 4)))
                        continue;
                    const unsigned char *symbol;
                    const char *name;
                    if (!get_relocation_symbol(object, rela, &symbol, &name))
                        return relocation_error(object, target, rela, "", "invalid relocation symbol");
                    const char *error = add_got_entry(link, object, rela, symbol, name);
                    if (error != NULL)
                        return relocation_error(object, target, rela, name, error);
                }
            }
        }
    }
    struct input_section got = {.name = ".got", .type = ELF64_PROGBITS,
                                .flags = ELF64_ALLOC | ELF64_WRITE, .align = 8,
                                .size = link->got_count * UINT64_C(8), .entsize = 8};
    return link->got_count == 0 || add_synthetic(link, got, "bld (GOT)", &link->got);
}

static const char *apply_relocation(const struct link *link, const struct object *object,
                                    struct executable *exe,
                                    const struct input_section *target, const unsigned char *rela,
                                    const unsigned char *symbol, const char *name, uint32_t type)
{
    /* psABI Table 4.9, Appendix B.2 and §10.3. For a static PLT32 call L
     * is S. GOTPCRELX permits ordinary GOT addressing without relaxation. */
    unsigned width;
    switch (type) {
    case R_X86_64_NONE: return NULL;
    case ELF64_R_64:
    case R_X86_64_PC64:
    case R_X86_64_TPOFF64:
        width = 8;
        break;
    case ELF64_R_PC32:
    case ELF64_R_PLT32:
    case ELF64_R_32:
    case ELF64_R_32S:
    case ELF64_R_GOTPCREL:
    case R_X86_64_GOTPCRELX:
    case R_X86_64_REX_GOTPCRELX:
    case ELF64_R_TPOFF32:
        width = 4;
        break;
    default:
        return "unsupported relocation";
    }

    uint64_t offset = load_le(rela, 8);
    if (target->type == ELF64_NOBITS || !fits(offset, width, target->size))
        return "relocation field outside section bytes";
    uint64_t field = target->output_offset + offset;
    uint64_t value;
    bool tls = type == ELF64_R_TPOFF32 || type == R_X86_64_TPOFF64;
    if (is_gotpcrel(type)) {
        const char *error = symbol_address(link, object, symbol, name, false, &value);
        if (error != NULL)
            return error;
        size_t slot = *got_slot(link, object, symbol, (size_t)load_le(rela + 12, 4), name);
        uint64_t got_offset = link->got->output_offset + 8 * (slot - 1);
        store_le(exe->bytes + got_offset, 8, value);
        value = got_offset - field;
    } else {
        const char *error = symbol_address(link, object, symbol, name, tls, &value);
        if (error != NULL)
            return error;
        if (tls)
            value -= link->base + exe->tls.offset + exe->tls_block_size;
        else if (type == ELF64_R_PC32 || type == ELF64_R_PLT32 || type == R_X86_64_PC64)
            value -= link->base + field;
    }
    value += load_le(rela + 16, 8); /* Two's-complement addend, with ELF64 width. */
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
    for (unsigned i = 1; i < object->count; ++i) {
        const struct input_section *table = &object->sections[i];
        if (table->type != SHT_RELA || !eligible(&object->sections[table->info]) ||
            !object->sections[table->info].live)
            continue;
        const struct input_section *target = &object->sections[table->info];
        for (uint64_t pos = 0; pos < table->size; pos += RELA_SIZE) {
            const unsigned char *rela = object->bytes + table->offset + pos;
            uint32_t type = (uint32_t)load_le(rela + 8, 4);
            const unsigned char *symbol;
            const char *name;
            if (!get_relocation_symbol(object, rela, &symbol, &name))
                return relocation_error(object, target, rela, "", "invalid relocation symbol");
            const char *error = apply_relocation(link, object, exe, target, rela, symbol, name, type);
            if (error != NULL)
                return relocation_error(object, target, rela, name, error);
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

static void write_segment(unsigned char *header, uint32_t type, uint32_t flags,
                          const struct load *load, uint64_t base, uint64_t align)
{
    store_le(header, 4, type);
    store_le(header + 4, 4, flags);
    store_le(header + 8, 8, load->offset);
    store_le(header + 16, 8, base + load->offset);
    store_le(header + 24, 8, base + load->offset);
    store_le(header + 32, 8, load->file_end - load->offset);
    store_le(header + 40, 8, load->memory_end - load->offset);
    store_le(header + 48, 8, align);
}

static void write_section(const struct link *link, struct executable *exe,
                          unsigned char **header, const struct input_section *section)
{
    if (section->output_name == 0)
        return;
    memcpy(exe->bytes + exe->strings + section->output_name, section->name,
           strlen(section->name) + 1);
    section_header(*header, section->output_name, section->type, section->flags & ~(uint64_t)SHF_GROUP,
                   link->base + section->output_offset, section->output_offset,
                   section->size, section->align, section->entsize);
    *header += SHDR_SIZE;
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
    store_le(bytes + 56, 2, BASE_PHDR_COUNT + (exe->tls_align != 0));
    store_le(bytes + 58, 2, SHDR_SIZE);
    store_le(bytes + 60, 2, exe->section_count);
    store_le(bytes + 62, 2, exe->section_count - 1);

    const unsigned flags[] = {PF_R | PF_X, PF_R, PF_R | PF_W};
    unsigned char *phdr = bytes + EHDR_SIZE;
    for (unsigned i = 0; i < LOAD_COUNT; ++i) {
        write_segment(phdr, PT_LOAD, flags[i], &exe->loads[i], link->base, PAGE_SIZE);
        phdr += PHDR_SIZE;
    }
    if (exe->tls_align != 0) {
        write_segment(phdr, PT_TLS, PF_R, &exe->tls, link->base, exe->tls_align);
        if (exe->tls.file_end == exe->tls.offset) {
            /* gABI §§7.1 and 7.7. For an empty image, use a congruent
             * offset within the file without padding it to .tbss. */
            uint64_t offset = exe->loads[DATA].file_end & ~(exe->tls_align - 1);
            store_le(phdr + 8, 8, offset);
        }
        phdr += PHDR_SIZE;
    }
    /* LSB Core 5.0 §11.2: p_flags specifies stack permissions. */
    store_le(phdr, 4, PT_GNU_STACK);
    store_le(phdr + 4, 4, PF_R | PF_W);

    memcpy(bytes + exe->strings + 1, ".shstrtab", sizeof ".shstrtab");
    unsigned char *header = bytes + exe->headers + SHDR_SIZE;
    for (size_t i = 0; i < link->script.count; ++i)
        write_section(link, exe, &header, &exe->outputs[i]);
    for (size_t i = 0; i < link->count; ++i) {
        const struct object *object = link->objects[i];
        for (unsigned j = 1; j < object->count; ++j)
            write_section(link, exe, &header, &object->sections[j]);
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

static bool parse_args(int argc, char **argv, struct link_input *inputs, size_t *count,
                       struct link *link, const char **script, const char **output)
{
    bool whole_archive = false;
    size_t group = SIZE_MAX, files = 0;
    for (int i = 1; i < argc; ++i) {
        const char *arg = argv[i];
        if (strcmp(arg, "-o") == 0) {
            if (*output != NULL || i + 1 == argc || argv[i + 1][0] == '\0')
                return false;
            *output = argv[++i];
        } else if (strcmp(arg, "-T") == 0 || strcmp(arg, "--script") == 0) {
            if (*script != NULL || i + 1 == argc || argv[i + 1][0] == '\0')
                return false;
            *script = argv[++i];
        } else if (strcmp(arg, "--gc-sections") == 0) {
            link->gc = true;
        } else if (strcmp(arg, "--no-gc-sections") == 0) {
            link->gc = false;
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

static bool load_script(struct link *link, const char *path)
{
    struct link_input input = {.path = path};
    size_t size = 0;
    bool ok = read_file(&input, &size);
    if (ok && memchr(input.bytes, 0, size) != NULL) {
        fprintf(stderr, "%s: linker script contains a NUL byte\n", path);
        ok = false;
    }
    ok = ok && parse_script(path, (const char *)input.bytes, &link->script);
    free(input.bytes);
    if (!ok)
        return false;
    link->base = link->script.base;
    link->entry = link->script.entry;
    if (link->base % PAGE_SIZE != 0 || link->base > UINT64_MAX - PAGE_SIZE) {
        fprintf(stderr, "%s: invalid page-aligned image base\n", path);
        return false;
    }
    return true;
}

int main(int argc, char **argv)
{
    struct link_input *inputs = calloc((size_t)argc, sizeof *inputs);
    if (inputs == NULL) {
        fputs("bld: cannot allocate link inputs\n", stderr);
        return 1;
    }
    struct link link = {.entry = "_start", .base = IMAGE_BASE};
    struct executable exe = {0};
    const char *script = NULL, *output = NULL;
    size_t count = 0;
    bool ok = parse_args(argc, argv, inputs, &count, &link, &script, &output);
    if (!ok)
        fputs("usage: bld -o OUTPUT [-T SCRIPT] [--gc-sections]\n"
              "       [--whole-archive | --no-whole-archive | --start-group |\n"
              "        --end-group | OBJECT | ARCHIVE] ...\n", stderr);
    ok = ok && (script == NULL || load_script(&link, script)) &&
         include_inputs(&link, inputs, count) && select_live_sections(&link) &&
         allocate_common(&link) && allocate_got(&link);
    if (ok && !layout(&link, &exe)) {
        fputs("bld: cannot lay out executable\n", stderr);
        ok = false;
    }
    ok = ok && entry_address(&link, &exe.entry);
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
    free(exe.outputs);
    free(link.comdats);
    dispose_script(&link.script);
    for (size_t i = 0; i < link.count; ++i) {
        free(link.objects[i]->member_path);
        free(link.objects[i]->local_got);
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
