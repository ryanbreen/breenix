/* libc suite, string-stdlib category: <string.h>, <strings.h> and <stdlib.h>. */
#define _POSIX_C_SOURCE 202405L
#include "case.h"

#include <errno.h>
#include <limits.h>
#include <math.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

static const char *memcpy_memmove(void) {
    char a[16] = "abcdefgh";
    char b[16] = { 0 };
    EXPECT_PTR("memcpy return", memcpy(b, a, 9), b);
    EXPECT_STR("memcpy result", b, "abcdefgh");
    EXPECT_PTR("memmove return", memmove(a + 2, a, 6), a + 2);
    EXPECT_STR("memmove to an overlapping later address", a, "ababcdef");
    char c[16] = "abcdefgh";
    memmove(c, c + 3, 6);
    EXPECT_STR("memmove to an overlapping earlier address", c, "defgh");
    return PASS;
}

static const char *memset_memchr(void) {
    unsigned char a[8] = { 1, 2, 3, 4, 5, 6, 7, 8 };
    EXPECT_PTR("memset return", memset(a + 1, 0x1ff, 3), a + 1);
    const unsigned char want[8] = { 1, 0xff, 0xff, 0xff, 5, 6, 7, 8 };
    EXPECT_MEM("memset converts its value to unsigned char", a, want, 8);
    const char s[] = "ab\0cd";
    EXPECT_PTR("memchr past a NUL", memchr(s, 'd', 5), s + 4);
    EXPECT_PTR("memchr outside its length", memchr(s, 'd', 4), NULL);
    EXPECT_PTR("memchr of 0x1ff finds 0xff", memchr(a, 0x1ff, 8), a + 1);
    return PASS;
}

static const char *memcmp_unsigned(void) {
    CHECK(memcmp("\x80", "\x01", 1) > 0, "memcmp compares bytes as signed: 0x80 sorted before 0x01");
    CHECK(memcmp("abc", "abd", 2) == 0, "memcmp looked past its length");
    CHECK(memcmp("abc", "abd", 3) < 0, "memcmp(\"abc\", \"abd\", 3) is not negative");
    EXPECT_INT("memcmp of zero bytes", memcmp("a", "b", 0), 0);
    return PASS;
}

static const char *strlen_strnlen(void) {
    EXPECT_UINT("strlen(\"\")", strlen(""), 0);
    EXPECT_UINT("strlen(\"hello\")", strlen("hello"), 5);
    EXPECT_UINT("strnlen(\"hello\", 3)", strnlen("hello", 3), 3);
    EXPECT_UINT("strnlen(\"hi\", 10)", strnlen("hi", 10), 2);
    return PASS;
}

static const char *strncpy_pads(void) {
    char b[8];
    memset(b, 'z', sizeof b);
    EXPECT_PTR("strncpy return", strncpy(b, "ab", 6), b);
    const char want[8] = { 'a', 'b', 0, 0, 0, 0, 'z', 'z' };
    EXPECT_MEM("strncpy pads with NUL to n", b, want, 8);
    memset(b, 'z', sizeof b);
    volatile size_t three = 3;
    strncpy(b, "abcdef", three);
    const char want2[4] = { 'a', 'b', 'c', 'z' };
    EXPECT_MEM("strncpy of a longer source writes no terminator", b, want2, 4);
    EXPECT_PTR("stpcpy returns the terminator", stpcpy(b, "xy"), b + 2);
    return PASS;
}

static const char *strcat_strncat(void) {
    char b[16] = "ab";
    EXPECT_PTR("strcat return", strcat(b, "cd"), b);
    EXPECT_STR("strcat", b, "abcd");
    strncat(b, "efgh", 2);
    EXPECT_STR("strncat appends n bytes and a terminator", b, "abcdef");
    strncat(b, "x", 5);
    EXPECT_STR("strncat of a shorter source", b, "abcdefx");
    return PASS;
}

