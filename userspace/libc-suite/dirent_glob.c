/*
 * libc suite, dirent-glob category: <dirent.h>, <glob.h> and <fnmatch.h>. Each case
 * builds its own tree in the scratch directory the suite runs it in. No case assumes
 * readdir's order.
 */
#define _POSIX_C_SOURCE 202405L
#include "case.h"

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <fnmatch.h>
#include <glob.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static int make(const char *name) {
    return h_put_file(name, "x");
}

/* Count readdir's entries, and how many times `name` appears. */
static long scan(DIR *d, const char *name, int *seen) {
    long n = 0;
    struct dirent *e;
    *seen = 0;
    while ((e = readdir(d)) != NULL) {
        n++;
        if (h_streq(e->d_name, name))
            (*seen)++;
    }
    return n;
}

static const char *opendir_errors(void) {
    errno = 0;
    CHECK(opendir("nope") == NULL, "opendir of a missing directory returned a stream");
    EXPECT_ERRNO("opendir(\"nope\")", ENOENT);
    CHECK(make("file") == 0, "could not create the file");
    errno = 0;
    CHECK(opendir("file") == NULL, "opendir of a regular file returned a stream");
    EXPECT_ERRNO("opendir(\"file\")", ENOTDIR);
    return PASS;
}

static const char *readdir_lists(void) {
    const char *names[] = { ".", "..", "a", "b", "c" };
    int count[5] = { 0 };
    CHECK(make("a") == 0 && make("b") == 0 && make("c") == 0, "could not create the files");
    DIR *d = opendir(".");
    CHECK(d != NULL, "opendir(\".\") failed");
    struct dirent *e;
    long n = 0;
    errno = 0;
    while ((e = readdir(d)) != NULL) {
        n++;
        for (int i = 0; i < 5; i++)
            if (h_streq(e->d_name, names[i]))
                count[i]++;
    }
    EXPECT_ERRNO("errno after readdir reached the end", 0);
    EXPECT_INT("entries readdir returned", n, 5);
    for (int i = 0; i < 5; i++)
        if (count[i] != 1)
            return fail_str("an entry did not appear exactly once", names[i], names[i]);
    EXPECT_INT("closedir", closedir(d), 0);
    return PASS;
}

static const char *readdir_ino_type(void) {
    struct stat st;
    CHECK(make("f") == 0 && mkdir("d", 0755) == 0, "could not create the entries");
    CHECK(stat("f", &st) == 0, "stat failed");
    DIR *d = opendir(".");
    CHECK(d != NULL, "opendir failed");
    struct dirent *e;
    int found = 0;
    while ((e = readdir(d)) != NULL) {
        if (h_streq(e->d_name, "f")) {
            found |= 1;
            EXPECT_UINT("d_ino of f against st_ino", e->d_ino, st.st_ino);
            EXPECT_INT("d_type of a regular file", e->d_type, DT_REG);
        } else if (h_streq(e->d_name, "d")) {
            found |= 2;
            EXPECT_INT("d_type of a directory", e->d_type, DT_DIR);
        }
    }
    EXPECT_INT("entries found (bit 1 f, bit 2 d)", found, 3);
    return PASS;
}

static const char *readdir_empty(void) {
    int seen;
    CHECK(mkdir("empty", 0755) == 0, "mkdir failed");
    DIR *d = opendir("empty");
    CHECK(d != NULL, "opendir failed");
    EXPECT_INT("entries in an empty directory", scan(d, ".", &seen), 2);
    EXPECT_INT("times \".\" appeared", seen, 1);
    return PASS;
}

