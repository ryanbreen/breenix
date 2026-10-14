/* libc suite, stdio category: <stdio.h> streams as ISO C17 and POSIX.1-2024 specify them. */
#define _POSIX_C_SOURCE 202405L
#include "case.h"

#include <errno.h>
#include <fcntl.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

/* A pipe whose read end does not block; 0 or -1. */
static int nb_pipe(int fds[2]) {
    if (pipe(fds) != 0)
        return -1;
    return fcntl(fds[0], F_SETFL, O_NONBLOCK) == 0 ? 0 : -1;
}

static const char *fopen_missing(void) {
    errno = 0;
    FILE *f = fopen("missing", "r");
    CHECK(f == NULL, "fopen(\"missing\", \"r\") returned a stream");
    EXPECT_ERRNO("fopen(\"missing\", \"r\")", ENOENT);
    return PASS;
}

static const char *fopen_w_truncates(void) {
    struct stat st;
    CHECK(h_put_file("f", "hello") == 0, "could not create the file");
    FILE *f = fopen("f", "w");
    CHECK(f != NULL, "fopen(\"f\", \"w\") failed");
    EXPECT_INT("fclose", fclose(f), 0);
    CHECK(stat("f", &st) == 0, "stat failed");
    EXPECT_INT("size after fopen \"w\"", st.st_size, 0);
    return PASS;
}

static const char *fopen_a_appends(void) {
    char buf[32];
    CHECK(h_put_file("f", "ab") == 0, "could not create the file");
    FILE *f = fopen("f", "a");
    CHECK(f != NULL, "fopen(\"f\", \"a\") failed");
    EXPECT_INT("fseek to the start", fseek(f, 0, SEEK_SET), 0);
    CHECK(fputs("cd", f) >= 0, "fputs failed");
    EXPECT_INT("fclose", fclose(f), 0);
    h_read_file("f", buf, sizeof buf);
    EXPECT_STR("file after appending at a sought-to start", buf, "abcd");
    return PASS;
}

static const char *fopen_rplus_update(void) {
    char buf[32];
    CHECK(h_put_file("f", "abc") == 0, "could not create the file");
    FILE *f = fopen("f", "r+");
    CHECK(f != NULL, "fopen(\"f\", \"r+\") failed");
    EXPECT_INT("first fgetc", fgetc(f), 'a');
    EXPECT_INT("fseek between reading and writing", fseek(f, 0, SEEK_CUR), 0);
    EXPECT_INT("fputc", fputc('X', f), 'X');
    EXPECT_INT("fclose", fclose(f), 0);
    h_read_file("f", buf, sizeof buf);
    EXPECT_STR("file after an r+ update", buf, "aXc");
    return PASS;
}

static const char *fopen_x_exclusive(void) {
    CHECK(h_put_file("f", "x") == 0, "could not create the file");
    errno = 0;
    FILE *f = fopen("f", "wx");
    CHECK(f == NULL, "fopen(\"f\", \"wx\") opened an existing file");
    EXPECT_ERRNO("fopen(\"f\", \"wx\") on an existing file", EEXIST);
    f = fopen("g", "wx");
    CHECK(f != NULL, "fopen(\"g\", \"wx\") failed to create a new file");
    EXPECT_INT("fclose", fclose(f), 0);
    return PASS;
}

static const char *fopen_create_mode(void) {
    struct stat st;
    umask(022);
    FILE *f = fopen("new", "w");
    CHECK(f != NULL, "fopen(\"new\", \"w\") failed");
    EXPECT_INT("fclose", fclose(f), 0);
    CHECK(stat("new", &st) == 0, "stat failed");
    EXPECT_INT("mode of a file fopen created under umask 022", st.st_mode & 0777, 0644);
    return PASS;
}

