/*
 * The libc suite's case runner: looks up argv[1] in the category's table, runs it, and
 * writes the result line to the descriptor named by argv[2]. See case.h.
 */
#include "case.h"

#include <errno.h>
#include <fcntl.h>
#include <unistd.h>

char **libc_case_envp;

static int result_fd = -1;

static char msg[512];
static size_t msg_len;

static void put(const char *s) {
    while (*s && msg_len < sizeof msg - 1)
        msg[msg_len++] = *s++;
    msg[msg_len] = 0;
}

static void put_char(char c) {
    char s[2] = { c, 0 };
    put(s);
}

static void put_uint(unsigned long long v) {
    char digits[24];
    int n = 0;
    do {
        digits[n++] = (char)('0' + v % 10);
        v /= 10;
    } while (v);
    while (n)
        put_char(digits[--n]);
}

static void put_int(long long v) {
    if (v < 0) {
        put_char('-');
        put_uint(0 - (unsigned long long)v);
    } else {
        put_uint((unsigned long long)v);
    }
}

static void put_hex(unsigned long long v) {
    static const char hex[] = "0123456789abcdef";
    char digits[17];
    int n = 0;
    do {
        digits[n++] = hex[v & 15];
        v >>= 4;
    } while (v);
    put("0x");
    while (n)
        put_char(digits[--n]);
}

static void put_byte(unsigned char c) {
    static const char hex[] = "0123456789abcdef";
    if (c == '"' || c == '\\') {
        put_char('\\');
        put_char((char)c);
    } else if (c >= 0x20 && c < 0x7f) {
        put_char((char)c);
    } else {
        put("\\x");
        put_char(hex[c >> 4]);
        put_char(hex[c & 15]);
    }
}

static void put_quoted(const char *s) {
    if (!s) {
        put("(null)");
        return;
    }
    put_char('"');
    for (size_t i = 0; s[i] && i < 80; i++)
        put_byte((unsigned char)s[i]);
    put_char('"');
}

static const struct { int code; const char *name; } errnos[] = {
    { EPERM, "EPERM" }, { ENOENT, "ENOENT" }, { ESRCH, "ESRCH" }, { EINTR, "EINTR" },
    { EIO, "EIO" }, { ENXIO, "ENXIO" }, { E2BIG, "E2BIG" }, { ENOEXEC, "ENOEXEC" },
    { EBADF, "EBADF" }, { ECHILD, "ECHILD" }, { EAGAIN, "EAGAIN" }, { ENOMEM, "ENOMEM" },
    { EACCES, "EACCES" }, { EFAULT, "EFAULT" }, { EBUSY, "EBUSY" }, { EEXIST, "EEXIST" },
    { EXDEV, "EXDEV" }, { ENODEV, "ENODEV" }, { ENOTDIR, "ENOTDIR" }, { EISDIR, "EISDIR" },
    { EINVAL, "EINVAL" }, { ENFILE, "ENFILE" }, { EMFILE, "EMFILE" }, { ENOTTY, "ENOTTY" },
    { EFBIG, "EFBIG" }, { ENOSPC, "ENOSPC" }, { ESPIPE, "ESPIPE" }, { EROFS, "EROFS" },
    { EMLINK, "EMLINK" }, { EPIPE, "EPIPE" }, { EDOM, "EDOM" }, { ERANGE, "ERANGE" },
    { EDEADLK, "EDEADLK" }, { ENAMETOOLONG, "ENAMETOOLONG" }, { ENOSYS, "ENOSYS" },
    { ENOTEMPTY, "ENOTEMPTY" }, { ELOOP, "ELOOP" }, { EOVERFLOW, "EOVERFLOW" },
    { EILSEQ, "EILSEQ" }, { ETIMEDOUT, "ETIMEDOUT" }, { EOWNERDEAD, "EOWNERDEAD" },
    { ENOTRECOVERABLE, "ENOTRECOVERABLE" }, { ENOTSUP, "ENOTSUP" },
};

static void put_errno(int e) {
    if (e == 0) {
        put("0");
        return;
    }
    for (size_t i = 0; i < sizeof errnos / sizeof errnos[0]; i++) {
        if (errnos[i].code == e) {
            put(errnos[i].name);
            return;
        }
    }
    put("errno ");
    put_int(e);
}

union dbl_bits { double d; unsigned long long u; };

static void put_dbl(double d) {
    union dbl_bits b = { d };
    unsigned exp = (unsigned)((b.u >> 52) & 0x7ff);
    if (b.u >> 63)
        put_char('-');
    if (exp == 0x7ff) {
        put((b.u & ((1ULL << 52) - 1)) ? "nan" : "inf");
        return;
    }
    double a = d < 0 ? -d : d;
    if (a < 1e15 && (a == 0 || a >= 1e-6)) {
        unsigned long long whole = (unsigned long long)a;
        double frac = a - (double)whole;
        put_uint(whole);
        put_char('.');
        for (int i = 0; i < 9; i++) {
            frac *= 10;
            int digit = (int)frac;
            put_char((char)('0' + digit));
            frac -= digit;
        }
    }
    put(" [");
    put_hex(b.u);
    put("]");
}

static const char *start(const char *what) {
    msg_len = 0;
    msg[0] = 0;
    put(what);
    return msg;
}