static const char *strcmp_unsigned(void) {
    CHECK(strcmp("a\x80", "a\x01") > 0, "strcmp compares bytes as signed");
    CHECK(strcmp("abc", "abc") == 0, "strcmp of equal strings is not 0");
    CHECK(strcmp("ab", "abc") < 0, "a prefix does not sort first");
    CHECK(strncmp("abcx", "abcy", 3) == 0, "strncmp looked past n");
    CHECK(strncmp("\xff", "a", 1) > 0, "strncmp compares bytes as signed");
    CHECK(strcasecmp("HeLLo", "hello") == 0, "strcasecmp is not case-insensitive");
    CHECK(strncasecmp("ABx", "aby", 2) == 0, "strncasecmp looked past n");
    return PASS;
}

static const char *strchr_strrchr(void) {
    const char *s = "abcabc";
    EXPECT_PTR("strchr", strchr(s, 'b'), s + 1);
    EXPECT_PTR("strrchr", strrchr(s, 'b'), s + 4);
    EXPECT_PTR("strchr of the terminator", strchr(s, '\0'), s + 6);
    EXPECT_PTR("strrchr of the terminator", strrchr(s, '\0'), s + 6);
    EXPECT_PTR("strchr of a missing byte", strchr(s, 'z'), NULL);
    return PASS;
}

static const char *strstr_search(void) {
    const char *s = "hello world";
    EXPECT_PTR("strstr", strstr(s, "o w"), s + 4);
    EXPECT_PTR("strstr of an empty needle", strstr(s, ""), s);
    EXPECT_PTR("strstr of a missing needle", strstr(s, "worlds"), NULL);
    const char *t = "aaab";
    EXPECT_PTR("strstr with a partial match first", strstr(t, "aab"), t + 1);
    return PASS;
}

static const char *strspn_strpbrk(void) {
    EXPECT_UINT("strspn", strspn("aabbcd", "ab"), 4);
    EXPECT_UINT("strcspn", strcspn("hello, world", ",; "), 5);
    EXPECT_UINT("strcspn with no match", strcspn("abc", "xyz"), 3);
    const char *s = "key=value";
    EXPECT_PTR("strpbrk", strpbrk(s, "=:"), s + 3);
    EXPECT_PTR("strpbrk with no match", strpbrk(s, "!"), NULL);
    return PASS;
}

static const char *strtok_runs(void) {
    char b[] = ",,a,,b c,";
    EXPECT_STR("first token", strtok(b, ", "), "a");
    EXPECT_STR("second token", strtok(NULL, ", "), "b");
    EXPECT_STR("third token", strtok(NULL, ", "), "c");
    EXPECT_PTR("after the last token", strtok(NULL, ", "), NULL);
    char d[] = "x:y";
    char *save = NULL;
    EXPECT_STR("strtok_r first", strtok_r(d, ":", &save), "x");
    EXPECT_STR("strtok_r second", strtok_r(NULL, ":", &save), "y");
    EXPECT_PTR("strtok_r end", strtok_r(NULL, ":", &save), NULL);
    return PASS;
}

static const char *strdup_strndup(void) {
    const char *s = "copy me";
    char *a = strdup(s);
    CHECK(a != NULL && a != s, "strdup did not return a new string");
    EXPECT_STR("strdup", a, "copy me");
    char *b = strndup(s, 4);
    EXPECT_STR("strndup(s, 4)", b, "copy");
    char *c = strndup("ab", 10);
    EXPECT_STR("strndup of a shorter string", c, "ab");
    free(a);
    free(b);
    free(c);
    return PASS;
}

static const char *strerror_text(void) {
    const char *a = strerror(EINVAL);
    CHECK(a != NULL && a[0] != 0, "strerror(EINVAL) is empty");
    char buf[128];
    EXPECT_INT("strerror_r(ENOENT)", strerror_r(ENOENT, buf, sizeof buf), 0);
    CHECK(!h_streq(buf, a), "strerror_r(ENOENT) has the same text as EINVAL");
    char tiny[2];
    EXPECT_INT("strerror_r into a 2-byte buffer", strerror_r(ENOENT, tiny, sizeof tiny), ERANGE);
    return PASS;
}

