/* Intel SDM Vol. 2 MOV, CALL, RET and SYSCALL opcode tables, Table 2-4;
 * AMD APM Vol. 1 Figure 3-3. ELF gABI §3.10 for named sections.
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

struct bytes {
    unsigned char *data;
    size_t size;
    size_t capacity;
};

struct assembler {
    const char *path;
    size_t line_number;
    struct bytes line;
    struct bytes text;
    struct elf64_section sections[sizeof supported_sections / sizeof supported_sections[0]];
    size_t section_count;
    size_t current;
    struct elf64_global *globals;
    size_t global_count;
    size_t global_capacity;
};

static _Noreturn void io_error(const char *path)
{
    perror(path);
    exit(EXIT_FAILURE);
}

static _Noreturn void source_error(const struct assembler *as, const char *at,
                                   const char *message)
{
    size_t column = (size_t)(at - (const char *)as->line.data) + 1;
    fprintf(stderr, "%s:%zu:%zu: %s\n", as->path, as->line_number, column, message);
    exit(EXIT_FAILURE);
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

static char *skip_space(char *word)
{
    while (isspace((unsigned char)*word))
        ++word;
    return word;
}

static void trim_end(char *word)
{
    char *end = word + strlen(word);
    while (end > word && isspace((unsigned char)end[-1]))
        *--end = '\0';
}

static bool is_name(const char *word)
{
    if (!isalpha((unsigned char)*word) && *word != '_')
        return false;
    for (++word; *word; ++word)
        if (!isalnum((unsigned char)*word) && *word != '_')
            return false;
    return true;
}

static void select_section(struct assembler *as, const char *name)
{
    for (size_t i = 0; i < as->section_count; ++i) {
        if (strcmp(name, as->sections[i].name) == 0) {
            as->current = i;
            return;
        }
    }
    for (size_t i = 0; i < sizeof supported_sections / sizeof supported_sections[0]; ++i) {
        if (strcmp(name, supported_sections[i].name) == 0) {
            as->current = as->section_count++;
            as->sections[as->current] = supported_sections[i];
            return;
        }
    }
    source_error(as, name, "unsupported section");
}

static struct elf64_global *find_global(struct assembler *as, const char *name)
{
    for (size_t i = 0; i < as->global_count; ++i)
        if (strcmp(name, as->globals[i].name) == 0)
            return &as->globals[i];
    return NULL;
}

static void declare_global(struct assembler *as, const char *name)
{
    if (!is_name(name))
        source_error(as, name, "expected one symbol name after global");
    if (find_global(as, name) != NULL)
        return;
    if (as->global_count == as->global_capacity)
        as->globals = grow(as->globals, &as->global_capacity, sizeof *as->globals);

    size_t length = strlen(name) + 1;
    char *copy = malloc(length);
    if (copy == NULL)
        io_error("bas");
    memcpy(copy, name, length);
    as->globals[as->global_count++] = (struct elf64_global){.name = copy};
}

static void define_label(struct assembler *as, const char *name)
{
    if (!is_name(name))
        source_error(as, name, "invalid label");
    struct elf64_global *symbol = find_global(as, name);
    if (symbol == NULL)
        source_error(as, name, "declare the label with global before defining it");
    if (symbol->section != 0)
        source_error(as, name, "duplicate label");
    symbol->section = (uint16_t)(as->current + 1);
    symbol->value = as->current == 0 ? as->text.size : 0;
}

static uint32_t immediate(const struct assembler *as, const char *word, long long max)
{
    const char *digits = word;
    if (*digits == '-' || *digits == '+')
        ++digits;
    int base = digits[0] == '0' && (digits[1] == 'x' || digits[1] == 'X') ? 16 : 10;
    char *end;
    errno = 0;
    long long value = strtoll(word, &end, base);
    if (end == word || *end != '\0' || errno == ERANGE || value < INT32_MIN || value > max)
        source_error(as, word, "expected a 32-bit integer in range");
    return (uint32_t)value;
}

static void emit_word(struct bytes *text, uint32_t value)
{
    for (size_t i = 0; i < 4; ++i) {
        append(text, (unsigned char)value);
        value >>= 8;
    }
}

static void encode_mov(struct assembler *as, char *operands)
{
    /* AMD APM Vol. 1 Figure 3-3 lists these register numbers. */
    static const char *const registers[] = {
        "eax", "ecx", "edx", "ebx", "esp", "ebp", "esi", "edi",
        "r8d", "r9d", "r10d", "r11d", "r12d", "r13d", "r14d", "r15d"
    };

    char *comma = strchr(operands, ',');
    if (comma == NULL)
        source_error(as, operands, "expected mov r32, imm32");
    *comma = '\0';
    trim_end(operands);

    size_t reg = 0;
    while (reg < sizeof registers / sizeof registers[0] && strcmp(operands, registers[reg]) != 0)
        ++reg;
    if (reg == sizeof registers / sizeof registers[0])
        source_error(as, operands, "expected a 32-bit register");

    uint32_t value = immediate(as, skip_space(comma + 1), UINT32_MAX);
    if (reg >= 8)
        append(&as->text, 0x41); /* REX.B extends the opcode register. */
    append(&as->text, (unsigned char)(0xb8 + (reg & 7)));
    emit_word(&as->text, value);
}

static void assemble_line(struct assembler *as)
{
    char *word = skip_space((char *)as->line.data);
    char *comment = strchr(word, ';');
    if (comment != NULL)
        *comment = '\0';
    trim_end(word);

    char *label_end = strchr(word, ':');
    if (label_end != NULL) {
        *label_end = '\0';
        trim_end(word);
        define_label(as, word);
        word = skip_space(label_end + 1);
    }
    if (*word == '\0')
        return;

    char *operands = word;
    while (*operands != '\0' && !isspace((unsigned char)*operands))
        ++operands;
    if (*operands != '\0')
        *operands++ = '\0';
    operands = skip_space(operands);

    if (strcmp(word, "section") == 0) {
        select_section(as, operands);
    } else if (strcmp(word, "global") == 0) {
        declare_global(as, operands);
    } else {
        if (as->current != 0)
            source_error(as, word, "instructions require .text");
        if (strcmp(word, "mov") == 0) {
            encode_mov(as, operands);
        } else if (strcmp(word, "call") == 0) {
            uint32_t displacement = immediate(as, operands, INT32_MAX);
            append(&as->text, 0xe8);
            emit_word(&as->text, displacement);
        } else if (strcmp(word, "ret") == 0 || strcmp(word, "syscall") == 0) {
            if (*operands != '\0')
                source_error(as, operands, "unexpected operands");
            if (strcmp(word, "ret") == 0) {
                append(&as->text, 0xc3);
            } else {
                append(&as->text, 0x0f);
                append(&as->text, 0x05);
            }
        } else {
            source_error(as, word, "unsupported instruction or directive");
        }
    }
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

    as.sections[0].data = as.text.data;
    as.sections[0].size = as.text.size;
    FILE *object = fopen(output, "wb");
    if (object == NULL)
        io_error(output);
    if (!elf64_write_rel(object, as.sections, as.section_count, as.globals, as.global_count) ||
        fclose(object) == EOF)
        io_error(output);

    for (size_t i = 0; i < as.global_count; ++i)
        free((void *)as.globals[i].name);
    free(as.globals);
    free(as.line.data);
    free(as.text.data);
    return EXIT_SUCCESS;
}
