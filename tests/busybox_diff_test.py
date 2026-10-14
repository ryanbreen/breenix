#!/usr/bin/env python3
"""Exercise the patched BusyBox hunk engine and writers with native host stdio.

This does not substitute for the ARM64 utilities gate: it isolates the code
changed by diff-normal-format.patch, without building any other applet or libc.
"""
from datetime import datetime, timezone
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SHIM = r"""
#include <ctype.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdarg.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <time.h>
#include <unistd.h>
#define MIN(a,b) ((a) < (b) ? (a) : (b))
#define MAX(a,b) ((a) > (b) ? (a) : (b))
#define NOINLINE __attribute__((noinline))
typedef unsigned char smallint;
static unsigned option_mask32;
static struct globals *ptr_to_globals;
#define SET_PTR_TO_GLOBALS(p) (ptr_to_globals = (p))
static void *xmalloc(size_t n) {
    void *p = malloc(n ? n : 1);
    if (!p) exit(2);
    return p;
}
static void *xzalloc(size_t n) {
    void *p = xmalloc(n);
    memset(p, 0, n);
    return p;
}
static void *xrealloc(void *p, size_t n) {
    p = realloc(p, n ? n : 1);
    if (!p) exit(2);
    return p;
}
#define xrealloc_vector(p,shift,i) \
    xrealloc((p), (((i) / (1 << (shift)) + 1) << (shift)) * sizeof(*(p)))
static unsigned isqrt(unsigned n) {
    unsigned r = 0;
    while (r + 1 <= n / (r + 1)) r++;
    return r;
}
#define LONE_DASH(s) ((s)[0] == '-' && !(s)[1])
#define COMMON_BUFSIZE 1024
static char bb_common_bufsiz1[COMMON_BUFSIZE];
#define setup_common_bufsiz() ((void)0)
static const char bb_msg_standard_output[] = "standard output";
static void bb_perror_msg(const char *fmt, ...) {
    int e = errno;
    va_list ap;
    fputs("diff: ", stderr);
    va_start(ap, fmt);
    vfprintf(stderr, fmt, ap);
    va_end(ap);
    fprintf(stderr, ": %s\n", strerror(e));
}
static void bb_simple_perror_msg(const char *s) { bb_perror_msg("%s", s); }
static int open_or_warn(const char *name, int flags) {
    int fd = open(name, flags);
    if (fd < 0) bb_perror_msg("can't open '%s'", name);
    return fd;
}
static int xopen(const char *name, int flags) {
    int fd = open(name, flags);
    if (fd < 0) exit(2);
    return fd;
}
static int xmkstemp(char *name) {
    int fd = mkstemp(name);
    if (fd < 0) exit(2);
    return fd;
}
static off_t bb_copyfd_eof(int from, int to) {
    char buf[4096];
    ssize_t n;
    off_t total = 0;
    while ((n = read(from, buf, sizeof(buf))) > 0) {
        if (write(to, buf, n) != n) return -1;
        total += n;
    }
    return n < 0 ? -1 : total;
}
static void xfunc_die(void) { exit(2); }
static void xlseek(int fd, off_t offset, int whence) {
    if (lseek(fd, offset, whence) < 0) exit(2);
}
static FILE *xfdopen_for_read(int fd) {
    FILE *fp = fdopen(fd, "r");
    if (!fp) exit(2);
    return fp;
}
static void fclose_if_not_stdin(FILE *fp) {
    if (fp != stdin) fclose(fp);
}
"""
MAIN = r"""
int main(int argc, char **argv) {
    if (argc != 5) return 2;
    INIT_G();
    for (const char *p = argv[3]; *p; p++) {
        switch (*p) {
        case 'u': option_mask32 |= FLAG(u); break;
        case 'U': option_mask32 |= FLAG(U); break;
        case 'c': option_mask32 |= FLAG(c); break;
        case 'C': option_mask32 |= FLAG(C); break;
        case 'e': option_mask32 |= FLAG(e); break;
        case 'f': option_mask32 |= FLAG(f); break;
        case 'q': option_mask32 |= FLAG(q); break;
        case 's': option_mask32 |= FLAG(s); break;
        case 'B': option_mask32 |= FLAG(B); break;
        case 'b': option_mask32 |= FLAG(b); break;
        case 'w': option_mask32 |= FLAG(w); break;
        case 'i': option_mask32 |= FLAG(i); break;
        case 't': option_mask32 |= FLAG(t); break;
        case 'T': option_mask32 |= FLAG(T); break;
        case 'D': G.dir_opts = " -r"; break; /* as diffdir sets it */
        default: return 2;
        }
    }
    opt_U_context = atoi(argv[4]);
    char *names[2] = {argv[1], argv[2]};
    for (int i = 0; i < 2; i++)
        stat(names[i], &stb[i]);
    print_status(diffreg(names), names);
    return output_status();
}
"""