static const char *strcoll_c_locale(void) {
    CHECK(strcoll("abc", "abd") < 0, "strcoll(\"abc\", \"abd\") is not negative in the C locale");
    CHECK(strcoll("b", "B") > 0, "strcoll does not order by byte value in the C locale");
    char x[16], y[16];
    EXPECT_UINT("strxfrm(\"abc\") length", strxfrm(x, "abc", sizeof x), 3);
    EXPECT_UINT("strxfrm(\"abd\") length", strxfrm(y, "abd", sizeof y), 3);
    CHECK(strcmp(x, y) < 0, "strcmp of strxfrm results disagrees with strcoll");
    return PASS;
}

static const char *strtol_bases(void) {
    char *end;
    EXPECT_INT("strtol(\"0x1A\", 0)", strtol("0x1A", &end, 0), 26);
    EXPECT_INT("strtol(\"017\", 0)", strtol("017", &end, 0), 15);
    EXPECT_INT("strtol(\"z\", 36)", strtol("z", &end, 36), 35);
    EXPECT_INT("strtol(\"0x1A\", 16)", strtol("0x1A", &end, 16), 26);
    const char *s = "  -42abc";
    EXPECT_INT("strtol(\"  -42abc\", 10)", strtol(s, &end, 10), -42);
    EXPECT_PTR("strtol end pointer", end, s + 5);
    EXPECT_INT("strtol(\"+7\", 10)", strtol("+7", &end, 10), 7);
    return PASS;
}

static const char *strtol_erange(void) {
    char *end;
    const char *s = "99999999999999999999x";
    errno = 0;
    EXPECT_RET_ERRNO("strtol of a value above LONG_MAX", strtol(s, &end, 10), LONG_MAX, ERANGE);
    EXPECT_PTR("end pointer after an out-of-range value", end, s + 20);
    errno = 0;
    EXPECT_RET_ERRNO("strtol of a value below LONG_MIN", strtol("-99999999999999999999", &end, 10), LONG_MIN, ERANGE);
    errno = 0;
    EXPECT_RET_ERRNO("strtoll(\"9223372036854775808\")", strtoll("9223372036854775808", &end, 10), LLONG_MAX, ERANGE);
    errno = 0;
    EXPECT_RET_ERRNO("strtol(\"9223372036854775807\")", strtol("9223372036854775807", &end, 10), LONG_MAX, 0);
    return PASS;
}

static const char *strtol_no_digits(void) {
    char *end;
    const char *s = "  xyz";
    EXPECT_INT("strtol of no digits", strtol(s, &end, 10), 0);
    EXPECT_PTR("end pointer with no digits", end, s);
    const char *h = "0xg";
    EXPECT_INT("strtol(\"0xg\", 16)", strtol(h, &end, 16), 0);
    EXPECT_PTR("end pointer after \"0\" of \"0xg\"", end, h + 1);
    errno = 0;
    EXPECT_RET_ERRNO("strtol with base 1", strtol("1", &end, 1), 0, EINVAL);
    return PASS;
}

static const char *strtoul_negative(void) {
    char *end;
    errno = 0;
    EXPECT_UINT("strtoul(\"-1\")", strtoul("-1", &end, 10), ULONG_MAX);
    EXPECT_ERRNO("strtoul(\"-1\")", 0);
    errno = 0;
    unsigned long long v = strtoull("18446744073709551616", &end, 10);
    EXPECT_UINT("strtoull above ULLONG_MAX", v, ULLONG_MAX);
    EXPECT_ERRNO("strtoull above ULLONG_MAX", ERANGE);
    return PASS;
}

static const char *strtod_forms(void) {
    char *end;
    const char *s = "3.25e2xyz";
    EXPECT_DBL("strtod(\"3.25e2xyz\")", strtod(s, &end), 325.0);
    EXPECT_PTR("strtod end pointer", end, s + 6);
    EXPECT_DBL("strtod(\"  -0.5\")", strtod("  -0.5", &end), -0.5);
    EXPECT_DBL("strtod(\"0x1.8p1\")", strtod("0x1.8p1", &end), 3.0);
    EXPECT_DBL("strtod(\"-INFINITY\")", strtod("-INFINITY", &end), -INFINITY);
    const char *n = "nan(123)z";
    CHECK(isnan(strtod(n, &end)), "strtod(\"nan(123)\") is not a NaN");
    EXPECT_PTR("end pointer after nan(123)", end, n + 8);
    EXPECT_DBL("strtod(\"-0\")", strtod("-0", &end), -0.0);
    return PASS;
}

