/* Intel SDM Vol. 2A §§2.1–2.2, Tables 2-2–2-5 and 2-7.
 * Vol. 2 instruction tables are cited alongside the encoders.
 * ELF gABI §§3 and 5 describe the object metadata.
 */
#include "../common/elf64.h"

#include <ctype.h>
#include <errno.h>
#include <stdlib.h>
#include <string.h>

static const struct elf64_section supported_sections[] = {
    {.name = ".text",   .type = ELF64_PROGBITS, .flags = ELF64_ALLOC | ELF64_EXECINSTR, .alignment = 1},
    {.name = ".data",   .type = ELF64_PROGBITS, .flags = ELF64_ALLOC | ELF64_WRITE,     .alignment = 1},
    {.name = ".rodata", .type = ELF64_PROGBITS, .flags = ELF64_ALLOC,                   .alignment = 1},
    {.name = ".bss",    .type = ELF64_NOBITS,   .flags = ELF64_ALLOC | ELF64_WRITE,     .alignment = 1},
    {.name = ".tdata",  .type = ELF64_PROGBITS, .flags = ELF64_ALLOC | ELF64_WRITE | ELF64_TLS,
     .alignment = 1},
    {.name = ".tbss",   .type = ELF64_NOBITS,   .flags = ELF64_ALLOC | ELF64_WRITE | ELF64_TLS,
     .alignment = 1}
};

enum { SECTION_LIMIT = sizeof supported_sections / sizeof supported_sections[0] };

struct bytes {
    unsigned char *data;
    size_t size;
    size_t capacity;
};

struct location {
    size_t line;
    size_t column;
};

struct token {
    const char *at;
    size_t length;
};

struct symbol {
    struct elf64_symbol elf;
    struct location first_use;
    struct location size_at;
};

struct reference {
    size_t symbol;
    size_t section;
    size_t field;
    size_t next_ip;
    enum elf64_reloc_type type;
    int64_t addend;
    struct location at;
};

struct relocations {
    struct elf64_relocation *data;
    size_t count;
    size_t capacity;
};

struct size_expression {
    size_t symbol;
    size_t end;
    size_t start;
    size_t section;
    size_t offset;
    struct location at;
};

struct assembler {
    const char *path;
    size_t line_number;
    struct bytes line;
    struct bytes contents[SECTION_LIMIT];
    struct elf64_section sections[SECTION_LIMIT];
    struct relocations relocations[SECTION_LIMIT];
    size_t section_count;
    size_t current;
    struct symbol *symbols;
    size_t symbol_count;
    size_t symbol_capacity;
    struct reference *references;
    size_t reference_count;
    size_t reference_capacity;
    struct size_expression *sizes;
    size_t size_count;
    size_t size_capacity;
};

struct parser {
    struct assembler *as;
    const char *at;
    bool locked;
};

static _Noreturn void io_error(const char *path)
{
    perror(path);
    exit(EXIT_FAILURE);
}

static _Noreturn void located_error(const struct assembler *as, struct location at,
                                    const char *message)
{
    fprintf(stderr, "%s:%zu:%zu: %s\n", as->path, at.line, at.column, message);
    exit(EXIT_FAILURE);
}

static struct location locate(const struct assembler *as, const char *at)
{
    return (struct location){as->line_number, (size_t)(at - (const char *)as->line.data) + 1};
}

static _Noreturn void source_error(const struct assembler *as, const char *at,
                                   const char *message)
{
    located_error(as, locate(as, at), message);
}

static void *grow(void *data, size_t *capacity, size_t element_size)
{
    if (*capacity > SIZE_MAX / element_size / 2) {
        errno = EFBIG;
        io_error("bas");
    }
    size_t next = *capacity == 0 ? 16 : 2 * *capacity;
    void *expanded = realloc(data, next * element_size);
    if (expanded == NULL)
        io_error("bas");
    *capacity = next;
    return expanded;
}

static void append(struct bytes *buffer, unsigned char byte)
{
    if (buffer->size == buffer->capacity)
        buffer->data = grow(buffer->data, &buffer->capacity, sizeof *buffer->data);
    buffer->data[buffer->size++] = byte;
}

static bool read_line(FILE *source, struct assembler *as)
{
    as->line.size = 0;
    ++as->line_number;

    int ch;
    while ((ch = fgetc(source)) != EOF && ch != '\n') {
        append(&as->line, (unsigned char)ch);
        if (ch == '\0')
            source_error(as, (const char *)&as->line.data[as->line.size - 1], "NUL in source");
    }
    if (ferror(source))
        io_error(as->path);
    if (ch == EOF && as->line.size == 0)
        return false;
    append(&as->line, '\0');
    return true;
}

static char peek(struct parser *p)
{
    while (isspace((unsigned char)*p->at))
        ++p->at;
    return *p->at;
}

static bool take(struct parser *p, char ch)
{
    if (peek(p) != ch)
        return false;
    ++p->at;
    return true;
}

static void expect(struct parser *p, char ch, const char *message)
{
    if (!take(p, ch))
        source_error(p->as, p->at, message);
}

static bool at_end(struct parser *p)
{
    char ch = peek(p);
    return ch == '\0' || ch == ';';
}

static struct token word(struct parser *p)
{
    peek(p);
    const char *start = p->at;
    while (isalnum((unsigned char)*p->at) || *p->at == '_' ||
           *p->at == '.' || *p->at == '$')
        ++p->at;
    return (struct token){start, (size_t)(p->at - start)};
}

static bool is(struct token word, const char *name)
{
    return word.length == strlen(name) && memcmp(word.at, name, word.length) == 0;
}

static bool is_name(struct token word)
{
    return word.length != 0 && !is(word, ".") &&
           (isalpha((unsigned char)*word.at) || *word.at == '_' || *word.at == '.');
}

static bool starts_number(char ch)
{
    return isdigit((unsigned char)ch) || ch == '+' || ch == '-';
}

static uint64_t magnitude(struct parser *p)
{
    if (!isdigit((unsigned char)peek(p)))
        source_error(p->as, p->at, "expected an integer");
    int base = p->at[0] == '0' && (p->at[1] == 'x' || p->at[1] == 'X') ? 16 : 10;
    char *end;
    errno = 0;
    unsigned long long number = strtoull(p->at, &end, base);
    if (errno == ERANGE || number > UINT64_MAX)
        source_error(p->as, p->at, "integer out of range");
    p->at = end;
    return (uint64_t)number;
}

static uint64_t integer(struct parser *p, unsigned width, bool signed_only)
{
    peek(p);
    const char *at = p->at;
    bool negative = take(p, '-');
    if (!negative)
        take(p, '+');

    uint64_t value = magnitude(p);
    uint64_t sign_bit = UINT64_C(1) << (width * 8 - 1);
    uint64_t max = signed_only ? sign_bit - 1 : (sign_bit - 1) * 2 + 1;
    if (value > (negative ? sign_bit : max))
        source_error(p->as, at, "integer out of range for operand width");
    return negative ? 0 - value : value;
}

static uint64_t count(struct parser *p)
{
    take(p, '+');
    return magnitude(p);
}

static void select_section(struct assembler *as, struct token name)
{
    for (size_t i = 0; i < as->section_count; ++i) {
        if (is(name, as->sections[i].name)) {
            as->current = i;
            return;
        }
    }
    for (size_t i = 0; i < SECTION_LIMIT; ++i) {
        if (is(name, supported_sections[i].name)) {
            as->current = as->section_count++;
            as->sections[as->current] = supported_sections[i];
            return;
        }
    }
    source_error(as, name.at, "unsupported section");
}

static size_t symbol_index(struct assembler *as, struct token name)
{
    if (!is_name(name))
        source_error(as, name.at, "expected a symbol name");
    for (size_t i = 0; i < as->symbol_count; ++i)
        if (is(name, as->symbols[i].elf.name))
            return i;

    if (as->symbol_count == as->symbol_capacity)
        as->symbols = grow(as->symbols, &as->symbol_capacity, sizeof *as->symbols);
    char *copy = malloc(name.length + 1);
    if (copy == NULL)
        io_error("bas");
    memcpy(copy, name.at, name.length);
    copy[name.length] = '\0';

    size_t index = as->symbol_count++;
    as->symbols[index] = (struct symbol){.elf.name = copy, .first_use = locate(as, name.at)};
    return index;
}

static void define_label(struct assembler *as, struct token name)
{
    size_t index = symbol_index(as, name);
    struct elf64_symbol *symbol = &as->symbols[index].elf;
    if (symbol->section != 0)
        source_error(as, name.at, "duplicate label");
    symbol->section = (uint16_t)(as->current + 1);
    symbol->value = as->contents[as->current].size;
    if (symbol->type == ELF64_NOTYPE && (as->sections[as->current].flags & ELF64_TLS))
        symbol->type = ELF64_TLS_SYMBOL;
}

static void emit_fill(struct assembler *as, const char *at, uint64_t count, unsigned char byte)
{
    struct bytes *section = &as->contents[as->current];
    if (count > SIZE_MAX - section->size)
        source_error(as, at, "section size out of range");
    if (count == 0)
        return;
    size_t end = section->size + (size_t)count;
    if (as->sections[as->current].type == ELF64_NOBITS) {
        if (byte != 0)
            source_error(as, at, "nonzero data in a NOBITS section");
    } else {
        while (section->capacity < end)
            section->data = grow(section->data, &section->capacity, sizeof *section->data);
        memset(section->data + section->size, byte, (size_t)count);
    }
    section->size = end;
}

static void emit_data(struct assembler *as, const char *at, uint64_t value, unsigned width)
{
    for (unsigned i = 0; i < width; ++i) {
        emit_fill(as, at, 1, (unsigned char)value);
        value >>= 8;
    }
}