static const char *readdir_many(void) {
    char name[16];
    static unsigned char seen[300];
    for (int i = 0; i < 300; i++) {
        int n = 0;
        name[n++] = 'f';
        name[n++] = (char)('0' + i / 100);
        name[n++] = (char)('0' + i / 10 % 10);
        name[n++] = (char)('0' + i % 10);
        for (int k = 0; k < 9; k++)
            name[n++] = 'x';
        name[n] = 0;
        CHECK(make(name) == 0, "could not create a file");
    }
    DIR *d = opendir(".");
    CHECK(d != NULL, "opendir failed");
    struct dirent *e;
    long total = 0;
    while ((e = readdir(d)) != NULL) {
        total++;
        if (e->d_name[0] == 'f') {
            int i = (e->d_name[1] - '0') * 100 + (e->d_name[2] - '0') * 10 + (e->d_name[3] - '0');
            if (i < 0 || i >= 300 || seen[i]++)
                return fail_str("an entry appeared twice or was not created", e->d_name, "f<000-299>xxxxxxxxx");
        }
    }
    EXPECT_INT("entries readdir returned for 300 files", total, 302);
    return PASS;
}

static const char *rewinddir_rescan(void) {
    int seen;
    CHECK(make("a") == 0, "could not create the file");
    DIR *d = opendir(".");
    CHECK(d != NULL, "opendir failed");
    EXPECT_INT("first pass", scan(d, "late", &seen), 3);
    CHECK(make("late") == 0, "could not create the file");
    rewinddir(d);
    EXPECT_INT("pass after rewinddir", scan(d, "late", &seen), 4);
    EXPECT_INT("times a file created before rewinddir appeared", seen, 1);
    return PASS;
}

static const char *telldir_seekdir(void) {
    char want[256];
    CHECK(make("a") == 0 && make("b") == 0 && make("c") == 0 && make("d") == 0, "could not create the files");
    DIR *d = opendir(".");
    CHECK(d != NULL, "opendir failed");
    readdir(d);
    readdir(d);
    long pos = telldir(d);
    CHECK(pos != -1, "telldir failed");
    struct dirent *e = readdir(d);
    CHECK(e != NULL, "readdir after telldir returned NULL");
    size_t n = 0;
    for (; e->d_name[n] && n < sizeof want - 1; n++)
        want[n] = e->d_name[n];
    want[n] = 0;
    while (readdir(d) != NULL)
        ;
    seekdir(d, pos);
    e = readdir(d);
    CHECK(e != NULL, "readdir after seekdir returned NULL");
    EXPECT_STR("entry after seekdir to a telldir position", e->d_name, want);
    return PASS;
}

static const char *dirfd_fdopendir(void) {
    struct stat st;
    int seen;
    CHECK(make("x") == 0, "could not create the file");
    DIR *d = opendir(".");
    CHECK(d != NULL, "opendir failed");
    int fd = dirfd(d);
    CHECK(fd >= 0, "dirfd failed");
    CHECK(fstat(fd, &st) == 0 && S_ISDIR(st.st_mode), "dirfd is not a directory descriptor");
    closedir(d);
    fd = open(".", O_RDONLY | O_DIRECTORY);
    CHECK(fd >= 0, "open(\".\", O_DIRECTORY) failed");
    DIR *e = fdopendir(fd);
    CHECK(e != NULL, "fdopendir failed");
    EXPECT_INT("entries through fdopendir", scan(e, "x", &seen), 3);
    int ffd = open("x", O_RDONLY);
    errno = 0;
    CHECK(fdopendir(ffd) == NULL, "fdopendir of a regular file returned a stream");
    EXPECT_ERRNO("fdopendir of a regular file", ENOTDIR);
    return PASS;
}

static int not_dot(const struct dirent *e) {
    return e->d_name[0] != '.';
}

static const char *scandir_alphasort(void) {
    struct dirent **list = NULL;
    CHECK(make("pear") == 0 && make("apple") == 0 && make("fig") == 0 && make(".hid") == 0, "could not create the files");
    int n = scandir(".", &list, not_dot, alphasort);
    EXPECT_INT("scandir with a filter", n, 3);
    EXPECT_STR("first", list[0]->d_name, "apple");
    EXPECT_STR("second", list[1]->d_name, "fig");
    EXPECT_STR("third", list[2]->d_name, "pear");
    for (int i = 0; i < n; i++)
        free(list[i]);
    free(list);
    return PASS;
}