static const char *strtod_erange(void) {
    char *end;
    errno = 0;
    EXPECT_DBL("strtod(\"1e999\")", strtod("1e999", &end), HUGE_VAL);
    EXPECT_ERRNO("strtod(\"1e999\")", ERANGE);
    errno = 0;
    EXPECT_DBL("strtod(\"-1e999\")", strtod("-1e999", &end), -HUGE_VAL);
    EXPECT_ERRNO("strtod(\"-1e999\")", ERANGE);
    errno = 0;
    double tiny = strtod("1e-999", &end);
    CHECK(tiny >= 0 && tiny <= 2.2250738585072014e-308, "strtod(\"1e-999\") is larger than DBL_MIN");
    EXPECT_ERRNO("strtod(\"1e-999\")", ERANGE);
    return PASS;
}

static const char *atoi_atol(void) {
    EXPECT_INT("atoi(\"  12abc\")", atoi("  12abc"), 12);
    EXPECT_INT("atoi(\"-7\")", atoi("-7"), -7);
    EXPECT_INT("atol(\"123456789\")", atol("123456789"), 123456789L);
    EXPECT_INT("atoll(\"-9000000000\")", atoll("-9000000000"), -9000000000LL);
    EXPECT_DBL("atof(\"2.5\")", atof("2.5"), 2.5);
    return PASS;
}

static int cmp_int(const void *a, const void *b) {
    int x = *(const int *)a, y = *(const int *)b;
    return (x > y) - (x < y);
}

static const char *qsort_ints(void) {
    enum { N = 1000 };
    static int v[N];
    unsigned seed = 12345;
    long sum = 0, sorted_sum = 0;
    for (int i = 0; i < N; i++) {
        seed = seed * 1103515245u + 12345u;
        v[i] = (int)(seed >> 8) % 5000 - 2500;
        sum += v[i];
    }
    qsort(v, N, sizeof v[0], cmp_int);
    for (int i = 0; i < N; i++) {
        sorted_sum += v[i];
        if (i && v[i - 1] > v[i])
            return fail_int("qsort left an element out of order at index", i, -1);
    }
    EXPECT_INT("sum of the elements after qsort", sorted_sum, sum);
    qsort(v, 0, sizeof v[0], cmp_int);
    return PASS;
}

struct rec { char name[12]; int key; long pad; };

static int cmp_rec(const void *a, const void *b) {
    return cmp_int(&((const struct rec *)a)->key, &((const struct rec *)b)->key);
}

static const char *qsort_bsearch(void) {
    struct rec r[5] = { { "e", 50, 0 }, { "a", 10, 0 }, { "d", 40, 0 }, { "b", 20, 0 }, { "c", 30, 0 } };
    qsort(r, 5, sizeof r[0], cmp_rec);
    for (int i = 0; i < 5; i++)
        EXPECT_INT("key of the sorted record", r[i].key, (i + 1) * 10);
    EXPECT_STR("record moved whole", r[3].name, "d");
    struct rec want = { "", 30, 0 };
    EXPECT_PTR("bsearch of a present key", bsearch(&want, r, 5, sizeof r[0], cmp_rec), &r[2]);
    want.key = 35;
    EXPECT_PTR("bsearch of a missing key", bsearch(&want, r, 5, sizeof r[0], cmp_rec), NULL);
    EXPECT_PTR("bsearch of an empty array", bsearch(&want, r, 0, sizeof r[0], cmp_rec), NULL);
    return PASS;
}

static const char *malloc_alignment(void) {
    for (size_t n = 1; n <= 4096; n *= 3) {
        void *p = malloc(n);
        CHECK(p != NULL, "malloc of a small size failed");
        EXPECT_UINT("malloc result modulo alignof(max_align_t)", (uintptr_t)p % _Alignof(max_align_t), 0);
        memset(p, 0xab, n);
        free(p);
    }
    void *z = malloc(0);
    free(z);
    free(NULL);
    return PASS;
}