static void emit_word(struct bytes *out, uint64_t value, unsigned width)
{
    for (unsigned i = 0; i < width; ++i) {
        append(out, (unsigned char)value);
        value >>= 8;
    }
}

static void emit_reference(struct assembler *as, struct token label, enum elf64_reloc_type type,
                           int64_t addend, unsigned trailing)
{
    size_t index = symbol_index(as, label);
    if (as->reference_count == as->reference_capacity)
        as->references = grow(as->references, &as->reference_capacity, sizeof *as->references);
    struct bytes *section = &as->contents[as->current];
    size_t field = section->size;
    unsigned width = type == ELF64_R_64 ? 8 : 4;
    if (as->sections[as->current].type == ELF64_NOBITS)
        source_error(as, label.at, "relocation requires a section with file contents");
    emit_data(as, label.at, 0, width);
    as->references[as->reference_count++] = (struct reference){
        .symbol = index, .section = as->current, .field = field,
        .next_ip = section->size + trailing, .type = type, .addend = addend,
        .at = locate(as, label.at)
    };
}

static int64_t signed_value(uint64_t magnitude, bool negative)
{
    /* The caller has checked the range, including the INT64_MIN magnitude. */
    if (!negative || magnitude == 0)
        return (int64_t)magnitude;
    return -(int64_t)(magnitude - 1) - 1;
}

static bool add_signed(int64_t left, int64_t right, int64_t *result)
{
    if ((right > 0 && left > INT64_MAX - right) ||
        (right < 0 && left < INT64_MIN - right))
        return false;
    *result = left + right;
    return true;
}

static int64_t parse_addend(struct parser *p)
{
    char sign = peek(p);
    if (sign != '+' && sign != '-')
        return 0;
    const char *at = p->at++;
    uint64_t value = magnitude(p);
    if (value > (uint64_t)INT64_MAX + (sign == '-'))
        source_error(p->as, at, "symbol addend out of range");
    return signed_value(value, sign == '-');
}

static enum elf64_reloc_type relocation_modifier(struct parser *p)
{
    if (!take(p, '@'))
        return 0;
    struct token name = word(p);
    if (is(name, "plt")) return ELF64_R_PLT32;
    if (is(name, "gotpcrel")) return ELF64_R_GOTPCREL;
    if (is(name, "32s")) return ELF64_R_32S;
    if (is(name, "tpoff")) return ELF64_R_TPOFF32;
    source_error(p->as, name.at, "unknown relocation modifier");
}

static void resolve_references(struct assembler *as)
{
    for (size_t i = 0; i < as->reference_count; ++i) {
        const struct reference *ref = &as->references[i];
        const struct elf64_symbol *symbol = &as->symbols[ref->symbol].elf;
        if (symbol->section == 0 && !symbol->global)
            located_error(as, ref->at, "undefined label in symbol expression");
        if ((ref->type == ELF64_R_TPOFF32) != (symbol->type == ELF64_TLS_SYMBOL))
            located_error(as, ref->at, "TPOFF32 requires a TLS symbol and vice versa");

        if (ref->type == ELF64_R_PC32 && symbol->section == ref->section + 1) {
            bool negative = symbol->value < ref->next_ip;
            uint64_t distance = negative ? ref->next_ip - symbol->value
                                         : symbol->value - ref->next_ip;
            int64_t displacement;
            if (distance > (uint64_t)INT64_MAX + negative ||
                !add_signed(signed_value(distance, negative), ref->addend, &displacement) ||
                displacement < INT32_MIN || displacement > INT32_MAX)
                located_error(as, ref->at, "relative displacement out of rel32 range");

            unsigned char *field = as->contents[ref->section].data + ref->field;
            uint32_t bits = (uint32_t)displacement;
            for (unsigned j = 0; j < 4; ++j) {
                field[j] = (unsigned char)bits;
                bits >>= 8;
            }
        } else {
            int64_t addend = ref->addend;
            /* psABI Table 4.9 uses P (the field), while the ISA uses next IP. */
            if ((ref->type == ELF64_R_PC32 || ref->type == ELF64_R_PLT32 ||
                 ref->type == ELF64_R_GOTPCREL) &&
                !add_signed(addend, -(int64_t)(ref->next_ip - ref->field), &addend))
                located_error(as, ref->at, "relative relocation addend out of range");
            struct relocations *relocs = &as->relocations[ref->section];
            if (relocs->count == relocs->capacity)
                relocs->data = grow(relocs->data, &relocs->capacity, sizeof *relocs->data);
            relocs->data[relocs->count++] = (struct elf64_relocation){
                .offset = ref->field, .symbol = ref->symbol, .type = ref->type, .addend = addend
            };
        }
    }
}

struct reg {
    unsigned number;
    unsigned width;
    bool high_byte;
};

static struct reg register_named(struct token name)
{
    static const char *const names[][16] = {
        {"al", "cl", "dl", "bl", "spl", "bpl", "sil", "dil",
         "r8b", "r9b", "r10b", "r11b", "r12b", "r13b", "r14b", "r15b"},
        {"ax", "cx", "dx", "bx", "sp", "bp", "si", "di",
         "r8w", "r9w", "r10w", "r11w", "r12w", "r13w", "r14w", "r15w"},
        {"eax", "ecx", "edx", "ebx", "esp", "ebp", "esi", "edi",
         "r8d", "r9d", "r10d", "r11d", "r12d", "r13d", "r14d", "r15d"},
        {"rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi",
         "r8", "r9", "r10", "r11", "r12", "r13", "r14", "r15"}
    };
    for (unsigned row = 0; row < sizeof names / sizeof names[0]; ++row)
        for (unsigned number = 0; number < 16; ++number)
            if (is(name, names[row][number]))
                return (struct reg){number, 1u << row, false};
    static const char *const highs[] = {"ah", "ch", "dh", "bh"};
    for (unsigned i = 0; i < sizeof highs / sizeof highs[0]; ++i)
        if (is(name, highs[i]))
            return (struct reg){i + 4, 1, true};
    return (struct reg){0};
}

struct address {
    int base;
    int index;
    unsigned scale;       /* SIB ss bits. */
    unsigned width;       /* Operand size, or zero if inferred. */
    unsigned addr_width;  /* Address size, or zero for the default. */
    unsigned char segment;
    bool ip_relative;
    int64_t displacement;
    struct token label;
    enum elf64_reloc_type relocation;
};

static unsigned char segment_prefix(struct token name)
{
    if (is(name, "es")) return 0x26;
    if (is(name, "cs")) return 0x2e;
    if (is(name, "ss")) return 0x36;
    if (is(name, "ds")) return 0x3e;
    if (is(name, "fs")) return 0x64;
    if (is(name, "gs")) return 0x65;
    return 0;
}

static unsigned operand_width(struct token name)
{
    if (is(name, "byte")) return 1;
    if (is(name, "word")) return 2;
    if (is(name, "dword")) return 4;
    if (is(name, "qword")) return 8;
    if (is(name, "tbyte")) return 10;
    if (is(name, "xmmword") || is(name, "oword")) return 16;
    return 0;
}

static unsigned scale_bits(struct parser *p)
{
    peek(p);
    const char *at = p->at;
    switch (magnitude(p)) {
    case 1: return 0;
    case 2: return 1;
    case 4: return 2;
    case 8: return 3;
    default: source_error(p->as, at, "index scale must be 1, 2, 4 or 8");
    }
}

static void address_register(struct parser *p, struct address *address, struct token name,
                             struct reg reg)
{
    if (address->ip_relative || (reg.width != 4 && reg.width != 8))
        source_error(p->as, name.at, "expected a 32-bit or 64-bit base or index");
    if (address->addr_width != 0 && address->addr_width != reg.width)
        source_error(p->as, name.at, "mixed address register widths");
    address->addr_width = reg.width;

    bool scaled = take(p, '*');
    int number = (int)reg.number;
    if (!scaled && address->base < 0) {
        address->base = number;
    } else {
        if (address->index >= 0)
            source_error(p->as, name.at, "too many address registers");
        /* An unscaled stack pointer can be the base instead of the index. */
        if (!scaled && number == 4) {
            number = address->base;
            address->base = 4;
        }
        if (number == 4)
            source_error(p->as, name.at, "stack pointer cannot be an index register");
        address->index = number;
        if (scaled)
            address->scale = scale_bits(p);
    }
}

static void address_term(struct parser *p, struct address *address, bool negative)
{
    if (isdigit((unsigned char)peek(p))) {
        const char *at = p->at;
        uint64_t value = magnitude(p);
        if (value > (uint64_t)INT64_MAX + negative ||
            !add_signed(address->displacement, signed_value(value, negative),
                        &address->displacement))
            source_error(p->as, at, "address displacement out of range");
        return;
    }

    struct token name = word(p);
    if (negative || !is_name(name))
        source_error(p->as, name.at, "expected an address register, label or displacement");
    if (is(name, "rip") || is(name, "eip")) {
        if (address->addr_width != 0)
            source_error(p->as, name.at, "instruction pointer cannot be combined with other registers");
        address->ip_relative = true;
        address->addr_width = is(name, "rip") ? 8 : 4;
        return;
    }
    struct reg reg = register_named(name);
    if (reg.width != 0) {
        address_register(p, address, name, reg);
    } else {
        if (address->label.length != 0)
            source_error(p->as, name.at, "expected at most one address label");
        address->label = name;
        address->relocation = relocation_modifier(p);
    }
}