static const char *setvbuf_full(void) {
    int p[2];
    char buf[32];
    CHECK(nb_pipe(p) == 0, "could not make a pipe");
    FILE *f = fdopen(p[1], "w");
    CHECK(f != NULL, "fdopen failed");
    EXPECT_INT("setvbuf(_IOFBF, 64)", setvbuf(f, NULL, _IOFBF, 64), 0);
    CHECK(fputs("abc", f) >= 0, "fputs failed");
    EXPECT_STR("pipe before fflush", (h_read_now(p[0], buf, sizeof buf), buf), "");
    EXPECT_INT("fflush", fflush(f), 0);
    EXPECT_STR("pipe after fflush", (h_read_now(p[0], buf, sizeof buf), buf), "abc");
    return PASS;
}

static const char *setvbuf_line(void) {
    int p[2];
    char buf[32];
    CHECK(nb_pipe(p) == 0, "could not make a pipe");
    FILE *f = fdopen(p[1], "w");
    CHECK(f != NULL, "fdopen failed");
    EXPECT_INT("setvbuf(_IOLBF, 64)", setvbuf(f, NULL, _IOLBF, 64), 0);
    CHECK(fputs("ab", f) >= 0, "fputs failed");
    EXPECT_STR("pipe before a newline", (h_read_now(p[0], buf, sizeof buf), buf), "");
    CHECK(fputs("c\n", f) >= 0, "fputs failed");
    EXPECT_STR("pipe after a newline", (h_read_now(p[0], buf, sizeof buf), buf), "abc\n");
    return PASS;
}

static const char *setvbuf_none(void) {
    int p[2];
    char buf[32];
    CHECK(nb_pipe(p) == 0, "could not make a pipe");
    FILE *f = fdopen(p[1], "w");
    CHECK(f != NULL, "fdopen failed");
    EXPECT_INT("setvbuf(_IONBF)", setvbuf(f, NULL, _IONBF, 0), 0);
    EXPECT_INT("fputc", fputc('x', f), 'x');
    EXPECT_STR("pipe after one fputc", (h_read_now(p[0], buf, sizeof buf), buf), "x");
    return PASS;
}

static const char *fflush_all(void) {
    int p[2], q[2];
    char buf[32];
    CHECK(nb_pipe(p) == 0 && nb_pipe(q) == 0, "could not make the pipes");
    FILE *f = fdopen(p[1], "w"), *g = fdopen(q[1], "w");
    CHECK(f && g, "fdopen failed");
    setvbuf(f, NULL, _IOFBF, 64);
    setvbuf(g, NULL, _IOFBF, 64);
    fputs("one", f);
    fputs("two", g);
    EXPECT_INT("fflush(NULL)", fflush(NULL), 0);
    EXPECT_STR("first pipe after fflush(NULL)", (h_read_now(p[0], buf, sizeof buf), buf), "one");
    EXPECT_STR("second pipe after fflush(NULL)", (h_read_now(q[0], buf, sizeof buf), buf), "two");
    return PASS;
}

static const char *fseek_ftell(void) {
    CHECK(h_put_file("f", "0123456789") == 0, "could not create the file");
    FILE *f = fopen("f", "r");
    CHECK(f != NULL, "fopen failed");
    EXPECT_INT("fseek(3, SEEK_SET)", fseek(f, 3, SEEK_SET), 0);
    EXPECT_INT("ftell after SEEK_SET 3", ftell(f), 3);
    EXPECT_INT("fgetc at 3", fgetc(f), '3');
    EXPECT_INT("fseek(-2, SEEK_END)", fseek(f, -2, SEEK_END), 0);
    EXPECT_INT("ftell after SEEK_END -2", ftell(f), 8);
    EXPECT_INT("fseek(-3, SEEK_CUR)", fseek(f, -3, SEEK_CUR), 0);
    EXPECT_INT("fgetc after SEEK_CUR -3", fgetc(f), '5');
    rewind(f);
    EXPECT_INT("ftell after rewind", ftell(f), 0);
    return PASS;
}

static const char *fseek_einval(void) {
    CHECK(h_put_file("f", "abc") == 0, "could not create the file");
    FILE *f = fopen("f", "r");
    CHECK(f != NULL, "fopen failed");
    errno = 0;
    EXPECT_RET_ERRNO("fseek(f, -1, SEEK_SET)", fseek(f, -1, SEEK_SET), -1, EINVAL);
    EXPECT_INT("ftell after the failed fseek", ftell(f), 0);
    return PASS;
}

