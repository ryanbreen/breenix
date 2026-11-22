/* Oracle Solaris ar(3HEAD): common archive headers, long-name table and
 * 32-/64-bit symbol indexes (spec-ledger.md).
 */
#include "ar.h"

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

enum { NAME_SIZE = 16, SIZE_OFFSET = 48, SIZE_WIDTH = 10, HEADER_SIZE = 60 };

static const char magic[] = "!<arch>\n";

bool ar_has_magic(const unsigned char *bytes, size_t size)
{
    return size >= sizeof magic - 1 && memcmp(bytes, magic, sizeof magic - 1) == 0;
}

static bool decimal(const unsigned char *field, size_t width, size_t *value)
{
    size_t result = 0;
    size_t i = 0;
    while (i < width && field[i] >= '0' && field[i] <= '9') {
        unsigned digit = field[i++] - '0';
        if (result > (SIZE_MAX - digit) / 10)
            return false;
        result = result * 10 + digit;
    }
    if (i == 0)
        return false;
    for (; i < width; ++i) {
        if (field[i] != ' ')
            return false;
    }
    *value = result;
    return true;
}

static uint64_t big_endian(const unsigned char *bytes, size_t width)
{
    uint64_t value = 0;
    for (size_t i = 0; i < width; ++i)
        value = (value << 8) | bytes[i];
    return value;
}

static size_t member_index(const struct ar_archive *archive, uint64_t offset)
{
    size_t first = 0, end = archive->member_count;
    while (first < end) {
        size_t middle = first + (end - first) / 2;
        size_t header = archive->members[middle].header_offset;
        if (header == offset)
            return middle;
        if (header < offset)
            first = middle + 1;
        else
            end = middle;
    }
    return archive->member_count;
}

static const char *read_index(struct ar_archive *archive, const unsigned char *bytes,
                              size_t size, size_t width)
{
    if (size < width)
        return "truncated archive symbol index";
    uint64_t count = big_endian(bytes, width);
    if (count > (size - width) / width || count > SIZE_MAX / sizeof *archive->symbols)
        return "archive symbol index count exceeds bounds";
    const unsigned char *name = bytes + width + (size_t)count * width;
    const unsigned char *end = bytes + size;
    if (count > (size_t)(end - name) / 2)
        return "archive symbol index names exceed bounds";
    if (count != 0) {
        archive->symbols = malloc((size_t)count * sizeof *archive->symbols);
        if (archive->symbols == NULL)
            return "cannot allocate archive symbol index";
    }
    for (size_t i = 0; i < count; ++i) {
        uint64_t offset = big_endian(bytes + width + i * width, width);
        size_t member = member_index(archive, offset);
        if (member == archive->member_count)
            return "archive symbol index offset does not name a member header";
        const unsigned char *nul = memchr(name, '\0', (size_t)(end - name));
        if (nul == NULL || nul == name)
            return "invalid name in archive symbol index";
        archive->symbols[i] = (struct ar_symbol){(const char *)name, member};
        name = nul + 1;
    }
    /* Some producers count the optional alignment NUL in the index size. */
    if (name != end && !(end - name == 1 && *name == '\0'))
        return "unexpected bytes after archive symbol index names";
    archive->symbol_count = (size_t)count;
    archive->indexed = true;
    return NULL;
}

static bool read_name(struct ar_member *member, const unsigned char *header, size_t width,
                      const unsigned char *names, size_t names_size)
{
    if (width == 0)
        return false;
    if (header[0] == '/') {
        size_t offset;
        if (!decimal(header + 1, width - 1, &offset) || offset >= names_size ||
            (offset != 0 && (offset < 2 || names[offset - 2] != '/' || names[offset - 1] != '\n')))
            return false;
        member->name = (const char *)names + offset;
        for (size_t i = offset; i + 1 < names_size && names[i] != '\0'; ++i) {
            if (names[i] == '/' && names[i + 1] == '\n') {
                member->name_length = i - offset;
                return i != offset;
            }
        }
        return false;
    }
    if (header[width - 1] != '/' || memchr(header, '\0', width) != NULL)
        return false;
    member->name = (const char *)header;
    member->name_length = width - 1;
    return true;
}

const char *ar_read(struct ar_archive *archive, const unsigned char *bytes, size_t size)
{
    if (!ar_has_magic(bytes, size))
        return "expected a common archive";
    const unsigned char *index = NULL, *names = NULL;
    size_t index_size = 0, index_width = 0, names_size = 0, capacity = 0;
    for (size_t pos = sizeof magic - 1; pos < size;) {
        const unsigned char *header = bytes + pos;
        size_t length;
        if (size - pos < HEADER_SIZE || memcmp(header + HEADER_SIZE - 2, "`\n", 2) != 0 ||
            !decimal(header + SIZE_OFFSET, SIZE_WIDTH, &length) ||
            length > size - pos - HEADER_SIZE)
            return "invalid or truncated archive member header";
        const unsigned char *payload = header + HEADER_SIZE;
        size_t next = pos + HEADER_SIZE + length;
        if (length % 2 != 0) {
            if (next == size || bytes[next] != '\n')
                return "missing archive member padding";
            ++next;
        }
        size_t width = NAME_SIZE;
        while (width != 0 && header[width - 1] == ' ')
            --width;

        if ((width == 1 && header[0] == '/') ||
            (width == 7 && memcmp(header, "/SYM64/", 7) == 0)) {
            if (pos != sizeof magic - 1)
                return "archive symbol index must be the first member";
            index = payload;
            index_size = length;
            index_width = width == 1 ? 4 : 8;
        } else if (width == 2 && memcmp(header, "//", 2) == 0) {
            if (names != NULL || archive->member_count != 0)
                return "misplaced or repeated archive long-name table";
            names = payload;
            names_size = length;
        } else {
            struct ar_member member = {.header_offset = pos, .bytes = payload, .size = length};
            if (!read_name(&member, header, width, names, names_size))
                return "invalid archive member name or long-name offset";
            if (archive->member_count == capacity) {
                if (capacity > SIZE_MAX / sizeof member / 2)
                    return "archive member count exceeds bounds";
                capacity = capacity == 0 ? 16 : capacity * 2;
                struct ar_member *members = realloc(archive->members, capacity * sizeof member);
                if (members == NULL)
                    return "cannot allocate archive member table";
                archive->members = members;
            }
            archive->members[archive->member_count++] = member;
        }
        pos = next;
    }
    return index == NULL ? NULL : read_index(archive, index, index_size, index_width);
}

void ar_dispose(struct ar_archive *archive)
{
    free(archive->symbols);
    free(archive->members);
}