static struct address memory_operand(struct parser *p, struct token name)
{
    struct address address = {.base = -1, .index = -1, .width = operand_width(name)};
    if (address.width != 0) {
        name = word(p);
        if (is(name, "ptr"))
            name = word(p);
    }
    if (name.length != 0) {
        address.segment = segment_prefix(name);
        if (address.segment == 0)
            source_error(p->as, name.at, "expected a register or memory operand");
        expect(p, ':', "expected ':' after segment override");
    }
    expect(p, '[', "expected '[' before memory address");
    bool negative = take(p, '-');
    if (!negative)
        take(p, '+');
    for (;;) {
        address_term(p, &address, negative);
        if (take(p, ']'))
            break;
        negative = take(p, '-');
        if (!negative)
            expect(p, '+', "expected '+', '-' or ']' in memory address");
    }
    if (address.label.length != 0) {
        if (address.relocation == 0) {
            address.relocation = address.ip_relative ? ELF64_R_PC32
                               : address.addr_width == 4 ? ELF64_R_32 : ELF64_R_32S;
        } else if (address.relocation != ELF64_R_32S &&
                   address.relocation != ELF64_R_TPOFF32 &&
                   address.relocation != ELF64_R_GOTPCREL) {
            source_error(p->as, address.label.at, "unsupported memory relocation");
        } else if (address.ip_relative != (address.relocation == ELF64_R_GOTPCREL)) {
            source_error(p->as, address.label.at, "relocation modifier conflicts with address form");
        }
    }
    int64_t max = address.addr_width == 4 && !address.ip_relative ? UINT32_MAX : INT32_MAX;
    if (address.label.length == 0 &&
        (address.displacement < INT32_MIN || address.displacement > max))
        source_error(p->as, name.at, "memory displacement out of disp32 range");
    return address;
}

struct operand {
    struct token at;
    struct reg reg;
    struct address memory;
};

static struct operand operand(struct parser *p)
{
    struct token name = word(p);
    struct operand result = {.at = name, .reg = register_named(name)};
    if (result.reg.width == 0)
        result.memory = memory_operand(p, name);
    return result;
}

static unsigned width_of(const struct operand *op)
{
    return op->reg.width ? op->reg.width : op->memory.width;
}

static unsigned explicit_width(struct parser *p, const struct operand *op)
{
    unsigned width = width_of(op);
    if (width == 0)
        source_error(p->as, op->at.at, "memory operand requires an explicit width");
    return width;
}

static unsigned require_width(struct parser *p, const struct operand *op)
{
    unsigned width = explicit_width(p, op);
    if (width > 8)
        source_error(p->as, op->at.at, "integer operand width exceeds 64 bits");
    return width;
}

static void match_width(struct parser *p, const struct operand *op, unsigned width)
{
    unsigned actual = width_of(op);
    if (actual != 0 && actual != width)
        source_error(p->as, op->at.at, "operand widths differ");
}

static bool fits_sign_extended(uint64_t value, unsigned from, unsigned to)
{
    /* Compare bit patterns at the destination width, including hex literals. */
    unsigned shift = (8 - from) * 8;
    uint64_t low = (value << shift) >> shift;
    uint64_t sign = UINT64_C(1) << (from * 8 - 1);
    uint64_t extended = (low ^ sign) - sign;
    return ((value ^ extended) << ((8 - to) * 8)) == 0;
}

static void emit_opcode(struct bytes *text, unsigned opcode)
{
    if (opcode > 0xffff)
        append(text, (unsigned char)(opcode >> 16));
    if (opcode > 0xff)
        append(text, (unsigned char)(opcode >> 8));
    append(text, (unsigned char)opcode);
}

static bool needs_rex_byte(struct reg reg)
{
    return reg.width == 1 && reg.number >= 4 && !reg.high_byte;
}

static void prefixes(struct assembler *as, const char *at, struct reg reg,
                     const struct operand *rm, unsigned char rex, unsigned mandatory)
{
    struct bytes *text = &as->contents[as->current];
    if (mandatory == 0xf0)
        append(text, (unsigned char)mandatory);
    if (rm->memory.segment)
        append(text, rm->memory.segment);
    if (mandatory != 0 && mandatory != 0xf0)
        append(text, (unsigned char)mandatory);
    else if (reg.width == 2)
        append(text, 0x66);
    if (rm->memory.addr_width == 4)
        append(text, 0x67);
    if (reg.width == 8)
        rex |= 8;
    if (rex != 0 || needs_rex_byte(reg) || needs_rex_byte(rm->reg)) {
        if (reg.high_byte || rm->reg.high_byte)
            source_error(as, at, "high byte register cannot be encoded with REX");
        append(text, (unsigned char)(0x40 | rex));
    }
}

static void modrm(struct assembler *as, unsigned opcode, struct reg reg,
                  const struct operand *rm, unsigned trailing)
{
    struct bytes *text = &as->contents[as->current];
    unsigned mandatory = opcode >> 16;
    opcode &= 0xffff;
    unsigned char rex = reg.number >= 8 ? 4 : 0;
    unsigned reg_field = (reg.number & 7) << 3;

    if (rm->reg.width != 0) {
        if (rm->reg.number >= 8)
            rex |= 1;
        prefixes(as, rm->at.at, reg, rm, rex, mandatory);
        emit_opcode(text, opcode);
        append(text, (unsigned char)(0xc0 | reg_field | (rm->reg.number & 7)));
        return;
    }

    const struct address *addr = &rm->memory;
    unsigned base = addr->base >= 0 ? (unsigned)addr->base & 7 : 5;
    unsigned index = addr->index >= 0 ? (unsigned)addr->index & 7 : 4;
    unsigned mode = 0;
    unsigned disp_width = 0;
    if (addr->base < 0) {
        disp_width = 4;
    } else if (addr->label.length != 0) {
        mode = 2;
        disp_width = 4;
    } else if (addr->displacement != 0 || base == 5) {
        mode = addr->displacement >= INT8_MIN && addr->displacement <= INT8_MAX ? 1 : 2;
        disp_width = mode == 1 ? 1 : 4;
    }
    bool sib = !addr->ip_relative && (addr->index >= 0 || addr->base < 0 || base == 4);
    if (addr->base >= 8)
        rex |= 1;
    if (addr->index >= 8)
        rex |= 2;
    prefixes(as, rm->at.at, reg, rm, rex, mandatory);
    emit_opcode(text, opcode);
    append(text, (unsigned char)((mode << 6) | reg_field | (sib ? 4 : base)));
    if (sib)
        append(text, (unsigned char)((addr->scale << 6) | (index << 3) | base));
    if (addr->label.length != 0)
        emit_reference(as, addr->label, addr->relocation, addr->displacement, trailing);
    else
        emit_word(text, (uint64_t)addr->displacement, disp_width);
}

/* MOV: B0/B8+rd, C6/C7 /0, 88–8B /r. */
static void encode_mov_immediate(struct parser *p, const struct operand *dest)
{
    struct assembler *as = p->as;
    struct bytes *text = &as->contents[as->current];
    unsigned width = require_width(p, dest);
    uint64_t value = integer(p, width, false);
    bool imm32 = width == 8 && fits_sign_extended(value, 4, 8);
    unsigned immediate = imm32 ? 4 : width;
    if (dest->reg.width != 0 && !imm32) {
        unsigned char rex = dest->reg.number >= 8 ? 1 : 0;
        prefixes(as, dest->at.at, dest->reg, dest, rex, 0);
        unsigned base = width == 1 ? 0xb0 : 0xb8;
        append(text, (unsigned char)(base | (dest->reg.number & 7)));
    } else {
        if (immediate == 8)
            source_error(as, dest->at.at, "qword memory move requires a sign-extended imm32");
        modrm(as, width == 1 ? 0xc6 : 0xc7, (struct reg){.width = width}, dest, immediate);
    }
    emit_word(text, value, immediate);
}

static void encode_mov(struct parser *p)
{
    struct operand dest = operand(p);
    expect(p, ',', "expected ',' between mov operands");
    if (starts_number(peek(p))) {
        encode_mov_immediate(p, &dest);
        return;
    }
    struct operand src = operand(p);

    bool load = src.reg.width == 0;
    struct reg reg = load ? dest.reg : src.reg;
    const struct operand *rm = load ? &src : &dest;
    if (reg.width == 0)
        source_error(p->as, src.at.at, "mov requires a register operand");
    match_width(p, rm, reg.width);
    unsigned opcode = load ? 0x8a : 0x88;
    modrm(p->as, opcode + (reg.width != 1), reg, rm, 0);
}

/* MOVZX/MOVSX: 0F B6/B7 and 0F BE/BF; MOVSXD: REX.W + 63. */
static void encode_extend(struct parser *p, struct token op)
{
    struct operand dest = operand(p);
    expect(p, ',', "expected ',' before extension source");
    struct operand src = operand(p);
    unsigned width = require_width(p, &src);
    unsigned opcode;
    if (is(op, "movsxd")) {
        if (dest.reg.width != 8 || width != 4)
            source_error(p->as, op.at, "movsxd requires a 64-bit register and 32-bit source");
        opcode = 0x63;
    } else {
        if (dest.reg.width <= width || width > 2)
            source_error(p->as, op.at, "extension requires a byte or word source and a wider register");
        opcode = (is(op, "movzx") ? 0x0fb6 : 0x0fbe) + (width == 2);
    }
    modrm(p->as, opcode, dest.reg, &src, 0);
}

static unsigned full_immediate_width(struct parser *p, const char *at, uint64_t value,
                                     unsigned width)
{
    if (width != 8)
        return width;
    if (!fits_sign_extended(value, 4, 8))
        source_error(p->as, at, "64-bit operand requires a sign-extended imm32");
    return 4;
}

/* ADD/OR/ADC/SBB/AND/SUB/XOR/CMP: 00–3B /r and 80/81/83 /digit.
 * TEST: 84/85 /r and F6/F7 /0 (no sign-extended imm8 form).
 */