static const char *malloc_enomem(void) {
    volatile size_t max = SIZE_MAX;
    errno = 0;
    void *p = malloc(max);
    CHECK(p == NULL, "malloc(SIZE_MAX) returned memory");
    EXPECT_ERRNO("malloc(SIZE_MAX)", ENOMEM);
    errno = 0;
    p = malloc(max / 2);
    CHECK(p == NULL, "malloc(SIZE_MAX / 2) returned memory");
    EXPECT_ERRNO("malloc(SIZE_MAX / 2)", ENOMEM);
    return PASS;
}

static const char *calloc_overflow(void) {
    unsigned char *p = calloc(100, 8);
    CHECK(p != NULL, "calloc(100, 8) failed");
    for (int i = 0; i < 800; i++)
        if (p[i])
            return fail_int("calloc left a nonzero byte at", i, -1);
    free(p);
    volatile size_t half = SIZE_MAX / 2 + 1;
    errno = 0;
    void *q = calloc(half, 2);
    CHECK(q == NULL, "calloc whose product overflows size_t returned memory");
    EXPECT_ERRNO("calloc(SIZE_MAX / 2 + 1, 2)", ENOMEM);
    return PASS;
}

static const char *realloc_preserves(void) {
    char *p = realloc(NULL, 4);
    CHECK(p != NULL, "realloc(NULL, 4) failed");
    memcpy(p, "abc", 4);
    p = realloc(p, 100000);
    CHECK(p != NULL, "realloc to grow failed");
    EXPECT_STR("contents after growing", p, "abc");
    p[99999] = 'z';
    p = realloc(p, 2);
    CHECK(p != NULL, "realloc to shrink failed");
    EXPECT_MEM("contents after shrinking", p, "ab", 2);
    free(p);
    return PASS;
}

static const char *realloc_failure_keeps(void) {
    char *p = malloc(8);
    CHECK(p != NULL, "malloc failed");
    memcpy(p, "keep", 5);
    volatile size_t huge = SIZE_MAX - 64;
    errno = 0;
    char *q = realloc(p, huge);
    CHECK(q == NULL, "realloc to a huge size returned memory");
    EXPECT_ERRNO("realloc to a huge size", ENOMEM);
    EXPECT_STR("block after the failed realloc", p, "keep");
    free(p);
    return PASS;
}

static const char *aligned_alloc_rules(void) {
    void *p = NULL;
    EXPECT_INT("posix_memalign(64, 100)", posix_memalign(&p, 64, 100), 0);
    EXPECT_UINT("posix_memalign result modulo 64", (uintptr_t)p % 64, 0);
    free(p);
    void *q = (void *)1;
    EXPECT_INT("posix_memalign with alignment 24", posix_memalign(&q, 24, 100), EINVAL);
    EXPECT_PTR("pointer after a failed posix_memalign", q, (void *)1);
    void *r = aligned_alloc(256, 512);
    CHECK(r != NULL, "aligned_alloc(256, 512) failed");
    EXPECT_UINT("aligned_alloc result modulo 256", (uintptr_t)r % 256, 0);
    free(r);
    return PASS;
}

static int exit_pipe = -1;
static void at_a(void) { if (write(exit_pipe, "a", 1) != 1) _exit(2); }
static void at_b(void) { if (write(exit_pipe, "b", 1) != 1) _exit(2); }
static void at_c(void) { if (write(exit_pipe, "c", 1) != 1) _exit(2); }

static const char *atexit_order(void) {
    int p[2];
    char b[16];
    CHECK(pipe(p) == 0, "could not make a pipe");
    pid_t pid = fork();
    CHECK(pid >= 0, "fork failed");
    if (pid == 0) {
        close(p[0]);
        exit_pipe = p[1];
        if (atexit(at_a) || atexit(at_b) || atexit(at_c))
            _exit(1);
        exit(0);
    }
    close(p[1]);
    ssize_t n = 0, r;
    while ((r = read(p[0], b + n, sizeof b - 1 - (size_t)n)) > 0)
        n += r;
    b[n] = 0;
    int status = 0;
    waitpid(pid, &status, 0);
    EXPECT_STR("order the atexit handlers ran in", b, "cba");
    CHECK(WIFEXITED(status) && WEXITSTATUS(status) == 0, "the child did not exit with status 0");
    return PASS;
}