static const char *ungetc_pushback(void) {
    char buf[32];
    CHECK(h_put_file("f", "abc") == 0, "could not create the file");
    FILE *f = fopen("f", "r");
    CHECK(f != NULL, "fopen failed");
    EXPECT_INT("fgetc", fgetc(f), 'a');
    EXPECT_INT("ungetc('z')", ungetc('z', f), 'z');
    EXPECT_INT("fgetc after ungetc", fgetc(f), 'z');
    EXPECT_INT("next fgetc", fgetc(f), 'b');
    EXPECT_INT("ungetc(EOF)", ungetc(EOF, f), EOF);
    EXPECT_INT("fgetc after ungetc(EOF)", fgetc(f), 'c');
    fclose(f);
    h_read_file("f", buf, sizeof buf);
    EXPECT_STR("file after ungetc", buf, "abc");
    return PASS;
}

static const char *ungetc_clears_eof(void) {
    CHECK(h_put_file("f", "a") == 0, "could not create the file");
    FILE *f = fopen("f", "r");
    CHECK(f != NULL, "fopen failed");
    fgetc(f);
    EXPECT_INT("fgetc at end of file", fgetc(f), EOF);
    CHECK(feof(f), "feof is clear after reading past the end");
    EXPECT_INT("ungetc('x') at end of file", ungetc('x', f), 'x');
    CHECK(!feof(f), "ungetc did not clear the end-of-file indicator");
    EXPECT_INT("fgetc after ungetc", fgetc(f), 'x');
    EXPECT_INT("fgetc after the pushed-back byte", fgetc(f), EOF);
    return PASS;
}

static const char *printf_integers(void) {
    char b[128];
    int n = snprintf(b, sizeof b, "%d|%i|%u|%o|%x|%X|%#o|%#x|%d", -42, 42, 4294967295u, 8, 255, 255, 8, 255, 0);
    EXPECT_STR("integer conversions", b, "-42|42|4294967295|10|ff|FF|010|0xff|0");
    EXPECT_INT("snprintf return", n, 37);
    return PASS;
}

static const char *printf_flags(void) {
    char b[128];
    snprintf(b, sizeof b, "%5d|%-5d|%05d|%+d|% d|%.3d|%8.3d|%-+6d|%.0d", 42, 42, 42, 42, 42, 7, 7, 7, 0);
    EXPECT_STR("flags, field widths and precisions", b, "   42|42   |00042|+42| 42|007|     007|+7    |");
    return PASS;
}

static const char *printf_strings(void) {
    char b[128];
    snprintf(b, sizeof b, "%c|%s|%.2s|%5s|%-5s|%.0s|%%", 'x', "abc", "abc", "ab", "ab", "abc");
    EXPECT_STR("%c and %s conversions", b, "x|abc|ab|   ab|ab   ||%");
    return PASS;
}

static const char *printf_floats(void) {
    char b[128];
    snprintf(b, sizeof b, "%.2f|%e|%g|%G|%.0f|%#.0f|%10.3f|%g", 3.14159, 12345.678, 0.0001, 1e20, 2.5, 2.0, -1.5, 100000.0);
    EXPECT_STR("floating conversions", b, "3.14|1.234568e+04|0.0001|1E+20|2|2.|    -1.500|100000");
    return PASS;
}

static const char *printf_length(void) {
    char b[128];
    snprintf(b, sizeof b, "%ld|%lld|%hd|%hhd|%zu|%jd|%td|%hhu", -1L, 9223372036854775807LL, 70000, 300,
             (size_t)7, (intmax_t)-9, (ptrdiff_t)3, 257);
    EXPECT_STR("length modifiers", b, "-1|9223372036854775807|4464|44|7|-9|3|1");
    return PASS;
}

