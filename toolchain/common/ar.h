/* Common archive format: Oracle Solaris ar(3HEAD), spec-ledger.md. */
#ifndef BREENIX_TOOLCHAIN_AR_H
#define BREENIX_TOOLCHAIN_AR_H

#include <stdbool.h>
#include <stddef.h>

struct ar_member {
    size_t header_offset;
    const unsigned char *bytes;
    size_t size;
    const char *name;
    size_t name_length;
};

struct ar_symbol {
    const char *name;
    size_t member;
};

struct ar_archive {
    struct ar_member *members;
    size_t member_count;
    struct ar_symbol *symbols;
    size_t symbol_count;
    bool indexed;
};

bool ar_has_magic(const unsigned char *bytes, size_t size);

/* On failure return a diagnostic. The caller disposes the archive either
 * way. The supplied bytes outlive the member and symbol views. */
const char *ar_read(struct ar_archive *archive, const unsigned char *bytes, size_t size);
/* For index reconstruction: validate member framing and names without
 * interpreting the old symbol index payload. */
const char *ar_read_members(struct ar_archive *archive, const unsigned char *bytes, size_t size);
void ar_dispose(struct ar_archive *archive);

#endif