static void encode_binary(struct parser *p, unsigned group, bool test)
{
    struct operand dest = operand(p);
    expect(p, ',', "expected ',' between operands");
    if (p->locked && dest.reg.width != 0)
        source_error(p->as, dest.at.at, "lock requires a memory destination");
    unsigned lock = p->locked ? 0xf00000 : 0;
    if (starts_number(peek(p))) {
        unsigned width = require_width(p, &dest);
        const char *at = p->at;
        uint64_t value = integer(p, width, false);
        unsigned immediate = full_immediate_width(p, at, value, width);
        unsigned opcode = test ? 0xf6 + (width != 1) : 0x80 + (width != 1);
        if (!test && width != 1 && fits_sign_extended(value, 1, width)) {
            opcode = 0x83;
            immediate = 1;
        }
        modrm(p->as, lock | opcode, (struct reg){.number = group, .width = width}, &dest, immediate);
        emit_word(&p->as->contents[p->as->current], value, immediate);
        return;
    }

    struct operand src = operand(p);
    bool load = src.reg.width == 0 && !test;
    struct reg reg = load ? dest.reg : src.reg;
    const struct operand *rm = load ? &src : &dest;
    if (reg.width == 0)
        source_error(p->as, src.at.at, "expected a register operand");
    match_width(p, rm, reg.width);
    unsigned opcode = test ? 0x84 : group * 8 + (load ? 2 : 0);
    modrm(p->as, lock | (opcode + (reg.width != 1)), reg, rm, 0);
}

static void encode_unary(struct parser *p, unsigned opcode, unsigned digit,
                         const struct operand *rm)
{
    unsigned width = require_width(p, rm);
    if (p->locked && rm->reg.width != 0)
        source_error(p->as, rm->at.at, "lock requires a memory destination");
    unsigned lock = p->locked ? 0xf00000 : 0;
    modrm(p->as, lock | (opcode + (width != 1)),
          (struct reg){.number = digit, .width = width}, rm, 0);
}

/* SAL/SAR/SHL/SHR and RCL/RCR/ROL/ROR: C0/C1, D0–D3 /digit. */
static void encode_shift(struct parser *p, unsigned digit)
{
    struct operand dest = operand(p);
    unsigned width = require_width(p, &dest);
    expect(p, ',', "expected a shift count after ','");
    unsigned opcode = 0xd2;
    unsigned immediate = 0;
    uint64_t value = 0;
    if (starts_number(peek(p))) {
        const char *at = p->at;
        value = count(p);
        if (value > UINT8_MAX)
            source_error(p->as, at, "shift count out of imm8 range");
        opcode = value == 1 ? 0xd0 : 0xc0;
        immediate = value != 1;
    } else {
        struct token cl = word(p);
        if (!is(cl, "cl"))
            source_error(p->as, cl.at, "shift count must be an immediate or cl");
    }
    modrm(p->as, opcode + (width != 1), (struct reg){.number = digit, .width = width},
          &dest, immediate);
    emit_word(&p->as->contents[p->as->current], value, immediate);
}

/* IMUL: F6/F7 /5, 0F AF /r, and 69/6B /r with immediate. */
static void encode_imul(struct parser *p)
{
    struct operand dest = operand(p);
    if (!take(p, ',')) {
        encode_unary(p, 0xf6, 5, &dest);
        return;
    }
    unsigned width = dest.reg.width;
    if (width < 2)
        source_error(p->as, dest.at.at, "imul destination requires a register width of 16, 32 or 64 bits");

    /* In the register/immediate shorthand the register is also the source. */
    bool has_immediate = starts_number(peek(p));
    struct operand src = has_immediate ? dest : operand(p);
    match_width(p, &src, width);
    if (!has_immediate && !take(p, ',')) {
        modrm(p->as, 0x0faf, dest.reg, &src, 0);
        return;
    }
    peek(p);
    const char *at = p->at;
    uint64_t value = integer(p, width, false);
    unsigned immediate = full_immediate_width(p, at, value, width);
    if (fits_sign_extended(value, 1, width))
        immediate = 1;
    modrm(p->as, immediate == 1 ? 0x6b : 0x69, dest.reg, &src, immediate);
    emit_word(&p->as->contents[p->as->current], value, immediate);
}

/* LEA: 8D /r. */
static void encode_lea(struct parser *p)
{
    struct operand dest = operand(p);
    if (dest.reg.width < 2)
        source_error(p->as, dest.at.at, "lea requires a 16-, 32- or 64-bit register");
    expect(p, ',', "expected ',' before lea address");
    struct operand src = operand(p);
    if (src.reg.width != 0)
        source_error(p->as, src.at.at, "lea requires a memory operand");
    match_width(p, &src, dest.reg.width);
    modrm(p->as, 0x8d, dest.reg, &src, 0);
}

/* PUSH/POP: 50/58+rd, FF /6, 8F /0 and PUSH 6A ib or 68 id. */
static void encode_stack(struct parser *p, bool push)
{
    struct assembler *as = p->as;
    struct bytes *text = &as->contents[as->current];
    if (push && starts_number(peek(p))) {
        uint64_t value = integer(p, 4, true);
        unsigned immediate = fits_sign_extended(value, 1, 4) ? 1 : 4;
        append(text, immediate == 1 ? 0x6a : 0x68);
        emit_word(text, value, immediate);
        return;
    }
    struct operand op = operand(p);
    unsigned width = width_of(&op);
    if (width != 0 && width != 2 && width != 8)
        source_error(as, op.at.at, "push/pop requires a 16- or 64-bit operand width");
    if (op.reg.width != 0) {
        if (width == 2)
            append(text, 0x66);
        if (op.reg.number >= 8)
            append(text, 0x41);
        append(text, (unsigned char)((push ? 0x50 : 0x58) + (op.reg.number & 7)));
    } else {
        /* The long-mode default is 64 bits without REX.W. */
        struct reg digit = {.number = push ? 6 : 0, .width = width == 2 ? 2 : 0};
        modrm(as, push ? 0xff : 0x8f, digit, &op, 0);
    }
}

static void branch_displacement(struct parser *p, bool allow_plt)
{
    struct assembler *as = p->as;
    if (starts_number(peek(p))) {
        emit_word(&as->contents[as->current], integer(p, 4, true), 4);
    } else {
        struct token label = word(p);
        if (!is_name(label))
            source_error(as, label.at, "expected a branch label or displacement");
        enum elf64_reloc_type type = relocation_modifier(p);
        if (type != 0 && (type != ELF64_R_PLT32 || !allow_plt))
            source_error(as, label.at, "unsupported branch relocation");
        emit_reference(as, label, type ? type : ELF64_R_PC32, parse_addend(p), 0);
    }
}

/* Near CALL/JMP: E8/E9 cd, or FF /2 or /4 with a 64-bit operand. */
static void encode_transfer(struct parser *p, bool call)
{
    struct parser lookahead = *p;
    struct token name = word(&lookahead);
    char next = peek(&lookahead);
    if ((name.length == 0 && peek(&lookahead) == '[') ||
        (operand_width(name) != 0 && (next == '[' || is_name(word(&lookahead)))) ||
        (segment_prefix(name) != 0 && next == ':') ||
        register_named(name).width != 0) {
        struct operand dest = operand(p);
        unsigned width = width_of(&dest);
        if (width != 0 && width != 8)
            source_error(p->as, dest.at.at, "near call/jmp requires a 64-bit operand width");
        modrm(p->as, 0xff, (struct reg){.number = call ? 2 : 4}, &dest, 0);
    } else {
        append(&p->as->contents[p->as->current], call ? 0xe8 : 0xe9);
        branch_displacement(p, true);
    }
}

/* Jcc, SETcc, CMOVcc use the same condition numbers and aliases. */
static int condition_named(struct token suffix)
{
    static const char *const names[16][3] = {
        {"o"}, {"no"}, {"b", "c", "nae"}, {"ae", "nb", "nc"},
        {"e", "z"}, {"ne", "nz"}, {"be", "na"}, {"a", "nbe"},
        {"s"}, {"ns"}, {"p", "pe"}, {"np", "po"},
        {"l", "nge"}, {"ge", "nl"}, {"le", "ng"}, {"g", "nle"}
    };
    for (unsigned cc = 0; cc < sizeof names / sizeof names[0]; ++cc)
        for (unsigned alias = 0; alias < 3 && names[cc][alias]; ++alias)
            if (is(suffix, names[cc][alias]))
                return (int)cc;
    return -1;
}

static bool encode_condition(struct parser *p, struct token op)
{
    unsigned prefix;
    if (*op.at == 'j') {
        prefix = 1;
    } else if (op.length > 3 && memcmp(op.at, "set", 3) == 0) {
        prefix = 3;
    } else if (op.length > 4 && memcmp(op.at, "cmov", 4) == 0) {
        prefix = 4;
    } else {
        return false;
    }
    int cc = condition_named((struct token){op.at + prefix, op.length - prefix});
    if (cc < 0)
        return false;
    unsigned code = (unsigned)cc;
    struct assembler *as = p->as;
    if (prefix == 1) {
        emit_opcode(&as->contents[as->current], 0x0f80 + code);
        branch_displacement(p, false);
    } else if (prefix == 3) {
        struct operand dest = operand(p);
        unsigned width = width_of(&dest);
        if (width != 0 && width != 1)
            source_error(as, dest.at.at, "setcc requires a byte operand");
        modrm(as, 0x0f90 + code, (struct reg){.width = 1}, &dest, 0);
    } else {
        struct operand dest = operand(p);
        if (dest.reg.width < 2)
            source_error(as, dest.at.at, "cmovcc destination requires a register width of 16, 32 or 64 bits");
        expect(p, ',', "expected ',' before cmovcc source");
        struct operand src = operand(p);
        match_width(p, &src, dest.reg.width);
        modrm(as, 0x0f40 + code, dest.reg, &src, 0);
    }
    return true;
}