static const char *printf_inf_nan(void) {
    char b[64];
    snprintf(b, sizeof b, "%f", INFINITY);
    CHECK(h_streq(b, "inf") || h_streq(b, "infinity"), "%f of INFINITY is neither inf nor infinity");
    snprintf(b, sizeof b, "%F", -INFINITY);
    CHECK(h_streq(b, "-INF") || h_streq(b, "-INFINITY"), "%F of -INFINITY is neither -INF nor -INFINITY");
    snprintf(b, sizeof b, "%e", fabs(NAN));
    CHECK(b[0] == 'n' && b[1] == 'a' && b[2] == 'n', "%e of a positive NaN does not start with nan");
    snprintf(b, sizeof b, "%5.1f", INFINITY);
    CHECK(h_streq(b, "  inf") || h_streq(b, "infinity"), "%5.1f of INFINITY ignores the field width");
    return PASS;
}

static const char *snprintf_truncate(void) {
    char b[8] = "zzzzzzz";
    volatile size_t five = 5;
    EXPECT_INT("snprintf(b, 5, \"abcdefgh\")", snprintf(b, five, "abcdefgh"), 8);
    EXPECT_STR("truncated buffer", b, "abcd");
    EXPECT_INT("snprintf(NULL, 0, \"%d\", 12345)", snprintf(NULL, 0, "%d", 12345), 5);
    b[0] = 'q';
    EXPECT_INT("snprintf(b, 0, \"x\")", snprintf(b, 0, "x"), 1);
    EXPECT_INT("buffer after a zero-size snprintf", b[0], 'q');
    return PASS;
}

static const char *scanf_integers(void) {
    int a = 0, b = 0, d = 0, e = 0;
    unsigned c = 0, o = 0;
    int n = sscanf("  42 -17 0x1f 017 0x10 010", "%d %d %x %o %i %i", &a, &b, &c, &o, &d, &e);
    EXPECT_INT("items converted", n, 6);
    EXPECT_INT("%d of 42", a, 42);
    EXPECT_INT("%d of -17", b, -17);
    EXPECT_INT("%x of 0x1f", c, 31);
    EXPECT_INT("%o of 017", o, 15);
    EXPECT_INT("%i of 0x10", d, 16);
    EXPECT_INT("%i of 010", e, 8);
    return PASS;
}

static const char *scanf_widths_sets(void) {
    char a[8], b[8], c[8], d[8];
    int v = 0;
    EXPECT_INT("sscanf(\"abcdef123\", \"%3s%[a-z]%2d\")", sscanf("abcdef123", "%3s%7[a-z]%2d", a, b, &v), 3);
    EXPECT_STR("%3s", a, "abc");
    EXPECT_STR("%[a-z]", b, "def");
    EXPECT_INT("%2d", v, 12);
    EXPECT_INT("sscanf(\"x y,z\", \"%[^,],%s\")", sscanf("x y,z", "%7[^,],%7s", c, d), 2);
    EXPECT_STR("%[^,]", c, "x y");
    EXPECT_STR("%s after the comma", d, "z");
    return PASS;
}

static const char *scanf_floats(void) {
    double x = 0, z = 0;
    float y = 0;
    EXPECT_INT("items converted", sscanf("3.5e2 -0.25 inf", "%lf %f %lf", &x, &y, &z), 3);
    EXPECT_DBL("%lf of 3.5e2", x, 350.0);
    EXPECT_DBL("%f of -0.25", (double)y, -0.25);
    EXPECT_DBL("%lf of inf", z, INFINITY);
    return PASS;
}

static const char *scanf_return(void) {
    int v = 0, n = 0;
    EXPECT_INT("sscanf(\"\", \"%d\")", sscanf("", "%d", &v), EOF);
    EXPECT_INT("sscanf(\"x\", \"%d\")", sscanf("x", "%d", &v), 0);
    EXPECT_INT("sscanf(\"12 34\", \"%*d %d%n\")", sscanf("12 34", "%*d %d%n", &v, &n), 1);
    EXPECT_INT("value after a suppressed conversion", v, 34);
    EXPECT_INT("%n count", n, 5);
    return PASS;
}

