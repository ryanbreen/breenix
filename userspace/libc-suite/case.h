/*
 * The C side of the libc suite (docs/suites/libc.json).
 *
 * Each category is one program: harness.c plus <category>.c, compiled against Breenix's
 * own C library (libs/libbreenix-libc) and installed as
 * /usr/local/test/bin/libc-<category>_test, with -std=c17 -fno-builtin -frounding-math so
 * every call reaches the library instead of being evaluated by the compiler, and with the
 * floating-point environment honoured. /sbin/suite-libc runs it once per case as
 *
 *     libc-<category>_test <case-id> <result-fd>
 *
 * in a fresh scratch directory, with stdin, stdout and stderr on /dev/null and the
 * environment PATH=/usr/local/libc/bin, HOME=/tmp, LIBC_SUITE=1. The harness writes one
 * line to <result-fd>, "PASS", "FAIL <message>" or "SKIP <message>", and exits 0.
 *
 * A case returns PASS (a null pointer) or the text from one of the fail helpers below.
 * The harness and the helpers use nothing from the library under test but write() and
 * _exit(): messages are built here, not with printf or the string functions, so a broken
 * stdio or string.h cannot garble the report of what broke.
 *
 * Every case tests one rule of ISO C17 or POSIX.1-2024, with Linux's ABI where POSIX
 * leaves a choice open; the manifest's title for the case names the rule.
 */
#ifndef LIBC_SUITE_CASE_H
#define LIBC_SUITE_CASE_H

#include <stddef.h>

typedef const char *(*libc_case_fn)(void);

struct libc_case {
    const char *id;
    libc_case_fn run;
};

/* Each <category>.c defines this table, ended by { 0, 0 }, in the manifest's order. */
extern const struct libc_case libc_cases[];

/* The environment main received, for cases that compare it with environ. */
extern char **libc_case_envp;

#define PASS ((const char *)0)

/* Fail with fixed text. */
const char *fail_text(const char *what);
/* "<what>: got <got>, expected <want>". */
const char *fail_int(const char *what, long long got, long long want);
const char *fail_uint(const char *what, unsigned long long got, unsigned long long want);
/* "<what>: errno <name>, expected <name>". */
const char *fail_errno(const char *what, int got, int want);
/* "<what> returned <ret> (errno <name>), expected <want_ret> with errno <name>". */
const char *fail_ret_errno(const char *what, long long ret, int err, long long want_ret, int want_err);
/* Strings are shown quoted with bytes outside printable ASCII as \xNN; NULL as (null). */
const char *fail_str(const char *what, const char *got, const char *want);
const char *fail_mem(const char *what, const void *got, const void *want, size_t n);
const char *fail_dbl(const char *what, double got, double want);
const char *fail_ptr(const char *what, const void *got, const void *want);
/* Mark the case skipped, saying why. */
const char *skip_text(const char *why);

/* Harness copies of what the cases would otherwise take from the library under test. */
int h_streq(const char *a, const char *b);
int h_memeq(const void *a, const void *b, size_t n);
size_t h_strlen(const char *s);
/* Same value, or both NaN; a zero must also have the expected sign. */
int h_dbl_same(double got, double want);
/* Create or replace `name` holding `text`, with open/write; 0 or -1. */
int h_put_file(const char *name, const char *text);
/* Read up to cap-1 bytes of `name` into buf, NUL-terminated; the length or -1. */
long h_read_file(const char *name, char *buf, size_t cap);
/* Read what a non-blocking descriptor holds now, NUL-terminated; 0 when it holds nothing. */
long h_read_now(int fd, char *buf, size_t cap);

#define CHECK(cond, what) do { if (!(cond)) return fail_text(what); } while (0)
#define EXPECT_INT(what, got, want) do { long long g_ = (long long)(got), w_ = (long long)(want); \
    if (g_ != w_) return fail_int((what), g_, w_); } while (0)
#define EXPECT_UINT(what, got, want) do { unsigned long long g_ = (unsigned long long)(got), \
    w_ = (unsigned long long)(want); if (g_ != w_) return fail_uint((what), g_, w_); } while (0)
#define EXPECT_ERRNO(what, want) do { int e_ = errno; \
    if (e_ != (want)) return fail_errno((what), e_, (want)); } while (0)
#define EXPECT_RET_ERRNO(what, ret, want_ret, want_err) do { long long r_ = (long long)(ret); int e_ = errno; \
    if (r_ != (long long)(want_ret) || e_ != (want_err)) \
        return fail_ret_errno((what), r_, e_, (long long)(want_ret), (want_err)); } while (0)
#define EXPECT_STR(what, got, want) do { const char *g_ = (got), *w_ = (want); \
    if (!h_streq(g_, w_)) return fail_str((what), g_, w_); } while (0)
#define EXPECT_MEM(what, got, want, n) do { if (!h_memeq((got), (want), (n))) \
    return fail_mem((what), (got), (want), (n)); } while (0)
#define EXPECT_DBL(what, got, want) do { double g_ = (got), w_ = (want); \
    if (!h_dbl_same(g_, w_)) return fail_dbl((what), g_, w_); } while (0)
#define EXPECT_NEAR(what, got, want, tol) do { double g_ = (got), w_ = (want); \
    if (!(g_ - w_ <= (tol) && w_ - g_ <= (tol))) return fail_dbl((what), g_, w_); } while (0)
#define EXPECT_PTR(what, got, want) do { const void *g_ = (const void *)(got), *w_ = (const void *)(want); \
    if (g_ != w_) return fail_ptr((what), g_, w_); } while (0)

#endif