static const char *glob_star(void) {
    glob_t g;
    CHECK(make("a.c") == 0 && make("b.c") == 0 && make("c.h") == 0, "could not create the files");
    EXPECT_INT("glob(\"*.c\")", glob("*.c", 0, NULL, &g), 0);
    EXPECT_UINT("gl_pathc", g.gl_pathc, 2);
    EXPECT_STR("first match", g.gl_pathv[0], "a.c");
    EXPECT_STR("second match", g.gl_pathv[1], "b.c");
    EXPECT_PTR("gl_pathv terminator", g.gl_pathv[2], NULL);
    globfree(&g);
    return PASS;
}

static const char *glob_brackets(void) {
    glob_t g;
    CHECK(make("a1") == 0 && make("b2") == 0 && make("c3") == 0 && make("ab") == 0, "could not create the files");
    EXPECT_INT("glob(\"?[0-9]\")", glob("?[0-9]", 0, NULL, &g), 0);
    EXPECT_UINT("matches of ?[0-9]", g.gl_pathc, 3);
    globfree(&g);
    EXPECT_INT("glob(\"[!a]*\")", glob("[!a]*", 0, NULL, &g), 0);
    EXPECT_UINT("matches of [!a]*", g.gl_pathc, 2);
    EXPECT_STR("first match of [!a]*", g.gl_pathv[0], "b2");
    globfree(&g);
    return PASS;
}

static const char *glob_nomatch(void) {
    glob_t g;
    EXPECT_INT("glob of a pattern with no match", glob("*.zzz", 0, NULL, &g), GLOB_NOMATCH);
    EXPECT_INT("glob with GLOB_NOCHECK", glob("*.zzz", GLOB_NOCHECK, NULL, &g), 0);
    EXPECT_UINT("gl_pathc with GLOB_NOCHECK", g.gl_pathc, 1);
    EXPECT_STR("GLOB_NOCHECK returns the pattern", g.gl_pathv[0], "*.zzz");
    globfree(&g);
    return PASS;
}

static const char *glob_hidden(void) {
    glob_t g;
    CHECK(make(".hidden") == 0 && make("shown") == 0, "could not create the files");
    EXPECT_INT("glob(\"*\")", glob("*", 0, NULL, &g), 0);
    EXPECT_UINT("matches of *", g.gl_pathc, 1);
    EXPECT_STR("match of *", g.gl_pathv[0], "shown");
    globfree(&g);
    EXPECT_INT("glob(\".h*\")", glob(".h*", 0, NULL, &g), 0);
    EXPECT_STR("match of .h*", g.gl_pathv[0], ".hidden");
    globfree(&g);
    return PASS;
}

static const char *glob_mark(void) {
    glob_t g;
    CHECK(mkdir("dir", 0755) == 0 && make("file") == 0, "could not create the entries");
    EXPECT_INT("glob(\"*\", GLOB_MARK)", glob("*", GLOB_MARK, NULL, &g), 0);
    EXPECT_UINT("gl_pathc", g.gl_pathc, 2);
    EXPECT_STR("directory with GLOB_MARK", g.gl_pathv[0], "dir/");
    EXPECT_STR("file with GLOB_MARK", g.gl_pathv[1], "file");
    globfree(&g);
    return PASS;
}

static const char *glob_append_offs(void) {
    glob_t g;
    CHECK(make("x.a") == 0 && make("y.b") == 0, "could not create the files");
    g.gl_offs = 2;
    EXPECT_INT("glob(\"*.a\", GLOB_DOOFFS)", glob("*.a", GLOB_DOOFFS, NULL, &g), 0);
    EXPECT_INT("glob(\"*.b\", GLOB_DOOFFS|GLOB_APPEND)", glob("*.b", GLOB_DOOFFS | GLOB_APPEND, NULL, &g), 0);
    EXPECT_UINT("gl_pathc after GLOB_APPEND", g.gl_pathc, 2);
    EXPECT_PTR("first reserved slot", g.gl_pathv[0], NULL);
    EXPECT_PTR("second reserved slot", g.gl_pathv[1], NULL);
    EXPECT_STR("first match", g.gl_pathv[2], "x.a");
    EXPECT_STR("appended match", g.gl_pathv[3], "y.b");
    EXPECT_PTR("terminator", g.gl_pathv[4], NULL);
    globfree(&g);
    return PASS;
}