static const char *fgets_lines(void) {
    char b[16];
    CHECK(h_put_file("f", "ab\ncd") == 0, "could not create the file");
    FILE *f = fopen("f", "r");
    CHECK(f != NULL, "fopen failed");
    EXPECT_STR("first fgets", fgets(b, sizeof b, f), "ab\n");
    EXPECT_STR("second fgets", fgets(b, sizeof b, f), "cd");
    EXPECT_PTR("fgets at end of file", fgets(b, sizeof b, f), NULL);
    CHECK(feof(f), "feof is clear after fgets reached the end");
    f = fopen("f", "r");
    CHECK(f != NULL, "fopen failed");
    EXPECT_STR("fgets with room for two bytes", fgets(b, 3, f), "ab");
    EXPECT_STR("fgets continuing the line", fgets(b, 3, f), "\n");
    return PASS;
}

static const char *getline_grows(void) {
    char text[310];
    for (int i = 0; i < 300; i++)
        text[i] = 'x';
    text[300] = '\n';
    text[301] = 'z';
    text[302] = 0;
    CHECK(h_put_file("f", text) == 0, "could not create the file");
    FILE *f = fopen("f", "r");
    CHECK(f != NULL, "fopen failed");
    char *line = NULL;
    size_t cap = 0;
    EXPECT_INT("getline of a 301-byte line", getline(&line, &cap, f), 301);
    CHECK(line != NULL && cap > 301, "getline did not grow the buffer past the line");
    EXPECT_INT("byte before the terminator", line[300], '\n');
    EXPECT_INT("terminator", line[301], 0);
    EXPECT_INT("getline of the unterminated last line", getline(&line, &cap, f), 1);
    EXPECT_STR("last line", line, "z");
    EXPECT_INT("getline at end of file", getline(&line, &cap, f), -1);
    CHECK(feof(f), "feof is clear after getline reached the end");
    free(line);
    return PASS;
}

static const char *freopen_stdout(void) {
    char b[32];
    CHECK(freopen("out", "w", stdout) != NULL, "freopen(\"out\", \"w\", stdout) failed");
    fputs("hi", stdout);
    printf("%d", 5);
    EXPECT_INT("fclose(stdout)", fclose(stdout), 0);
    h_read_file("out", b, sizeof b);
    EXPECT_STR("file stdout was reopened on", b, "hi5");
    return PASS;
}

static const char *tmpfile_rw(void) {
    char b[32];
    FILE *f = tmpfile();
    CHECK(f != NULL, "tmpfile failed");
    CHECK(fputs("scratch\n", f) >= 0, "fputs failed");
    rewind(f);
    EXPECT_STR("read back from tmpfile", fgets(b, sizeof b, f), "scratch\n");
    EXPECT_INT("fclose", fclose(f), 0);
    return PASS;
}

static const char *error_indicator(void) {
    CHECK(h_put_file("f", "a") == 0, "could not create the file");
    FILE *f = fopen("f", "r");
    CHECK(f != NULL, "fopen failed");
    EXPECT_INT("fputc on a stream opened for reading", fputc('x', f), EOF);
    CHECK(ferror(f), "ferror is clear after a failed write");
    CHECK(!feof(f), "feof is set after a failed write");
    clearerr(f);
    CHECK(!ferror(f), "clearerr did not clear the error indicator");
    return PASS;
}

static const char *eof_indicator(void) {
    CHECK(h_put_file("f", "a") == 0, "could not create the file");
    FILE *f = fopen("f", "r");
    CHECK(f != NULL, "fopen failed");
    EXPECT_INT("fgetc", fgetc(f), 'a');
    CHECK(!feof(f), "feof is set before a read went past the end");
    EXPECT_INT("fgetc at end of file", fgetc(f), EOF);
    CHECK(feof(f), "feof is clear after a read went past the end");
    CHECK(!ferror(f), "ferror is set by reaching the end of file");
    clearerr(f);
    CHECK(!feof(f), "clearerr did not clear the end-of-file indicator");
    return PASS;
}

static const char *open_memstream_grows(void) {
    char *p = NULL;
    size_t n = 99;
    FILE *f = open_memstream(&p, &n);
    CHECK(f != NULL, "open_memstream failed");
    fprintf(f, "x=%d", 42);
    EXPECT_INT("fflush", fflush(f), 0);
    EXPECT_UINT("size after fflush", n, 4);
    EXPECT_STR("buffer after fflush", p, "x=42");
    EXPECT_INT("fclose", fclose(f), 0);
    free(p);
    return PASS;
}

