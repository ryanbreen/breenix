/* Solaris ar(3HEAD): common archive headers, long names and symbol indexes.
 * ELF gABI §5: index non-local definitions, including weak and common symbols. */
#define _POSIX_C_SOURCE 200809L
#include "../common/ar.h"
#include "../common/elf_symbols.h"

#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

enum { HEADER = 60, MAGIC = 8, SHORT_NAME = 15 };

struct member {
    char *name;
    const unsigned char *data;
    unsigned char *owned;
    size_t size, name_offset, header_offset;
};

struct index_entry {
    const char *name;
    size_t member;
};

struct archive_index {
    struct index_entry *entries;
    size_t count, capacity, strings_size;
};

static const char *collect_symbols(struct archive_index *index,
                                   const struct member *member, size_t number)
{
    if (member->size < 4 || memcmp(member->data, "\177ELF", 4) != 0)
        return NULL;
    struct elf_symbols view;
    const char *error = elf_symbols_read(&view, member->data, member->size);
    if (error != NULL)
        return error;
    if (member->data[16] != 1 || member->data[17] != 0)
        return "expected an ELF relocatable archive member";
    for (size_t i = 1; i < view.count; ++i) {
        struct elf_symbol sym;
        error = elf_symbol_at(&view, i, &sym);
        if (error != NULL)
            return error;
        if ((sym.binding != SYMBOL_GLOBAL && sym.binding != SYMBOL_WEAK) ||
            sym.section == SYMBOL_UNDEFINED || sym.name[0] == '\0')
            continue;
        if (index->count == index->capacity) {
            if (index->capacity > SIZE_MAX / sizeof *index->entries / 2)
                return "archive symbol count exceeds bounds";
            size_t capacity = index->capacity == 0 ? 64 : index->capacity * 2;
            void *entries = realloc(index->entries, capacity * sizeof *index->entries);
            if (entries == NULL)
                return "cannot allocate archive index";
            index->entries = entries;
            index->capacity = capacity;
        }
        size_t length = strlen(sym.name) + 1;
        if (length > SIZE_MAX - index->strings_size)
            return "archive symbol names exceed bounds";
        index->strings_size += length;
        index->entries[index->count++] = (struct index_entry){sym.name, number};
    }
    return NULL;
}

static bool header(FILE *out, const char *name, size_t size)
{
    char bytes[HEADER + 1];
    int length = snprintf(bytes, sizeof bytes, "%-16s%-12d%-6d%-6d%-8o%-10zu`\n",
                          name, 0, 0, 0, 0644u, size);
    return length == HEADER && fwrite(bytes, 1, HEADER, out) == HEADER;
}

static bool word(FILE *out, size_t value)
{
    unsigned char bytes[] = {
        (unsigned char)(value >> 24), (unsigned char)(value >> 16),
        (unsigned char)(value >> 8), (unsigned char)value
    };
    return fwrite(bytes, 1, sizeof bytes, out) == sizeof bytes;
}

static bool padding(FILE *out, size_t size)
{
    return size % 2 == 0 || fputc('\n', out) != EOF;
}

static bool measure_member(size_t *offset, size_t size)
{
    /* This writer emits the documented 32-bit index. */
    if (size > UINT32_MAX - HEADER - 1 ||
        *offset > UINT32_MAX - HEADER - size - size % 2)
        return false;
    *offset += HEADER + size + size % 2;
    return true;
}

static bool write_contents(FILE *out, const struct member *members, size_t count,
                           const struct archive_index *index, size_t index_size,
                           size_t names_size)
{
    if (fwrite("!<arch>\n", 1, MAGIC, out) != MAGIC ||
        !header(out, "/", index_size) || !word(out, index->count))
        return false;
    for (size_t i = 0; i < index->count; ++i)
        if (!word(out, members[index->entries[i].member].header_offset))
            return false;
    for (size_t i = 0; i < index->count; ++i) {
        const char *name = index->entries[i].name;
        size_t length = strlen(name) + 1;
        if (fwrite(name, 1, length, out) != length)
            return false;
    }
    if (!padding(out, index_size))
        return false;
    if (names_size != 0) {
        if (!header(out, "//", names_size))
            return false;
        for (size_t i = 0; i < count; ++i)
            if (members[i].name_offset != SIZE_MAX &&
                fprintf(out, "%s/\n", members[i].name) < 0)
                return false;
        if (!padding(out, names_size))
            return false;
    }
    for (size_t i = 0; i < count; ++i) {
        const struct member *member = &members[i];
        char name[SHORT_NAME + 2];
        if (member->name_offset == SIZE_MAX)
            snprintf(name, sizeof name, "%s/", member->name);
        else
            snprintf(name, sizeof name, "/%zu", member->name_offset);
        if (!header(out, name, member->size) ||
            fwrite(member->data, 1, member->size, out) != member->size ||
            !padding(out, member->size))
            return false;
    }
    return true;
}

static bool publish(const char *path, const struct member *members, size_t count,
                    const struct archive_index *index, size_t index_size, size_t names_size,
                    mode_t mode)
{
    const char suffix[] = ".bar.XXXXXX";
    size_t length = strlen(path);
    if (length > SIZE_MAX - sizeof suffix)
        return false;
    char *temp = malloc(length + sizeof suffix);
    if (temp == NULL)
        return false;
    memcpy(temp, path, length);
    memcpy(temp + length, suffix, sizeof suffix);
    int fd = mkstemp(temp);
    if (fd < 0) {
        free(temp);
        return false;
    }
    FILE *out = fdopen(fd, "wb");
    bool ok = false;
    if (out == NULL) {
        close(fd);
    } else {
        ok = write_contents(out, members, count, index, index_size, names_size) &&
             fchmod(fd, mode) == 0;
        if (fclose(out) != 0)
            ok = false;
    }
    ok = ok && rename(temp, path) == 0;
    if (!ok)
        unlink(temp);
    free(temp);
    return ok;
}