static const char *glob_subdirs(void) {
    glob_t g;
    CHECK(mkdir("d1", 0755) == 0 && mkdir("d2", 0755) == 0 && mkdir("e1", 0755) == 0, "mkdir failed");
    CHECK(make("d1/xa") == 0 && make("d2/xb") == 0 && make("d2/yc") == 0 && make("e1/xd") == 0,
          "could not create the files");
    EXPECT_INT("glob(\"d*/x*\")", glob("d*/x*", 0, NULL, &g), 0);
    EXPECT_UINT("matches of d*/x*", g.gl_pathc, 2);
    EXPECT_STR("first match", g.gl_pathv[0], "d1/xa");
    EXPECT_STR("second match", g.gl_pathv[1], "d2/xb");
    globfree(&g);
    return PASS;
}

static const char *glob_escape(void) {
    glob_t g;
    CHECK(make("*") == 0 && make("ab") == 0, "could not create the files");
    EXPECT_INT("glob(\"\\\\*\")", glob("\\*", 0, NULL, &g), 0);
    EXPECT_UINT("matches of an escaped *", g.gl_pathc, 1);
    EXPECT_STR("match of an escaped *", g.gl_pathv[0], "*");
    globfree(&g);
    EXPECT_INT("glob(\"\\\\*\", GLOB_NOESCAPE)", glob("\\*", GLOB_NOESCAPE, NULL, &g), GLOB_NOMATCH);
    return PASS;
}

static const char *fnmatch_basic(void) {
    EXPECT_INT("fnmatch(\"*.c\", \"a.c\")", fnmatch("*.c", "a.c", 0), 0);
    EXPECT_INT("fnmatch(\"*.c\", \"a.h\")", fnmatch("*.c", "a.h", 0), FNM_NOMATCH);
    EXPECT_INT("fnmatch(\"a?c\", \"abc\")", fnmatch("a?c", "abc", 0), 0);
    EXPECT_INT("fnmatch(\"a?c\", \"ac\")", fnmatch("a?c", "ac", 0), FNM_NOMATCH);
    EXPECT_INT("fnmatch(\"*\", \"\")", fnmatch("*", "", 0), 0);
    return PASS;
}

static const char *fnmatch_pathname(void) {
    EXPECT_INT("fnmatch(\"*\", \"a/b\") without FNM_PATHNAME", fnmatch("*", "a/b", 0), 0);
    EXPECT_INT("fnmatch(\"*\", \"a/b\", FNM_PATHNAME)", fnmatch("*", "a/b", FNM_PATHNAME), FNM_NOMATCH);
    EXPECT_INT("fnmatch(\"a/*\", \"a/b\", FNM_PATHNAME)", fnmatch("a/*", "a/b", FNM_PATHNAME), 0);
    EXPECT_INT("fnmatch(\"a?b\", \"a/b\", FNM_PATHNAME)", fnmatch("a?b", "a/b", FNM_PATHNAME), FNM_NOMATCH);
    EXPECT_INT("fnmatch(\"a[/]b\", \"a/b\", FNM_PATHNAME)", fnmatch("a[/]b", "a/b", FNM_PATHNAME), FNM_NOMATCH);
    return PASS;
}

static const char *fnmatch_period(void) {
    EXPECT_INT("fnmatch(\"*\", \".x\") without FNM_PERIOD", fnmatch("*", ".x", 0), 0);
    EXPECT_INT("fnmatch(\"*\", \".x\", FNM_PERIOD)", fnmatch("*", ".x", FNM_PERIOD), FNM_NOMATCH);
    EXPECT_INT("fnmatch(\"?x\", \".x\", FNM_PERIOD)", fnmatch("?x", ".x", FNM_PERIOD), FNM_NOMATCH);
    EXPECT_INT("fnmatch(\".*\", \".x\", FNM_PERIOD)", fnmatch(".*", ".x", FNM_PERIOD), 0);
    EXPECT_INT("fnmatch(\"a/*\", \"a/.x\", FNM_PATHNAME|FNM_PERIOD)", fnmatch("a/*", "a/.x", FNM_PATHNAME | FNM_PERIOD),
               FNM_NOMATCH);
    EXPECT_INT("fnmatch(\"a*\", \"a.x\", FNM_PERIOD)", fnmatch("a*", "a.x", FNM_PERIOD), 0);
    return PASS;
}