static const char *setenv_rules(void) {
    EXPECT_INT("setenv(\"SV\", \"1\", 0)", setenv("SV", "1", 0), 0);
    EXPECT_INT("setenv(\"SV\", \"2\", 0)", setenv("SV", "2", 0), 0);
    EXPECT_STR("SV after a setenv without overwrite", getenv("SV"), "1");
    EXPECT_INT("setenv(\"SV\", \"3\", 1)", setenv("SV", "3", 1), 0);
    EXPECT_STR("SV after a setenv with overwrite", getenv("SV"), "3");
    EXPECT_INT("unsetenv(\"SV\")", unsetenv("SV"), 0);
    EXPECT_PTR("getenv after unsetenv", getenv("SV"), NULL);
    errno = 0;
    EXPECT_RET_ERRNO("setenv(\"A=B\", ...)", setenv("A=B", "x", 1), -1, EINVAL);
    errno = 0;
    EXPECT_RET_ERRNO("setenv(\"\", ...)", setenv("", "x", 1), -1, EINVAL);
    return PASS;
}

static const char *div_abs(void) {
    div_t d = div(-7, 2);
    EXPECT_INT("div(-7, 2).quot", d.quot, -3);
    EXPECT_INT("div(-7, 2).rem", d.rem, -1);
    ldiv_t l = ldiv(7L, -2L);
    EXPECT_INT("ldiv(7, -2).quot", l.quot, -3);
    EXPECT_INT("ldiv(7, -2).rem", l.rem, 1);
    EXPECT_INT("abs(-5)", abs(-5), 5);
    EXPECT_INT("labs(-5)", labs(-5L), 5);
    EXPECT_INT("llabs(-5)", llabs(-5LL), 5);
    return PASS;
}

static const char *rand_repeats(void) {
    int first[8];
    srand(7);
    for (int i = 0; i < 8; i++) {
        first[i] = rand();
        CHECK(first[i] >= 0 && first[i] <= RAND_MAX, "rand returned a value outside 0..RAND_MAX");
    }
    srand(7);
    for (int i = 0; i < 8; i++)
        EXPECT_INT("rand after the same srand seed", rand(), first[i]);
    CHECK(RAND_MAX >= 32767, "RAND_MAX is below 32767");
    return PASS;
}

const struct libc_case libc_cases[] = {
    { "memcpy-memmove", memcpy_memmove },
    { "memset-memchr", memset_memchr },
    { "memcmp-unsigned", memcmp_unsigned },
    { "strlen-strnlen", strlen_strnlen },
    { "strncpy-pads", strncpy_pads },
    { "strcat-strncat", strcat_strncat },
    { "strcmp-unsigned", strcmp_unsigned },
    { "strchr-strrchr", strchr_strrchr },
    { "strstr-search", strstr_search },
    { "strspn-strpbrk", strspn_strpbrk },
    { "strtok-runs", strtok_runs },
    { "strdup-strndup", strdup_strndup },
    { "strerror-text", strerror_text },
    { "strcoll-c-locale", strcoll_c_locale },
    { "strtol-bases", strtol_bases },
    { "strtol-erange", strtol_erange },
    { "strtol-no-digits", strtol_no_digits },
    { "strtoul-negative", strtoul_negative },
    { "strtod-forms", strtod_forms },
    { "strtod-erange", strtod_erange },
    { "atoi-atol", atoi_atol },
    { "qsort-ints", qsort_ints },
    { "qsort-bsearch", qsort_bsearch },
    { "malloc-alignment", malloc_alignment },
    { "malloc-enomem", malloc_enomem },
    { "calloc-overflow", calloc_overflow },
    { "realloc-preserves", realloc_preserves },
    { "realloc-failure-keeps", realloc_failure_keeps },
    { "aligned-alloc", aligned_alloc_rules },
    { "atexit-order", atexit_order },
    { "setenv-rules", setenv_rules },
    { "div-abs", div_abs },
    { "rand-repeats", rand_repeats },
    { 0, 0 },
};
