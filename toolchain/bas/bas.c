/* Intel SDM Vol. 2A §§2.1–2.2, Tables 2-2–2-5 and 2-7, and the MOV,
 * CALL, JMP, Jcc, NOP, RET and SYSCALL opcode tables. ELF gABI §§3 and 5.
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
    {.name = ".bss",    .type = ELF64_NOBITS,   .flags = ELF64_ALLOC | ELF64_WRITE,     .alignment = 1}
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
    int64_t addend;
    struct location at;
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
            source_error(as, at, "nonzero data in .bss");
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

static void emit_relative(struct assembler *as, struct token label, int64_t addend,
                          unsigned trailing)
{
    struct bytes *text = &as->contents[as->current];
    size_t index = symbol_index(as, label);
    if (as->reference_count == as->reference_capacity)
        as->references = grow(as->references, &as->reference_capacity, sizeof *as->references);
    size_t field = text->size;
    emit_word(text, 0, 4);
    as->references[as->reference_count++] = (struct reference){
        .symbol = index, .section = as->current, .field = field,
        .next_ip = text->size + trailing, .addend = addend, .at = locate(as, label.at)
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

static void resolve_references(struct assembler *as)
{
    for (size_t i = 0; i < as->reference_count; ++i) {
        const struct reference *ref = &as->references[i];
        const struct elf64_symbol *symbol = &as->symbols[ref->symbol].elf;
        if (symbol->section == 0)
            located_error(as, ref->at, "undefined label in relative operand");
        if (symbol->section != ref->section + 1)
            located_error(as, ref->at, "relative operand requires a label in the same section");

        bool negative = symbol->value < ref->next_ip;
        uint64_t distance = negative ? ref->next_ip - symbol->value : symbol->value - ref->next_ip;
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
    if (address.label.length != 0 && !address.ip_relative)
        source_error(p->as, address.label.at, "a symbolic address requires RIP or EIP");
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

static bool needs_rex_byte(struct reg reg)
{
    return reg.width == 1 && reg.number >= 4 && !reg.high_byte;
}

static void prefixes(struct assembler *as, const char *at, struct reg reg,
                     const struct operand *rm, unsigned char rex)
{
    struct bytes *text = &as->contents[as->current];
    if (rm->memory.segment)
        append(text, rm->memory.segment);
    if (reg.width == 2)
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

static void modrm(struct assembler *as, unsigned char opcode, struct reg reg,
                  const struct operand *rm, unsigned trailing)
{
    struct bytes *text = &as->contents[as->current];
    unsigned char rex = reg.number >= 8 ? 4 : 0;
    unsigned reg_field = (reg.number & 7) << 3;

    if (rm->reg.width != 0) {
        if (rm->reg.number >= 8)
            rex |= 1;
        prefixes(as, rm->at.at, reg, rm, rex);
        append(text, opcode);
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
    } else if (addr->displacement != 0 || base == 5) {
        mode = addr->displacement >= INT8_MIN && addr->displacement <= INT8_MAX ? 1 : 2;
        disp_width = mode == 1 ? 1 : 4;
    }
    bool sib = !addr->ip_relative && (addr->index >= 0 || addr->base < 0 || base == 4);
    if (addr->base >= 8)
        rex |= 1;
    if (addr->index >= 8)
        rex |= 2;
    prefixes(as, rm->at.at, reg, rm, rex);
    append(text, opcode);
    append(text, (unsigned char)((mode << 6) | reg_field | (sib ? 4 : base)));
    if (sib)
        append(text, (unsigned char)((addr->scale << 6) | (index << 3) | base));
    if (addr->label.length != 0)
        emit_relative(as, addr->label, addr->displacement, trailing);
    else
        emit_word(text, (uint64_t)addr->displacement, disp_width);
}

static void encode_mov_immediate(struct parser *p, const struct operand *dest)
{
    struct assembler *as = p->as;
    struct bytes *text = &as->contents[as->current];
    unsigned width = dest->reg.width != 0 ? dest->reg.width : dest->memory.width;
    if (width == 0 || width == 8)
        source_error(as, dest->at.at, "immediate move requires a byte, word or dword destination");
    uint64_t value = integer(p, width, false);
    if (dest->reg.width != 0) {
        unsigned char rex = dest->reg.number >= 8 ? 1 : 0;
        prefixes(as, dest->at.at, dest->reg, dest, rex);
        unsigned base = width == 1 ? 0xb0 : 0xb8;
        append(text, (unsigned char)(base | (dest->reg.number & 7)));
    } else {
        unsigned char opcode = width == 1 ? 0xc6 : 0xc7;
        modrm(as, opcode, (struct reg){.width = width}, dest, width);
    }
    emit_word(text, value, width);
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
    unsigned rm_width = rm->reg.width != 0 ? rm->reg.width : rm->memory.width;
    if (reg.width == 0)
        source_error(p->as, src.at.at, "mov requires a register operand");
    if (rm_width != 0 && rm_width != reg.width)
        source_error(p->as, rm->at.at, "mov operand widths differ");
    unsigned opcode = load ? 0x8a : 0x88;
    modrm(p->as, (unsigned char)(opcode + (reg.width != 1)), reg, rm, 0);
}

static bool encode_branch(struct parser *p, struct token op)
{
    static const struct {
        const char *name;
        unsigned char condition;
    } conditions[] = {
        {"jo", 0}, {"jno", 1}, {"jb", 2}, {"jc", 2}, {"jnae", 2},
        {"jae", 3}, {"jnb", 3}, {"jnc", 3}, {"je", 4}, {"jz", 4},
        {"jne", 5}, {"jnz", 5}, {"jbe", 6}, {"jna", 6}, {"ja", 7},
        {"jnbe", 7}, {"js", 8}, {"jns", 9}, {"jp", 10}, {"jpe", 10},
        {"jnp", 11}, {"jpo", 11}, {"jl", 12}, {"jnge", 12},
        {"jge", 13}, {"jnl", 13}, {"jle", 14}, {"jng", 14}, {"jg", 15}, {"jnle", 15}
    };
    struct assembler *as = p->as;
    struct bytes *text = &as->contents[as->current];
    if (is(op, "call") || is(op, "jmp")) {
        append(text, is(op, "call") ? 0xe8 : 0xe9);
    } else {
        size_t i = 0;
        while (i < sizeof conditions / sizeof conditions[0] && !is(op, conditions[i].name))
            ++i;
        if (i == sizeof conditions / sizeof conditions[0])
            return false;
        append(text, 0x0f);
        append(text, (unsigned char)(0x80 | conditions[i].condition));
    }
    if (starts_number(peek(p))) {
        emit_word(text, integer(p, 4, true), 4);
    } else {
        struct token label = word(p);
        int64_t addend = 0;
        char sign = peek(p);
        if (sign == '+' || sign == '-') {
            const char *at = p->at++;
            uint64_t offset = magnitude(p);
            if (offset > (uint64_t)INT64_MAX + (sign == '-'))
                source_error(as, at, "branch addend out of range");
            addend = signed_value(offset, sign == '-');
        }
        emit_relative(as, label, addend, 0);
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

static void emit_data_list(struct parser *p, unsigned width)
{
    do {
        if (peek(p) == '"' && width == 1) {
            emit_string(p, false);
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
    } else if (is(op, "global") || is(op, "globl")) {
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

static void instruction(struct parser *p, struct token op)
{
    struct assembler *as = p->as;
    struct bytes *text = &as->contents[as->current];
    if (as->current != 0)
        source_error(as, op.at, "instructions require .text");
    if (is(op, "mov")) {
        encode_mov(p);
    } else if (is(op, "ret")) {
        append(text, 0xc3);
    } else if (is(op, "syscall")) {
        append(text, 0x0f);
        append(text, 0x05);
    } else if (!encode_branch(p, op)) {
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
            size_t section_size = as->contents[symbol->elf.section - 1].size;
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
    for (size_t i = 0; i < as.section_count; ++i)
        free(as.contents[i].data);
    free(as.line.data);
    return EXIT_SUCCESS;
}