class NormalDiff(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix="breenix-diff-")
        cls.work = Path(cls.tmp.name)
        with tarfile.open(ROOT / "vendor/busybox/busybox-1.37.0.tar.bz2") as archive:
            source = archive.extractfile("busybox-1.37.0/editors/diff.c").read()
        editor = cls.work / "editors"
        editor.mkdir()
        (editor / "diff.c").write_bytes(source)
        subprocess.run(
            ["patch", "-p1", "-i", str(ROOT / "vendor/busybox/patches/diff-normal-format.patch")],
            cwd=cls.work, check=True, capture_output=True,
        )
        source = (editor / "diff.c").read_text()
        # Compile BusyBox's actual match/hunk computation, every output format,
        # diffreg's input handling and the final output check.
        engine = source[source.index("enum {                  /* print_status()"):source.index("#if ENABLE_FEATURE_DIFF_DIR\nstruct dlist")]
        (cls.work / "test.c").write_text(SHIM + engine + MAIN)
        cls.binary = cls.work / "diff-test"
        subprocess.run([os.environ.get("CC", "cc"), "-std=gnu11", "-Werror",
                        str(cls.work / "test.c"), "-o", str(cls.binary)], check=True)

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def run_diff(self, options="", context=3, first="a", second="b", **kwargs):
        return subprocess.run([str(self.binary), first, second, options, str(context)], cwd=self.work,
                              env={**os.environ, "TZ": "UTC"}, **kwargs)

    def check_diff(self, old, new, expected, options="", context=3, status=1):
        (self.work / "a").write_text(old)
        (self.work / "b").write_text(new)
        result = self.run_diff(options, context, capture_output=True, text=True)
        self.assertEqual((result.returncode, result.stdout, result.stderr), (status, expected, ""))

    def test_normal_commands(self):
        cases = [
            ("1\n2\n3\n", "1\nx\n3\n", "2c2\n< 2\n---\n> x\n"),
            ("", "x\n", "0a1\n> x\n"),
            ("", "x\ny\n", "0a1,2\n> x\n> y\n"),
            ("x\n", "", "1d0\n< x\n"),
            ("x\ny\n", "", "1,2d0\n< x\n< y\n"),
            ("a\n", "x\na\n", "0a1\n> x\n"),
            ("a\n", "a\nx\ny\n", "1a2,3\n> x\n> y\n"),
            ("a\nx\nb\n", "a\nb\n", "2d1\n< x\n"),
            ("a\nx\ny\n", "a\n", "2,3d1\n< x\n< y\n"),
            ("a\nx\ny\nz\n", "a\nw\nz\n", "2,3c2\n< x\n< y\n---\n> w\n"),
            ("a\nx\nz\n", "a\ny\nw\nz\n", "2c2,3\n< x\n---\n> y\n> w\n"),
            ("x\ny\n", "u\nv\n", "1,2c1,2\n< x\n< y\n---\n> u\n> v\n"),
            ("a\nb\nc\n", "x\nb\ny\n", "1c1\n< a\n---\n> x\n3c3\n< c\n---\n> y\n"),
        ]
        for old, new, expected in cases:
            with self.subTest(old=old, new=new):
                self.check_diff(old, new, expected)

    def test_identical_and_empty(self):
        self.check_diff("a\n", "a\n", "", status=0)
        self.check_diff("", "", "", status=0)

    def test_unified_stays_unchanged(self):
        expected = "--- a\n+++ b\n@@ -1,3 +1,3 @@\n 1\n-2\n+x\n 3\n"
        self.check_diff("1\n2\n3\n", "1\nx\n3\n", expected, "u")
        self.check_diff("1\n2\n3\n", "1\nx\n3\n", expected, "U")
        self.check_diff("1\n2\n3\n", "1\nx\n3\n",
                        "--- a\n+++ b\n@@ -2 +2 @@\n-2\n+x\n", "U", context=0)

    def test_quiet_and_ignore_options(self):
        self.check_diff("a\n", "b\n", "Files a and b differ\n", "q")
        self.check_diff("a\n", "A\n", "", "i", status=0)
        self.check_diff("a b\n", "a  b\n", "", "b", status=0)
        self.check_diff("a b\n", "ab\n", "", "w", status=0)
        self.check_diff("a\n\nb\nc\n", "a\nb\nx\n", "4c3\n< c\n---\n> x\n", "B")

    def test_incomplete_lines_and_tabs(self):
        self.check_diff("a", "b", "1c1\n< a\n\\ No newline at end of file\n---\n> b\n\\ No newline at end of file\n")
        self.check_diff("\tx\n", "y\n", "1c1\n<         x\n---\n> y\n", "t")
        self.check_diff("x\n", "y\n", "1c1\n<\tx\n---\n>\ty\n", "T")

    def test_context_format(self):
        when = int(datetime(2026, 10, 14, 12, tzinfo=timezone.utc).timestamp())
        def check(old, new, expected, options="c", context=3):
            (self.work / "a").write_text(old)
            (self.work / "b").write_text(new)
            os.utime(self.work / "a", (when, when))
            os.utime(self.work / "b", (when + 1, when + 1))
            header = "*** a Wed Oct 14 12:00:00 2026\n--- b Wed Oct 14 12:00:01 2026\n"
            result = self.run_diff(options, context, capture_output=True, text=True)
            self.assertEqual((result.returncode, result.stdout, result.stderr), (1, header + expected, ""))
        check("1\n2\n3\n", "1\nx\n3\n",
              "***************\n*** 1,3 ****\n  1\n! 2\n  3\n--- 1,3 ----\n  1\n! x\n  3\n")
        check("1\n2\n3\n", "1\n3\n",
              "***************\n*** 1,3 ****\n  1\n- 2\n  3\n--- 1,2 ----\n")
        check("1\n3\n", "1\n2\n3\n",
              "***************\n*** 1,2 ****\n--- 1,3 ----\n  1\n+ 2\n  3\n")
        check("", "x\n", "***************\n*** 0 ****\n--- 1 ----\n+ x\n")
        lines = "".join(f"{n}\n" for n in range(1, 11))
        check(lines, lines.replace("2\n", "two\n", 1).replace("9\n", "nine\n"),
              "***************\n*** 2 ****\n! 2\n--- 2 ----\n! two\n"
              "***************\n*** 9 ****\n! 9\n--- 9 ----\n! nine\n", "C", 0)
        check(lines, lines.replace("5\n", "", 1),
              "***************\n*** 4,6 ****\n  4\n- 5\n  6\n--- 4,5 ----\n", "C", 1)

    def test_ed_scripts(self):
        old, new = "a\nb\nc\nd\n", "a\nB\nc\nd\ne\n"
        self.check_diff(old, new, "4a\ne\n.\n2c\nB\n.\n", "e")
        self.check_diff(old, new, "c2\nB\n.\na4\ne\n.\n", "f")
        self.check_diff("a\nb\nc\n", "a\n", "2,3d\n", "e")
        self.check_diff("a\nb\nc\n", "a\n", "d2 3\n", "f")
        # A lone "." in inserted text must not end ed's input mode.
        self.check_diff("x\n", "x\n.\ny\n", "1a\n..\n.\ns/.//\na\ny\n.\n", "e")
        self.check_diff("x\n", "x\n.\n", "1a\n..\n.\ns/.//\n", "e")

    def test_ed_script_rebuilds_the_new_file(self):
        if not shutil.which("ed"):
            self.skipTest("no ed on this host")
        old = "".join(f"{n}\n" for n in range(1, 30))
        new = old.replace("3\n", ".\nthree\n", 1).replace("17\n", "", 1) + ".\nend\n"
        (self.work / "a").write_text(old)
        (self.work / "b").write_text(new)
        script = self.run_diff("e", capture_output=True, text=True).stdout
        subprocess.run(["ed", "-s", str(self.work / "a")], input=script + "w\nq\n", text=True, check=True)
        self.assertEqual((self.work / "a").read_text(), new)

    def test_directory_pair_line(self):
        self.check_diff("1\n", "2\n", "diff -r a b\n1c1\n< 1\n---\n> 2\n", "D")
        self.check_diff("1\n", "2\n", "Files a and b differ\n", "Dq")

    def test_unreadable_input_is_an_error(self):
        (self.work / "a").write_text("x\n")
        missing = self.run_diff("s", second="missing", capture_output=True, text=True)
        self.assertEqual((missing.returncode, missing.stdout), (2, ""))
        self.assertIn("missing", missing.stderr)
        (self.work / "dir").mkdir(exist_ok=True)
        directory = self.run_diff("", second="dir", capture_output=True, text=True)
        self.assertEqual((directory.returncode, directory.stdout), (2, ""))
        self.assertIn("dir", directory.stderr)

    def test_unwritten_output_is_an_error(self):
        (self.work / "a").write_text("1\n")
        (self.work / "b").write_text("2\n")
        read, write = os.pipe()
        os.close(read)
        # Python ignores SIGPIPE; the child inherits that, so the write fails with EPIPE.
        result = self.run_diff(stdout=write, stderr=subprocess.PIPE, text=True, restore_signals=False)
        os.close(write)
        self.assertEqual(result.returncode, 2)
        self.assertIn("standard output", result.stderr)


if __name__ == "__main__":
    unittest.main()