static const char *exit_flushes(void) {
    int p[2];
    char b[32];
    CHECK(pipe(p) == 0, "could not make a pipe");
    pid_t pid = fork();
    CHECK(pid >= 0, "fork failed");
    if (pid == 0) {
        close(p[0]);
        FILE *f = fdopen(p[1], "w");
        if (!f)
            _exit(1);
        setvbuf(f, NULL, _IOFBF, 64);
        fputs("pending", f);
        exit(0);
    }
    close(p[1]);
    long n = 0;
    for (;;) {
        ssize_t r = read(p[0], b + n, sizeof b - 1 - (size_t)n);
        if (r <= 0)
            break;
        n += r;
    }
    b[n] = 0;
    int status;
    waitpid(pid, &status, 0);
    EXPECT_STR("what exit() flushed", b, "pending");
    return PASS;
}

static const char *perror_format(void) {
    int p[2];
    char b[128], want[128];
    CHECK(nb_pipe(p) == 0, "could not make a pipe");
    CHECK(dup2(p[1], 2) == 2, "dup2 onto stderr failed");
    errno = ENOENT;
    perror("ctx");
    const char *text = strerror(ENOENT);
    size_t n = 0;
    for (const char *s = "ctx: "; *s; s++)
        want[n++] = *s;
    for (const char *s = text; *s && n < sizeof want - 2; s++)
        want[n++] = *s;
    want[n++] = '\n';
    want[n] = 0;
    h_read_now(p[0], b, sizeof b);
    EXPECT_STR("perror(\"ctx\") with errno ENOENT", b, want);
    return PASS;
}

static const char *fclose_closes(void) {
    char b[16];
    FILE *f = fopen("f", "w");
    CHECK(f != NULL, "fopen failed");
    int fd = fileno(f);
    CHECK(fd >= 0, "fileno failed");
    setvbuf(f, NULL, _IOFBF, 64);
    fputs("data", f);
    EXPECT_INT("fclose", fclose(f), 0);
    h_read_file("f", b, sizeof b);
    EXPECT_STR("file after fclose", b, "data");
    errno = 0;
    EXPECT_RET_ERRNO("fcntl(F_GETFD) on the closed stream's descriptor", fcntl(fd, F_GETFD), -1, EBADF);
    return PASS;
}

const struct libc_case libc_cases[] = {
    { "fopen-missing", fopen_missing },
    { "fopen-w-truncates", fopen_w_truncates },
    { "fopen-a-appends", fopen_a_appends },
    { "fopen-rplus-update", fopen_rplus_update },
    { "fopen-x-exclusive", fopen_x_exclusive },
    { "fopen-create-mode", fopen_create_mode },
    { "setvbuf-full", setvbuf_full },
    { "setvbuf-line", setvbuf_line },
    { "setvbuf-none", setvbuf_none },
    { "fflush-all", fflush_all },
    { "fseek-ftell", fseek_ftell },
    { "fseek-einval", fseek_einval },
    { "ungetc-pushback", ungetc_pushback },
    { "ungetc-clears-eof", ungetc_clears_eof },
    { "printf-integers", printf_integers },
    { "printf-flags", printf_flags },
    { "printf-strings", printf_strings },
    { "printf-floats", printf_floats },
    { "printf-length", printf_length },
    { "printf-inf-nan", printf_inf_nan },
    { "snprintf-truncate", snprintf_truncate },
    { "scanf-integers", scanf_integers },
    { "scanf-widths-sets", scanf_widths_sets },
    { "scanf-floats", scanf_floats },
    { "scanf-return", scanf_return },
    { "fgets-lines", fgets_lines },
    { "getline-grows", getline_grows },
    { "freopen-stdout", freopen_stdout },
    { "tmpfile-rw", tmpfile_rw },
    { "error-indicator", error_indicator },
    { "eof-indicator", eof_indicator },
    { "open-memstream", open_memstream_grows },
    { "exit-flushes", exit_flushes },
    { "perror-format", perror_format },
    { "fclose-closes", fclose_closes },
    { 0, 0 },
};