const char *fail_text(const char *what) {
    return start(what);
}

const char *fail_int(const char *what, long long got, long long want) {
    start(what);
    put(": got ");
    put_int(got);
    put(", expected ");
    put_int(want);
    return msg;
}

const char *fail_uint(const char *what, unsigned long long got, unsigned long long want) {
    start(what);
    put(": got ");
    put_uint(got);
    put(", expected ");
    put_uint(want);
    return msg;
}

const char *fail_errno(const char *what, int got, int want) {
    start(what);
    put(": errno ");
    put_errno(got);
    put(", expected ");
    put_errno(want);
    return msg;
}

const char *fail_ret_errno(const char *what, long long ret, int err, long long want_ret, int want_err) {
    start(what);
    put(" returned ");
    put_int(ret);
    put(" (errno ");
    put_errno(err);
    put("), expected ");
    put_int(want_ret);
    put(" with errno ");
    put_errno(want_err);
    return msg;
}

const char *fail_str(const char *what, const char *got, const char *want) {
    start(what);
    put(": got ");
    put_quoted(got);
    put(", expected ");
    put_quoted(want);
    return msg;
}

const char *fail_mem(const char *what, const void *got, const void *want, size_t n) {
    const unsigned char *g = got, *w = want;
    start(what);
    put(": got ");
    for (size_t i = 0; i < n && i < 32; i++)
        put_byte(g[i]);
    put(", expected ");
    for (size_t i = 0; i < n && i < 32; i++)
        put_byte(w[i]);
    return msg;
}

const char *fail_dbl(const char *what, double got, double want) {
    start(what);
    put(": got ");
    put_dbl(got);
    put(", expected ");
    put_dbl(want);
    return msg;
}

const char *fail_ptr(const char *what, const void *got, const void *want) {
    start(what);
    put(": got ");
    put_hex((unsigned long long)(size_t)got);
    put(", expected ");
    put_hex((unsigned long long)(size_t)want);
    return msg;
}

int h_streq(const char *a, const char *b) {
    if (!a || !b)
        return a == b;
    while (*a && *a == *b) {
        a++;
        b++;
    }
    return *a == *b;
}

int h_memeq(const void *a, const void *b, size_t n) {
    const unsigned char *x = a, *y = b;
    for (size_t i = 0; i < n; i++)
        if (x[i] != y[i])
            return 0;
    return 1;
}

size_t h_strlen(const char *s) {
    size_t n = 0;
    while (s[n])
        n++;
    return n;
}

int h_dbl_same(double got, double want) {
    union dbl_bits g = { got }, w = { want };
    if (want != want)
        return got != got;
    if (want == 0)
        return got == 0 && (g.u >> 63) == (w.u >> 63);
    return got == want;
}

int h_put_file(const char *name, const char *text) {
    int fd = open(name, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0)
        return -1;
    size_t n = h_strlen(text);
    ssize_t w = write(fd, text, n);
    close(fd);
    return w == (ssize_t)n ? 0 : -1;
}

long h_read_file(const char *name, char *buf, size_t cap) {
    int fd = open(name, O_RDONLY);
    if (fd < 0)
        return -1;
    size_t n = 0;
    for (;;) {
        ssize_t r = read(fd, buf + n, cap - 1 - n);
        if (r <= 0)
            break;
        n += (size_t)r;
    }
    close(fd);
    buf[n] = 0;
    return (long)n;
}

long h_read_now(int fd, char *buf, size_t cap) {
    ssize_t r = read(fd, buf, cap - 1);
    if (r < 0)
        r = 0;
    buf[r] = 0;
    return (long)r;
}

static void write_all(int fd, const char *s, size_t n) {
    while (n) {
        ssize_t w = write(fd, s, n);
        if (w <= 0)
            return;
        s += w;
        n -= (size_t)w;
    }
}

static void report(int fd, const char *kind, const char *text) {
    char line[sizeof msg + 16];
    size_t n = 0;
    for (const char *k = kind; *k; k++)
        line[n++] = *k;
    if (text) {
        line[n++] = ' ';
        for (const char *t = text; *t && n < sizeof line - 2; t++)
            line[n++] = (*t == '\n' || *t == '\r') ? ' ' : *t;
    }
    line[n++] = '\n';
    write_all(fd, line, n);
}

void libc_case_missing(const char *name) {
    start(name);
    put(" is not in Breenix's C library yet");
    report(result_fd, "FAIL", msg);
    _exit(0);
}

int main(int argc, char **argv, char **envp) {
    libc_case_envp = envp;
    if (argc < 3)
        _exit(2);
    int fd = 0;
    for (const char *p = argv[2]; *p; p++) {
        if (*p < '0' || *p > '9')
            _exit(2);
        fd = fd * 10 + (*p - '0');
    }
    result_fd = fd;
    for (const struct libc_case *c = libc_cases; c->id; c++) {
        if (h_streq(c->id, argv[1])) {
            const char *result = c->run();
            if (!result)
                report(fd, "PASS", 0);
            else
                report(fd, "FAIL", result);
            _exit(0);
        }
    }
    start("the C program has no case ");
    put(argv[1]);
    report(fd, "FAIL", msg);
    _exit(0);
}