static unsigned hex_digit(struct parser *p)
{
    unsigned char ch = (unsigned char)*p->at;
    if (ch >= '0' && ch <= '9') {
        ++p->at;
        return ch - '0';
    }
    ch = (unsigned char)tolower(ch);
    if (ch >= 'a' && ch <= 'f') {
        ++p->at;
        return ch - 'a' + 10;
    }
    source_error(p->as, p->at, "expected two hex digits after \\x");
}

static void emit_string(struct parser *p, bool terminated)
{
    peek(p);
    const char *at = p->at;
    expect(p, '"', "expected a quoted string");
    while (*p->at != '"') {
        if (*p->at == '\0')
            source_error(p->as, p->at, "unterminated string");
        unsigned char ch = (unsigned char)*p->at++;
        if (ch == '\\') {
            ch = (unsigned char)*p->at;
            switch (ch) {
            case '\\': case '"': ++p->at; break;
            case '0': ch = 0; ++p->at; break;
            case 'n': ch = '\n'; ++p->at; break;
            case 'r': ch = '\r'; ++p->at; break;
            case 't': ch = '\t'; ++p->at; break;
            case 'x': {
                ++p->at;
                unsigned high = hex_digit(p);
                ch = (unsigned char)((high << 4) | hex_digit(p));
                break;
            }
            default: source_error(p->as, p->at, "unsupported string escape");
            }
        }
        emit_fill(p->as, at, 1, ch);
    }
    ++p->at;
    if (terminated)
        emit_fill(p->as, at, 1, 0);
}

static enum elf64_reloc_type absolute_relocation(struct parser *p, struct token label,
                                                unsigned width)
{
    enum elf64_reloc_type type = relocation_modifier(p);
    if (width == 8 && type == 0)
        return ELF64_R_64;
    if (width == 4 && (type == 0 || type == ELF64_R_32S))
        return type ? type : ELF64_R_32;
    source_error(p->as, label.at, "unsupported absolute relocation for this field");
}

static void emit_data_list(struct parser *p, unsigned width)
{
    do {
        if (peek(p) == '"' && width == 1) {
            emit_string(p, false);
        } else if (!starts_number(peek(p))) {
            struct token label = word(p);
            enum elf64_reloc_type type = absolute_relocation(p, label, width);
            emit_reference(p->as, label, type, parse_addend(p), 0);
        } else {
            const char *at = p->at;
            emit_data(p->as, at, integer(p, width, false), width);
        }
    } while (take(p, ','));
}

static void align_section(struct parser *p)
{
    struct assembler *as = p->as;
    peek(p);
    const char *at = p->at;
    uint64_t alignment = count(p);
    if (alignment == 0 || (alignment & (alignment - 1)) != 0)
        source_error(as, at, "alignment must be a positive power of two");
    struct elf64_section *section = &as->sections[as->current];
    unsigned char fill = section->flags & ELF64_EXECINSTR ? 0x90 : 0;
    if (take(p, ','))
        fill = (unsigned char)integer(p, 1, false);
    if (section->alignment < alignment)
        section->alignment = alignment;
    uint64_t size = as->contents[as->current].size;
    emit_fill(as, at, (0 - size) & (alignment - 1), fill);
}

static void set_type(struct parser *p)
{
    struct assembler *as = p->as;
    size_t index = symbol_index(as, word(p));
    expect(p, ',', "expected ',' before symbol type");
    take(p, '@');
    struct token kind = word(p);
    unsigned char type;
    if (is(kind, "function") || is(kind, "func"))
        type = ELF64_FUNC;
    else if (is(kind, "object"))
        type = ELF64_OBJECT;
    else if (is(kind, "tls"))
        type = ELF64_TLS_SYMBOL;
    else if (is(kind, "notype"))
        type = ELF64_NOTYPE;
    else
        source_error(as, kind.at, "unsupported symbol type");
    as->symbols[index].elf.type = type;
}

static size_t size_term(struct assembler *as, struct token term)
{
    return is(term, ".") ? SIZE_MAX : symbol_index(as, term);
}

static void set_size(struct parser *p)
{
    struct assembler *as = p->as;
    struct token name = word(p);
    size_t symbol = symbol_index(as, name);
    if (as->symbols[symbol].size_at.line != 0)
        source_error(as, name.at, "duplicate symbol size");
    struct location at = locate(as, name.at);
    as->symbols[symbol].size_at = at;
    expect(p, ',', "expected ',' before symbol size");
    if (starts_number(peek(p))) {
        as->symbols[symbol].elf.size = count(p);
        return;
    }
    size_t end = size_term(as, word(p));
    expect(p, '-', "expected 'END - START' after symbol name");
    size_t start = size_term(as, word(p));
    if (as->size_count == as->size_capacity)
        as->sizes = grow(as->sizes, &as->size_capacity, sizeof *as->sizes);
    as->sizes[as->size_count++] = (struct size_expression){
        .symbol = symbol, .end = end, .start = start, .section = as->current + 1,
        .offset = as->contents[as->current].size, .at = at
    };
}

static struct elf64_symbol size_endpoint(const struct assembler *as,
                                         const struct size_expression *size, size_t symbol)
{
    if (symbol == SIZE_MAX)
        return (struct elf64_symbol){.section = (uint16_t)size->section, .value = size->offset};
    if (as->symbols[symbol].elf.section == 0)
        located_error(as, size->at, "undefined label in symbol size");
    return as->symbols[symbol].elf;
}

static void resolve_sizes(struct assembler *as)
{
    for (size_t i = 0; i < as->size_count; ++i) {
        const struct size_expression *size = &as->sizes[i];
        struct elf64_symbol end = size_endpoint(as, size, size->end);
        struct elf64_symbol start = size_endpoint(as, size, size->start);
        if (start.section != end.section || end.value < start.value)
            located_error(as, size->at, "symbol size requires a nonnegative same-section difference");
        as->symbols[size->symbol].elf.size = end.value - start.value;
    }
}

static bool directive(struct parser *p, struct token op)
{
    if (*op.at == '.') {
        ++op.at;
        --op.length;
    }
    if (is(op, "section")) {
        select_section(p->as, word(p));
    } else if (is(op, "global") || is(op, "globl") || is(op, "extern")) {
        size_t index = symbol_index(p->as, word(p));
        p->as->symbols[index].elf.global = true;
    } else if (is(op, "db") || is(op, "byte")) {
        emit_data_list(p, 1);
    } else if (is(op, "dw") || is(op, "word")) {
        emit_data_list(p, 2);
    } else if (is(op, "dd") || is(op, "long")) {
        emit_data_list(p, 4);
    } else if (is(op, "dq") || is(op, "quad")) {
        emit_data_list(p, 8);
    } else if (is(op, "ascii") || is(op, "asciz")) {
        do {
            emit_string(p, is(op, "asciz"));
        } while (take(p, ','));
    } else if (is(op, "zero")) {
        emit_fill(p->as, op.at, count(p), 0);
    } else if (is(op, "align") || is(op, "balign")) {
        align_section(p);
    } else if (is(op, "type")) {
        set_type(p);
    } else if (is(op, "size")) {
        set_size(p);
    } else {
        return false;
    }
    return true;
}

/* Intel SDM Vol. 2 legacy SSE/SSE2 opcode tables. The high byte of a
 * ModRM opcode holds its mandatory prefix, before the address size and REX.
 */
static struct operand vector_operand(struct parser *p)
{
    static const char *const names[] = {
        "xmm0", "xmm1", "xmm2", "xmm3", "xmm4", "xmm5", "xmm6", "xmm7",
        "xmm8", "xmm9", "xmm10", "xmm11", "xmm12", "xmm13", "xmm14", "xmm15"
    };
    struct parser lookahead = *p;
    struct token name = word(&lookahead);
    for (unsigned i = 0; i < sizeof names / sizeof names[0]; ++i) {
        if (is(name, names[i])) {
            *p = lookahead;
            return (struct operand){.at = name, .reg = {.number = i, .width = 16}};
        }
    }
    return operand(p);
}

static void match_vector_rm(struct parser *p, const struct operand *rm, unsigned width)
{
    if (rm->reg.width != 0 && rm->reg.width != 16)
        source_error(p->as, rm->at.at, "expected an XMM register or memory");
    if (rm->reg.width == 0)
        match_width(p, rm, width);
}

static void encode_vector_pair(struct parser *p, unsigned load, unsigned store,
                               unsigned width, bool immediate)
{
    struct operand dest = vector_operand(p);
    expect(p, ',', "expected ',' between SSE operands");
    struct operand src = vector_operand(p);
    bool storing = store != 0 && dest.reg.width == 0;
    struct reg xmm = storing ? src.reg : dest.reg;
    const struct operand *rm = storing ? &dest : &src;
    if (xmm.width != 16)
        source_error(p->as, dest.at.at, "SSE instruction requires an XMM register");
    match_vector_rm(p, rm, width);
    uint64_t value = 0;
    if (immediate) {
        expect(p, ',', "expected ',' before SSE immediate");
        if (peek(p) == '-')
            source_error(p->as, p->at, "expected an unsigned SSE immediate");
        value = integer(p, 1, false);
    }
    modrm(p->as, storing ? store : load, xmm, rm, immediate);
    if (immediate)
        emit_word(&p->as->contents[p->as->current], value, 1);
}

