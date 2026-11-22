/* ELF gABI §§3, 5: describe static symbols in an ELF file or common archive. */
#include "../common/ar.h"
#include "../common/elf_symbols.h"

#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

struct options {
    bool external, undefined, prefix, portable;
};

static char symbol_class(const struct elf_symbol *sym)
{
    if (sym->binding == SYMBOL_WEAK) {
        char defined = sym->type == SYMBOL_OBJECT ? 'V' : 'W';
        return sym->section == SYMBOL_UNDEFINED ? (char)(defined + 'a' - 'A') : defined;
    }
    char kind = '?';
    if (sym->section == SYMBOL_UNDEFINED)
        kind = 'U';
    else if (sym->section == SYMBOL_ABSOLUTE)
        kind = 'A';
    else if (sym->section == SYMBOL_COMMON)
        kind = 'C';
    else if ((sym->flags & SYMBOL_ALLOC) != 0) {
        if ((sym->flags & SYMBOL_EXEC) != 0)
            kind = 'T';
        else if (sym->section_type == SYMBOL_NOBITS)
            kind = 'B';
        else
            kind = (sym->flags & SYMBOL_WRITE) != 0 ? 'D' : 'R';
    }
    if (sym->binding == SYMBOL_LOCAL && kind != '?')
        kind = (char)(kind + 'a' - 'A');
    return kind;
}

static int compare_symbols(const void *left, const void *right)
{
    const struct elf_symbol *a = left;
    const struct elf_symbol *b = right;
    int order = strcmp(a->name, b->name);
    if (order != 0)
        return order;
    if (a->section != b->section)
        return a->section < b->section ? -1 : 1;
    return (a->value > b->value) - (a->value < b->value);
}

static void print_name(FILE *out, const char *path, const struct ar_member *member)
{
    fputs(path, out);
    if (member != NULL)
        fprintf(out, "[%.*s]", (int)member->name_length, member->name);
}

static void print_symbol(const struct options *options, const struct elf_symbol *sym)
{
    char kind = symbol_class(sym);
    bool defined = sym->section != SYMBOL_UNDEFINED;
    if (options->portable) {
        printf("%s %c", sym->name, kind);
        if (defined)
            printf(" %llx %llx", (unsigned long long)sym->value, (unsigned long long)sym->size);
        putchar('\n');
    } else if (defined) {
        printf("%016llx %c %s\n", (unsigned long long)sym->value, kind, sym->name);
    } else {
        printf("                 %c %s\n", kind, sym->name);
    }
}

static const char *print_elf(const struct options *options, const char *path,
                             const struct ar_member *member, bool heading,
                             const unsigned char *data, size_t size)
{
    struct elf_symbols view;
    const char *error = elf_symbols_read(&view, data, size);
    if (error != NULL || view.count == 0)
        return error;
    if (view.count > SIZE_MAX / sizeof(struct elf_symbol))
        return "ELF symbol count exceeds bounds";
    struct elf_symbol *symbols = malloc(view.count * sizeof *symbols);
    if (symbols == NULL)
        return "cannot allocate symbols";
    size_t count = 0;
    for (size_t i = 1; i < view.count; ++i) {
        struct elf_symbol sym;
        error = elf_symbol_at(&view, i, &sym);
        if (error != NULL)
            goto done;
        if (sym.name[0] == '\0' || sym.type == SYMBOL_FILE || sym.type == SYMBOL_SECTION ||
            (options->external && sym.binding == SYMBOL_LOCAL) ||
            (options->undefined && sym.section != SYMBOL_UNDEFINED))
            continue;
        symbols[count++] = sym;
    }
    qsort(symbols, count, sizeof *symbols, compare_symbols);
    if (heading && !options->prefix) {
        print_name(stdout, path, member);
        puts(":");
    }
    for (size_t i = 0; i < count; ++i) {
        if (options->prefix) {
            print_name(stdout, path, member);
            fputs(": ", stdout);
        }
        print_symbol(options, &symbols[i]);
    }
done:
    free(symbols);
    return error;
}

static void diagnostic(const char *path, const struct ar_member *member, const char *error)
{
    fputs("bnm: ", stderr);
    print_name(stderr, path, member);
    fprintf(stderr, ": %s\n", error);
}

static bool inspect(const struct options *options, const char *path, bool heading)
{
    size_t size;
    unsigned char *data = tool_read_file(path, &size);
    if (data == NULL) {
        diagnostic(path, NULL, "cannot read input");
        return false;
    }
    struct ar_archive archive = {0};
    const char *error;
    bool ok = true;
    if (ar_has_magic(data, size)) {
        error = ar_read(&archive, data, size);
        if (error == NULL) {
            for (size_t i = 0; i < archive.member_count; ++i) {
                const struct ar_member *member = &archive.members[i];
                if (member->name_length > INT_MAX) {
                    diagnostic(path, NULL, "archive member name too long");
                    ok = false;
                    continue;
                }
                const char *bad_member = print_elf(options, path, member, true,
                                                   member->bytes, member->size);
                if (bad_member != NULL) {
                    diagnostic(path, member, bad_member);
                    ok = false;
                }
            }
        }
    } else {
        error = print_elf(options, path, NULL, heading, data, size);
    }
    if (error != NULL) {
        diagnostic(path, NULL, error);
        ok = false;
    }
    ar_dispose(&archive);
    free(data);
    return ok;
}

int main(int argc, char **argv)
{
    struct options options = {0};
    int i;
    for (i = 1; i < argc && argv[i][0] == '-' && argv[i][1] != '\0'; ++i) {
        if (strcmp(argv[i], "--") == 0) {
            ++i;
            break;
        }
        for (const char *flag = argv[i] + 1; *flag != '\0'; ++flag) {
            switch (*flag) {
            case 'A': options.prefix = true; break;
            case 'g': options.external = true; break;
            case 'P': options.portable = true; break;
            case 'u': options.undefined = true; break;
            default: goto usage;
            }
        }
    }
    if (i < argc) {
        bool ok = true;
        bool heading = argc - i > 1;
        for (; i < argc; ++i)
            if (!inspect(&options, argv[i], heading))
                ok = false;
        return ok && fflush(stdout) == 0 ? 0 : 1;
    }
usage:
    fputs("usage: bnm [-AgPu] file ...\n", stderr);
    return 2;
}