static const char *fnmatch_escape(void) {
    EXPECT_INT("fnmatch(\"\\\\*\", \"*\")", fnmatch("\\*", "*", 0), 0);
    EXPECT_INT("fnmatch(\"\\\\*\", \"x\")", fnmatch("\\*", "x", 0), FNM_NOMATCH);
    EXPECT_INT("fnmatch(\"\\\\*\", \"\\\\x\", FNM_NOESCAPE)", fnmatch("\\*", "\\x", FNM_NOESCAPE), 0);
    EXPECT_INT("fnmatch(\"\\\\*\", \"*\", FNM_NOESCAPE)", fnmatch("\\*", "*", FNM_NOESCAPE), FNM_NOMATCH);
    return PASS;
}

static const char *fnmatch_brackets(void) {
    EXPECT_INT("fnmatch(\"[a-c]x\", \"bx\")", fnmatch("[a-c]x", "bx", 0), 0);
    EXPECT_INT("fnmatch(\"[!a-c]x\", \"bx\")", fnmatch("[!a-c]x", "bx", 0), FNM_NOMATCH);
    EXPECT_INT("fnmatch(\"[!a-c]x\", \"dx\")", fnmatch("[!a-c]x", "dx", 0), 0);
    EXPECT_INT("fnmatch(\"[]]\", \"]\")", fnmatch("[]]", "]", 0), 0);
    EXPECT_INT("fnmatch(\"[a-]\", \"-\")", fnmatch("[a-]", "-", 0), 0);
    EXPECT_INT("fnmatch(\"[[:digit:]]\", \"5\")", fnmatch("[[:digit:]]", "5", 0), 0);
    EXPECT_INT("fnmatch(\"[[:alpha:]]\", \"5\")", fnmatch("[[:alpha:]]", "5", 0), FNM_NOMATCH);
    EXPECT_INT("fnmatch(\"[[:upper:][:digit:]]x\", \"Qx\")", fnmatch("[[:upper:][:digit:]]x", "Qx", 0), 0);
    return PASS;
}

static const char *fnmatch_casefold(void) {
    EXPECT_INT("fnmatch(\"*.TXT\", \"a.txt\")", fnmatch("*.TXT", "a.txt", 0), FNM_NOMATCH);
    EXPECT_INT("fnmatch(\"*.TXT\", \"a.txt\", FNM_CASEFOLD)", fnmatch("*.TXT", "a.txt", FNM_CASEFOLD), 0);
    EXPECT_INT("fnmatch(\"[A-C]\", \"b\", FNM_CASEFOLD)", fnmatch("[A-C]", "b", FNM_CASEFOLD), 0);
    return PASS;
}

const struct libc_case libc_cases[] = {
    { "opendir-errors", opendir_errors },
    { "readdir-lists", readdir_lists },
    { "readdir-ino-type", readdir_ino_type },
    { "readdir-empty", readdir_empty },
    { "readdir-many", readdir_many },
    { "rewinddir-rescan", rewinddir_rescan },
    { "telldir-seekdir", telldir_seekdir },
    { "dirfd-fdopendir", dirfd_fdopendir },
    { "scandir-alphasort", scandir_alphasort },
    { "glob-star", glob_star },
    { "glob-brackets", glob_brackets },
    { "glob-nomatch", glob_nomatch },
    { "glob-hidden", glob_hidden },
    { "glob-mark", glob_mark },
    { "glob-append-offs", glob_append_offs },
    { "glob-subdirs", glob_subdirs },
    { "glob-escape", glob_escape },
    { "fnmatch-basic", fnmatch_basic },
    { "fnmatch-pathname", fnmatch_pathname },
    { "fnmatch-period", fnmatch_period },
    { "fnmatch-escape", fnmatch_escape },
    { "fnmatch-brackets", fnmatch_brackets },
    { "fnmatch-casefold", fnmatch_casefold },
    { 0, 0 },
};