static void encode_movd_q(struct parser *p, bool quad)
{
    struct operand dest = vector_operand(p);
    expect(p, ',', "expected ',' between SSE move operands");
    struct operand src = vector_operand(p);
    unsigned width = quad ? 8 : 4;
    bool load = dest.reg.width == 16;
    struct reg xmm = load ? dest.reg : src.reg;
    const struct operand *rm = load ? &src : &dest;
    if (xmm.width != 16)
        source_error(p->as, dest.at.at, "SSE move requires an XMM register");
    if (quad && (rm->reg.width == 0 || rm->reg.width == 16)) {
        /* MOVQ xmm,xmm/m64 is F3 0F 7E; MOVQ m64,xmm is 66 0F D6. */
        match_vector_rm(p, rm, width);
        modrm(p->as, load ? 0xf30f7e : 0x660fd6, xmm, rm, 0);
    } else {
        match_width(p, rm, width);
        xmm.width = width;
        modrm(p->as, load ? 0x660f6e : 0x660f7e, xmm, rm, 0);
    }
}

static void encode_scalar_integer(struct parser *p, unsigned opcode, unsigned scalar_width,
                                  bool to_integer)
{
    struct operand dest = vector_operand(p);
    expect(p, ',', "expected ',' between conversion operands");
    struct operand src = vector_operand(p);
    if (to_integer && dest.reg.width != 4 && dest.reg.width != 8)
        source_error(p->as, dest.at.at, "conversion requires a 32-bit or 64-bit integer register");
    const struct operand *rm = &src;
    struct reg reg = dest.reg;
    if (to_integer) {
        match_vector_rm(p, rm, scalar_width);
    } else {
        if (dest.reg.width != 16)
            source_error(p->as, dest.at.at, "conversion requires an XMM destination");
        reg.width = explicit_width(p, rm);
        if (reg.width != 4 && reg.width != 8)
            source_error(p->as, rm->at.at, "conversion requires a 32-bit or 64-bit integer source");
    }
    modrm(p->as, opcode, reg, rm, 0);
}

static bool encode_sse(struct parser *p, struct token op)
{
    static const struct {
        const char *name;
        unsigned load;
        unsigned store;
        unsigned width;
    } moves[] = {
        {"movss", 0xf30f10, 0xf30f11, 4}, {"movsd", 0xf20f10, 0xf20f11, 8},
        {"movups", 0x0f10, 0x0f11, 16}, {"movupd", 0x660f10, 0x660f11, 16},
        {"movaps", 0x0f28, 0x0f29, 16}, {"movapd", 0x660f28, 0x660f29, 16},
        {"movdqa", 0x660f6f, 0x660f7f, 16}, {"movdqu", 0xf30f6f, 0xf30f7f, 16}
    };
    static const struct {
        const char *stem;
        unsigned opcode;
        bool scalar;
    } float_ops[] = {
        {"add", 0x0f58, true}, {"sub", 0x0f5c, true}, {"mul", 0x0f59, true},
        {"div", 0x0f5e, true}, {"sqrt", 0x0f51, true}, {"min", 0x0f5d, true},
        {"max", 0x0f5f, true}, {"cmp", 0x0fc2, true},
        {"and", 0x0f54, false}, {"andn", 0x0f55, false},
        {"or", 0x0f56, false}, {"xor", 0x0f57, false}
    };
    static const struct {
        const char *name;
        unsigned opcode;
        unsigned width;
        bool immediate;
    } pairs[] = {
        {"comiss", 0x0f2f, 4, false}, {"comisd", 0x660f2f, 8, false},
        {"ucomiss", 0x0f2e, 4, false}, {"ucomisd", 0x660f2e, 8, false},
        {"cvtss2sd", 0xf30f5a, 4, false}, {"cvtsd2ss", 0xf20f5a, 8, false},
        {"shufps", 0x0fc6, 16, true}, {"shufpd", 0x660fc6, 16, true},
        {"pshufd", 0x660f70, 16, true},
        {"punpcklbw", 0x660f60, 16, false}, {"punpcklwd", 0x660f61, 16, false},
        {"punpckldq", 0x660f62, 16, false}, {"punpcklqdq", 0x660f6c, 16, false},
        {"unpcklps", 0x0f14, 16, false}, {"unpcklpd", 0x660f14, 16, false},
        {"pcmpeqb", 0x660f74, 16, false}, {"pcmpeqw", 0x660f75, 16, false},
        {"pcmpeqd", 0x660f76, 16, false},
        {"paddb", 0x660ffc, 16, false}, {"paddw", 0x660ffd, 16, false},
        {"paddd", 0x660ffe, 16, false}, {"paddq", 0x660fd4, 16, false},
        {"psubb", 0x660ff8, 16, false}, {"psubw", 0x660ff9, 16, false},
        {"psubd", 0x660ffa, 16, false}, {"psubq", 0x660ffb, 16, false},
        {"pand", 0x660fdb, 16, false}, {"pandn", 0x660fdf, 16, false},
        {"por", 0x660feb, 16, false}, {"pxor", 0x660fef, 16, false}
    };
    static const struct {
        const char *name;
        unsigned opcode;
        unsigned width;
        bool to_integer;
    } converts[] = {
        {"cvtsi2ss", 0xf30f2a, 4, false}, {"cvtsi2sd", 0xf20f2a, 8, false},
        {"cvtss2si", 0xf30f2d, 4, true}, {"cvtsd2si", 0xf20f2d, 8, true},
        {"cvttss2si", 0xf30f2c, 4, true}, {"cvttsd2si", 0xf20f2c, 8, true}
    };
    if (is(op, "movd") || is(op, "movq")) {
        encode_movd_q(p, is(op, "movq"));
        return true;
    }
    for (size_t i = 0; i < sizeof moves / sizeof moves[0]; ++i) {
        if (is(op, moves[i].name)) {
            encode_vector_pair(p, moves[i].load, moves[i].store, moves[i].width, false);
            return true;
        }
    }
    if (op.length > 2) {
        struct token stem = {op.at, op.length - 2};
        struct token suffix = {op.at + stem.length, 2};
        for (size_t i = 0; i < sizeof float_ops / sizeof float_ops[0]; ++i) {
            if (!is(stem, float_ops[i].stem))
                continue;
            unsigned opcode = float_ops[i].opcode;
            unsigned width = 16;
            if (is(suffix, "pd")) {
                opcode |= 0x660000;
            } else if (is(suffix, "ss") && float_ops[i].scalar) {
                opcode |= 0xf30000;
                width = 4;
            } else if (is(suffix, "sd") && float_ops[i].scalar) {
                opcode |= 0xf20000;
                width = 8;
            } else if (!is(suffix, "ps")) {
                break;
            }
            encode_vector_pair(p, opcode, 0, width, is(stem, "cmp"));
            return true;
        }
    }
    for (size_t i = 0; i < sizeof pairs / sizeof pairs[0]; ++i) {
        if (is(op, pairs[i].name)) {
            encode_vector_pair(p, pairs[i].opcode, 0, pairs[i].width, pairs[i].immediate);
            return true;
        }
    }
    for (size_t i = 0; i < sizeof converts / sizeof converts[0]; ++i) {
        if (is(op, converts[i].name)) {
            encode_scalar_integer(p, converts[i].opcode, converts[i].width, converts[i].to_integer);
            return true;
        }
    }
    return false;
}

/* Intel SDM Vol. 2 x87 opcode tables. An x87 memory width selects the
 * opcode or digit; it does not set 66 or REX.W.
 */
static int take_x87_register(struct parser *p)
{
    struct parser lookahead = *p;
    struct token name = word(&lookahead);
    if (is(name, "st")) {
        *p = lookahead;
        if (!take(p, '('))
            return 0;
        uint64_t number = count(p);
        if (number > 7)
            source_error(p->as, name.at, "x87 stack register index exceeds 7");
        expect(p, ')', "expected ')' after x87 register index");
        return (int)number;
    }
    if (name.length == 3 && name.at[0] == 's' && name.at[1] == 't' &&
        name.at[2] >= '0' && name.at[2] <= '7') {
        *p = lookahead;
        return name.at[2] - '0';
    }
    return -1;
}

static unsigned x87_register(struct parser *p)
{
    int reg = take_x87_register(p);
    if (reg < 0)
        source_error(p->as, p->at, "expected an x87 stack register");
    return (unsigned)reg;
}

static struct operand required_memory(struct parser *p, unsigned width)
{
    struct operand rm = operand(p);
    if (rm.reg.width != 0)
        source_error(p->as, rm.at.at, "expected a memory operand");
    if (width != 0)
        match_width(p, &rm, width);
    return rm;
}

static void encode_x87_memory(struct parser *p, const struct operand *rm, unsigned encoding)
{
    if (encoding == 0)
        source_error(p->as, rm->at.at, "unsupported x87 memory width");
    modrm(p->as, encoding >> 4, (struct reg){.number = encoding & 7}, rm, 0);
}