static const char *write_archive(const char *path, struct member *members, size_t count,
                                 mode_t mode)
{
    struct archive_index index = {0};
    size_t names_size = 0, index_size = 0, offset = MAGIC;
    const char *error = "archive exceeds the 32-bit index limit";
    for (size_t i = 0; i < count; ++i) {
        struct member *member = &members[i];
        size_t length = strlen(member->name);
        member->name_offset = SIZE_MAX;
        if (length > SHORT_NAME || strchr(member->name, ' ') != NULL) {
            if (length > UINT32_MAX - 2 || names_size > UINT32_MAX - length - 2)
                goto done;
            member->name_offset = names_size;
            names_size += length + 2;
        }
        const char *bad_symbols = collect_symbols(&index, member, i);
        if (bad_symbols != NULL) {
            fprintf(stderr, "bar: %s(%s): %s\n", path, member->name, bad_symbols);
            error = "could not index archive members";
            goto done;
        }
    }
    if (index.count >= UINT32_MAX / 4 ||
        index.strings_size > UINT32_MAX - 4 * (index.count + 1))
        goto done;
    index_size = 4 * (index.count + 1) + index.strings_size;
    if (!measure_member(&offset, index_size) ||
        (names_size != 0 && !measure_member(&offset, names_size)))
        goto done;
    for (size_t i = 0; i < count; ++i) {
        members[i].header_offset = offset;
        if (!measure_member(&offset, members[i].size))
            goto done;
    }
    error = publish(path, members, count, &index, index_size, names_size, mode)
        ? NULL : "cannot publish archive";
done:
    free(index.entries);
    return error;
}

static const char *replace_member(struct member *members, size_t *count, const char *path)
{
    const char *name = strrchr(path, '/');
    name = name == NULL ? path : name + 1;
    if (*name == '\0' || strchr(name, '\n') != NULL || strchr(name, '\r') != NULL)
        return "invalid member name";
    size_t size;
    unsigned char *data = tool_read_file(path, &size);
    if (data == NULL)
        return "cannot read member";
    size_t i = 0;
    while (i < *count && strcmp(members[i].name, name) != 0)
        ++i;
    if (i == *count) {
        members[i].name = strdup(name);
        if (members[i].name == NULL) {
            free(data);
            return "cannot allocate member name";
        }
        ++*count;
    }
    free(members[i].owned);
    members[i].owned = data;
    members[i].data = data;
    members[i].size = size;
    return NULL;
}

static int update(const char *path, bool replace, char **inputs, size_t input_count)
{
    struct ar_archive original = {0};
    struct stat st;
    struct member *members = NULL;
    unsigned char *bytes = NULL;
    size_t count = 0;
    const char *error = NULL;
    mode_t mode;
    if (stat(path, &st) == 0) {
        size_t size;
        mode = st.st_mode & 0777;
        bytes = tool_read_file(path, &size);
        error = bytes == NULL ? "cannot read archive" : ar_read(&original, bytes, size);
        if (error != NULL)
            goto done;
    } else if (errno == ENOENT && replace) {
        mode_t mask = umask(0);
        umask(mask);
        mode = 0666 & ~mask;
    } else {
        error = "cannot open archive";
        goto done;
    }

    if (original.member_count > SIZE_MAX / sizeof *members - input_count) {
        error = "too many archive members";
        goto done;
    }
    size_t capacity = original.member_count + input_count;
    members = calloc(capacity == 0 ? 1 : capacity, sizeof *members);
    if (members == NULL) {
        error = "cannot allocate member table";
        goto done;
    }
    for (; count < original.member_count; ++count) {
        const struct ar_member *member = &original.members[count];
        char *name = malloc(member->name_length + 1);
        if (name == NULL) {
            error = "cannot allocate member name";
            goto done;
        }
        memcpy(name, member->name, member->name_length);
        name[member->name_length] = '\0';
        members[count] = (struct member){.name = name, .data = member->bytes, .size = member->size};
    }
    for (size_t i = 0; i < input_count; ++i) {
        error = replace_member(members, &count, inputs[i]);
        if (error != NULL) {
            path = inputs[i];
            goto done;
        }
    }
    error = write_archive(path, members, count, mode);
done:
    for (size_t i = 0; i < count; ++i) {
        free(members[i].name);
        free(members[i].owned);
    }
    free(members);
    ar_dispose(&original);
    free(bytes);
    if (error != NULL)
        fprintf(stderr, "bar: %s: %s\n", path, error);
    return error != NULL;
}

int main(int argc, char **argv)
{
    if (argc >= 3) {
        const char *op = argv[1] + (argv[1][0] == '-');
        if (strcmp(op, "s") == 0 && argc == 3)
            return update(argv[2], false, NULL, 0);
        if ((strcmp(op, "r") == 0 || strcmp(op, "rc") == 0 || strcmp(op, "rs") == 0 ||
             strcmp(op, "rcs") == 0))
            return update(argv[2], true, argv + 3, (size_t)(argc - 3));
    }
    fprintf(stderr, "usage: bar r[c][s] archive [file ...]\n       bar s archive\n");
    return 2;
}