static bool encode_x87(struct parser *p, struct token op)
{
    static const struct {
        const char *name;
        unsigned reg_opcode;
        unsigned m16, m32, m64, m80;  /* Opcode in high bits, ModRM digit below. */
    } operands[] = {
        {"fld", 0xd9c0, 0, 0xd90, 0xdd0, 0xdb5},
        {"fst", 0xddd0, 0, 0xd92, 0xdd2, 0},
        {"fstp", 0xddd8, 0, 0xd93, 0xdd3, 0xdb7},
        {"fild", 0, 0xdf0, 0xdb0, 0xdf5, 0},
        {"fist", 0, 0xdf2, 0xdb2, 0, 0},
        {"fistp", 0, 0xdf3, 0xdb3, 0xdf7, 0},
        {"fcom", 0xd8d0, 0, 0xd82, 0xdc2, 0},
        {"fcomp", 0xd8d8, 0, 0xd83, 0xdc3, 0},
        {"fiadd", 0, 0xde0, 0xda0, 0, 0},
        {"fimul", 0, 0xde1, 0xda1, 0, 0},
        {"ficom", 0, 0xde2, 0xda2, 0, 0},
        {"ficomp", 0, 0xde3, 0xda3, 0, 0},
        {"fisub", 0, 0xde4, 0xda4, 0, 0},
        {"fisubr", 0, 0xde5, 0xda5, 0, 0},
        {"fidiv", 0, 0xde6, 0xda6, 0, 0},
        {"fidivr", 0, 0xde7, 0xda7, 0, 0},
        {"fucom", 0xdde0, 0, 0, 0, 0},
        {"fucomp", 0xdde8, 0, 0, 0, 0},
        {"fxch", 0xd9c8, 0, 0, 0, 0},
        {"ffree", 0xddc0, 0, 0, 0, 0}
    };
    static const struct {
        const char *name;
        unsigned digit;
        unsigned dest_i_digit;
    } arithmetic[] = {
        {"fadd", 0, 0}, {"fmul", 1, 1}, {"fsub", 4, 5},
        {"fsubr", 5, 4}, {"fdiv", 6, 7}, {"fdivr", 7, 6}
    };
    static const struct {
        const char *name;
        unsigned opcode;
    } compares[] = {
        {"fcomi", 0xdbf0}, {"fcomip", 0xdff0},
        {"fucomi", 0xdbe8}, {"fucomip", 0xdfe8}
    };
    struct bytes *text = &p->as->contents[p->as->current];
    for (size_t i = 0; i < sizeof operands / sizeof operands[0]; ++i) {
        if (!is(op, operands[i].name))
            continue;
        unsigned opcode = operands[i].reg_opcode;
        int reg = take_x87_register(p);
        if (reg < 0 && at_end(p) &&
            (is(op, "fxch") || is(op, "fcom") || is(op, "fcomp") ||
             is(op, "fucom") || is(op, "fucomp")))
            reg = 1;
        if (reg >= 0) {
            if (opcode == 0)
                source_error(p->as, op.at, "x87 instruction requires a memory operand");
            emit_opcode(text, opcode + (unsigned)reg);
        } else {
            struct operand rm = required_memory(p, 0);
            unsigned width = explicit_width(p, &rm);
            unsigned encoding = width == 2 ? operands[i].m16 :
                                width == 4 ? operands[i].m32 :
                                width == 8 ? operands[i].m64 :
                                width == 10 ? operands[i].m80 : 0;
            encode_x87_memory(p, &rm, encoding);
        }
        return true;
    }
    bool pop = op.length > 1 && op.at[op.length - 1] == 'p';
    struct token stem = {op.at, op.length - pop};
    for (size_t i = 0; i < sizeof arithmetic / sizeof arithmetic[0]; ++i) {
        if (!is(stem, arithmetic[i].name))
            continue;
        unsigned digit = arithmetic[i].digit;
        unsigned dest_i = 0xc0 + 8 * arithmetic[i].dest_i_digit;
        if (pop && at_end(p)) {
            emit_opcode(text, 0xde00 + dest_i + 1);
        } else {
            int dest = take_x87_register(p);
            if (dest < 0 && !pop) {
                struct operand rm = required_memory(p, 0);
                unsigned width = explicit_width(p, &rm);
                unsigned opcode = width == 4 ? 0xd8 : width == 8 ? 0xdc : 0;
                encode_x87_memory(p, &rm, opcode ? (opcode << 4) | digit : 0);
            } else {
                if (dest < 0)
                    source_error(p->as, p->at, "expected an x87 stack destination");
                expect(p, ',', "expected ',' between x87 operands");
                unsigned src = x87_register(p);
                if (src == 0 && (dest != 0 || pop))
                    emit_opcode(text, (pop ? 0xde00 : 0xdc00) + dest_i + (unsigned)dest);
                else if (!pop && dest == 0)
                    emit_opcode(text, 0xd8c0 + 8 * digit + src);
                else
                    source_error(p->as, op.at, "x87 arithmetic requires ST(0) as an operand");
            }
        }
        return true;
    }
    for (size_t i = 0; i < sizeof compares / sizeof compares[0]; ++i) {
        if (is(op, compares[i].name)) {
            if (x87_register(p) != 0)
                source_error(p->as, op.at, "x87 flag comparison requires ST(0) first");
            expect(p, ',', "expected ',' between x87 operands");
            emit_opcode(text, compares[i].opcode + x87_register(p));
            return true;
        }
    }
    if (is(op, "fldcw") || is(op, "fnstcw") || is(op, "fstcw")) {
        struct operand rm = required_memory(p, 2);
        if (is(op, "fstcw"))
            append(text, 0x9b);
        encode_x87_memory(p, &rm, is(op, "fldcw") ? 0xd95 : 0xd97);
    } else if (is(op, "fnstsw") || is(op, "fstsw")) {
        struct operand rm = operand(p);
        if (is(op, "fstsw"))
            append(text, 0x9b);
        if (rm.reg.width == 2 && rm.reg.number == 0) {
            emit_opcode(text, 0xdfe0);
        } else {
            if (rm.reg.width != 0)
                source_error(p->as, rm.at.at, "x87 status destination requires AX or m16");
            match_width(p, &rm, 2);
            encode_x87_memory(p, &rm, 0xdd7);
        }
    } else {
        return false;
    }
    return true;
}

/* Intel SDM Vol. 2: LOCK, XADD, XCHG and CMPXCHG opcode tables. */
static void encode_exchange(struct parser *p, unsigned opcode)
{
    struct operand dest = operand(p);
    expect(p, ',', "expected ',' between exchange operands");
    struct operand src = operand(p);
    const struct operand *rm = &dest;
    struct reg reg = src.reg;
    if (opcode == 0x86 && reg.width == 0) {
        rm = &src;
        reg = dest.reg;
    }
    if (reg.width == 0)
        source_error(p->as, src.at.at, "exchange requires a register source");
    if (p->locked && rm->reg.width != 0)
        source_error(p->as, dest.at.at, "lock requires a memory destination");
    match_width(p, rm, reg.width);
    unsigned lock = p->locked ? 0xf00000 : 0;
    modrm(p->as, lock | (opcode + (reg.width != 1)), reg, rm, 0);
}

/* Intel SDM Vol. 2: BT/BTC/BTR/BTS register and immediate forms. */
static void encode_bit(struct parser *p, unsigned digit, unsigned opcode)
{
    struct operand dest = operand(p);
    if (p->locked && dest.reg.width != 0)
        source_error(p->as, dest.at.at, "lock requires a memory destination");
    unsigned lock = p->locked ? 0xf00000 : 0;
    expect(p, ',', "expected ',' before bit index");
    if (starts_number(peek(p))) {
        unsigned width = require_width(p, &dest);
        if (width == 1)
            source_error(p->as, dest.at.at, "bit operation requires at least 16 bits");
        if (peek(p) == '-')
            source_error(p->as, p->at, "expected an unsigned bit index");
        uint64_t index = integer(p, 1, false);
        modrm(p->as, lock | 0x0fba, (struct reg){.width = width, .number = digit}, &dest, 1);
        emit_word(&p->as->contents[p->as->current], index, 1);
    } else {
        struct operand src = operand(p);
        if (src.reg.width < 2)
            source_error(p->as, src.at.at, "bit index requires a 16-, 32- or 64-bit register");
        match_width(p, &dest, src.reg.width);
        modrm(p->as, lock | opcode, src.reg, &dest, 0);
    }
}

/* Intel SDM Vol. 2 MOVS, CMPS, STOS, LODS, SCAS and REP/REPE/REPNE. */
static bool encode_string_instruction(struct parser *p, struct token op)
{
    static const struct {
        const char *name;
        unsigned opcode;
        bool compares;
    } strings[] = {
        {"movs", 0xa4, false}, {"cmps", 0xa6, true}, {"stos", 0xaa, false},
        {"lods", 0xac, false}, {"scas", 0xae, true}
    };
    unsigned repeat = 0;
    if (is(op, "rep"))
        repeat = 0xf3;
    else if (is(op, "repe") || is(op, "repz"))
        repeat = 0xf3;
    else if (is(op, "repne") || is(op, "repnz"))
        repeat = 0xf2;
    bool conditional = repeat != 0 && !is(op, "rep");
    if (repeat != 0)
        op = word(p);
    if (op.length == 5) {
        struct token stem = {op.at, 4};
        char suffix = op.at[4];
        if (suffix == 'b' || suffix == 'w' || suffix == 'd' || suffix == 'q') {
            for (size_t i = 0; i < sizeof strings / sizeof strings[0]; ++i) {
                if (!is(stem, strings[i].name))
                    continue;
                if (repeat != 0 && conditional != strings[i].compares)
                    source_error(p->as, op.at, "repeat prefix conflicts with string operation");
                if (repeat == 0 && !at_end(p))
                    return false;  /* MOVSD and CMPSD also have SSE forms. */
                struct bytes *text = &p->as->contents[p->as->current];
                if (repeat != 0)
                    append(text, (unsigned char)repeat);
                if (suffix == 'w')
                    append(text, 0x66);
                else if (suffix == 'q')
                    append(text, 0x48);
                append(text, (unsigned char)(strings[i].opcode + (suffix != 'b')));
                return true;
            }
        }
    }
    if (repeat != 0)
        source_error(p->as, op.at, "repeat prefix requires a string instruction");
    return false;
}

static void instruction(struct parser *p, struct token op)
{
    static const char *const binary[] = {"add", "or", "adc", "sbb", "and", "sub", "xor", "cmp"};
    static const struct {
        const char *name;
        unsigned digit;
    } shifts[] = {
        {"rol", 0}, {"ror", 1}, {"rcl", 2}, {"rcr", 3},
        {"shl", 4}, {"sal", 4}, {"shr", 5}, {"sar", 7}
    };
    /* INC, DEC: FE/FF. NOT, NEG, MUL, DIV, IDIV: F6/F7. */
    static const struct {
        const char *name;
        unsigned opcode;
        unsigned digit;
    } unary[] = {
        {"inc", 0xfe, 0}, {"dec", 0xfe, 1}, {"not", 0xf6, 2}, {"neg", 0xf6, 3},
        {"mul", 0xf6, 4}, {"div", 0xf6, 6}, {"idiv", 0xf6, 7}
    };
    /* CBW/CWDE/CDQE, CWD/CDQ/CQO, LEAVE, CLD, STD, NOP and SYSCALL. */
    static const struct {
        const char *name;
        unsigned opcode;
    } fixed[] = {
        {"cbw", 0x6698}, {"cwde", 0x98}, {"cdqe", 0x4898},
        {"cwd", 0x6699}, {"cdq", 0x99}, {"cqo", 0x4899},
        {"leave", 0xc9}, {"cld", 0xfc}, {"std", 0xfd}, {"nop", 0x90},
        {"syscall", 0x0f05},
        {"lfence", 0x0faee8}, {"mfence", 0x0faef0}, {"sfence", 0x0faef8},
        {"fwait", 0x9b}, {"fninit", 0xdbe3}, {"finit", 0x9bdbe3},
        {"fnclex", 0xdbe2}, {"fclex", 0x9bdbe2},
        {"fld1", 0xd9e8}, {"fldl2t", 0xd9e9}, {"fldl2e", 0xd9ea},
        {"fldpi", 0xd9eb}, {"fldlg2", 0xd9ec}, {"fldln2", 0xd9ed}, {"fldz", 0xd9ee},
        {"fchs", 0xd9e0}, {"fabs", 0xd9e1}, {"ftst", 0xd9e4}, {"fxam", 0xd9e5},
        {"fsqrt", 0xd9fa}, {"frndint", 0xd9fc}, {"fprem", 0xd9f8}, {"fprem1", 0xd9f5},
        {"fcompp", 0xded9}, {"fucompp", 0xdae9}
    };
    struct assembler *as = p->as;
    struct bytes *text = &as->contents[as->current];
    if (as->current != 0)
        source_error(as, op.at, "instructions require .text");
    if (is(op, "lock")) {
        p->locked = true;
        op = word(p);
    }
    if (is(op, "xchg") || is(op, "xadd") || is(op, "cmpxchg")) {
        encode_exchange(p, is(op, "xchg") ? 0x86 : is(op, "xadd") ? 0x0fc0 : 0x0fb0);
        return;
    }
    if (is(op, "cmpxchg8b") || is(op, "cmpxchg16b")) {
        bool wide = is(op, "cmpxchg16b");
        struct operand rm = required_memory(p, wide ? 16 : 8);
        unsigned lock = p->locked ? 0xf00000 : 0;
        modrm(as, lock | 0x0fc7, (struct reg){.width = wide ? 8 : 0, .number = 1}, &rm, 0);
        return;
    }
    if (is(op, "btc") || is(op, "btr") || is(op, "bts") || (!p->locked && is(op, "bt"))) {
        unsigned digit = is(op, "bt") ? 4 : is(op, "bts") ? 5 : is(op, "btr") ? 6 : 7;
        unsigned opcode = digit == 7 ? 0x0fbb : 0x0fa3 + 8 * (digit - 4);
        encode_bit(p, digit, opcode);
        return;
    }
    for (unsigned i = 0; i < sizeof binary / sizeof binary[0]; ++i) {
        if (is(op, binary[i]) && (!p->locked || i != 7)) {
            encode_binary(p, i, false);
            return;
        }
    }
    for (size_t i = 0; i < sizeof unary / sizeof unary[0]; ++i) {
        if (is(op, unary[i].name) && (!p->locked || i < 4)) {
            struct operand rm = operand(p);
            encode_unary(p, unary[i].opcode, unary[i].digit, &rm);
            return;
        }
    }
    if (p->locked)
        source_error(as, op.at, "instruction does not accept lock");
    if (encode_string_instruction(p, op))
        return;
    if (is(op, "mov")) {
        encode_mov(p);
    } else if (is(op, "movsx") || is(op, "movzx") || is(op, "movsxd")) {
        encode_extend(p, op);
    } else if (is(op, "test")) {
        encode_binary(p, 0, true);
    } else if (is(op, "imul")) {
        encode_imul(p);
    } else if (is(op, "lea")) {
        encode_lea(p);
    } else if (is(op, "push") || is(op, "pop")) {
        encode_stack(p, is(op, "push"));
    } else if (is(op, "call") || is(op, "jmp")) {
        encode_transfer(p, is(op, "call"));
    } else if (is(op, "ret")) {
        /* RET: C3 or C2 iw for an unsigned stack adjustment. */
        if (at_end(p)) {
            append(text, 0xc3);
        } else {
            const char *at = p->at;
            if (peek(p) == '-')
                source_error(as, at, "ret requires an unsigned imm16");
            uint64_t size = integer(p, 2, false);
            append(text, 0xc2);
            emit_word(text, size, 2);
        }
    } else if (is(op, "ldmxcsr") || is(op, "stmxcsr")) {
        struct operand rm = required_memory(p, 4);
        unsigned digit = is(op, "ldmxcsr") ? 2 : 3;
        modrm(as, 0x0fae, (struct reg){.number = digit}, &rm, 0);
    } else if (!encode_condition(p, op) && !encode_sse(p, op) && !encode_x87(p, op)) {
        for (size_t i = 0; i < sizeof shifts / sizeof shifts[0]; ++i) {
            if (is(op, shifts[i].name)) {
                encode_shift(p, shifts[i].digit);
                return;
            }
        }
        for (size_t i = 0; i < sizeof fixed / sizeof fixed[0]; ++i) {
            if (is(op, fixed[i].name)) {
                emit_opcode(text, fixed[i].opcode);
                return;
            }
        }
        source_error(as, op.at, "unsupported instruction or directive");
    }
}

static void assemble_line(struct assembler *as)
{
    struct parser p = {.as = as, .at = (const char *)as->line.data};
    while (!at_end(&p)) {
        struct token op = word(&p);
        if (take(&p, ':')) {
            define_label(as, op);
            continue;
        }
        if (!directive(&p, op))
            instruction(&p, op);
        if (!at_end(&p))
            source_error(as, p.at, "unexpected input after operands");
        break;
    }
}

static struct elf64_symbol *object_symbols(struct assembler *as)
{
    if (as->symbol_count == 0)
        return NULL;
    /* symbol_count is bounded by the larger struct symbol allocation. */
    struct elf64_symbol *symbols = malloc(as->symbol_count * sizeof *symbols);
    if (symbols == NULL)
        io_error("bas");
    for (size_t i = 0; i < as->symbol_count; ++i) {
        const struct symbol *symbol = &as->symbols[i];
        symbols[i] = symbol->elf;
        if (symbol->elf.section == 0) {
            if (!symbol->elf.global)
                located_error(as, symbol->first_use, "undefined local symbol");
            if (symbol->elf.size != 0)
                located_error(as, symbol->size_at, "undefined symbol cannot have a size");
        } else {
            size_t section_index = symbol->elf.section - 1;
            if ((symbol->elf.type == ELF64_TLS_SYMBOL) !=
                ((as->sections[section_index].flags & ELF64_TLS) != 0))
                located_error(as, symbol->first_use, "TLS symbol requires a TLS section and vice versa");
            size_t section_size = as->contents[section_index].size;
            if (symbol->elf.size > section_size - symbol->elf.value)
                located_error(as, symbol->size_at, "symbol size extends beyond its section");
        }
    }
    return symbols;
}

int main(int argc, char **argv)
{
    if (argc != 4 || strcmp(argv[1], "-o") != 0 || argv[3][0] == '-') {
        fputs("usage: bas -o OBJECT SOURCE\n", stderr);
        return EXIT_FAILURE;
    }

    const char *output = argv[2];
    struct assembler as = {.path = argv[3], .section_count = 1};
    as.sections[0] = supported_sections[0];
    FILE *source = fopen(as.path, "rb");
    if (source == NULL)
        io_error(as.path);
    while (read_line(source, &as))
        assemble_line(&as);
    if (fclose(source) == EOF)
        io_error(as.path);

    resolve_references(&as);
    resolve_sizes(&as);
    struct elf64_symbol *symbols = object_symbols(&as);
    for (size_t i = 0; i < as.section_count; ++i) {
        as.sections[i].data = as.contents[i].data;
        as.sections[i].size = as.contents[i].size;
        as.sections[i].relocations = as.relocations[i].data;
        as.sections[i].relocation_count = as.relocations[i].count;
    }
    FILE *object = fopen(output, "wb");
    if (object == NULL)
        io_error(output);
    if (!elf64_write_rel(object, as.sections, as.section_count, symbols, as.symbol_count) ||
        fclose(object) == EOF)
        io_error(output);

    for (size_t i = 0; i < as.symbol_count; ++i)
        free((void *)symbols[i].name);
    free(symbols);
    free(as.symbols);
    free(as.references);
    free(as.sizes);
    for (size_t i = 0; i < as.section_count; ++i) {
        free(as.contents[i].data);
        free(as.relocations[i].data);
    }
    free(as.line.data);
    return EXIT_SUCCESS;
}
