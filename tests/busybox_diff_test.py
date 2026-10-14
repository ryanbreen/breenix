#!/usr/bin/env python3
"""Exercise the patched BusyBox hunk engine and writers with native host stdio.

This does not substitute for the ARM64 utilities gate: it isolates the code
changed by diff-normal-format.patch, without building any other applet or libc.
"""
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SHIM = r"""
#include <ctype.h>
#include <limits.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <time.h>
#include <errno.h>
#include <fcntl.h>
#include <unistd.h>
#define LONE_DASH(s) ((s)[0] == '-' && !(s)[1])
#define COMMON_BUFSIZE 4096
static char bb_common_bufsiz1[COMMON_BUFSIZE];
static void setup_common_bufsiz(void) {}
static int open_or_warn(const char *s, int flags) {
    int fd = open(s, flags); if (fd < 0) perror(s); return fd;
}
static int xopen(const char *s, int flags) { int fd = open_or_warn(s, flags); if (fd < 0) exit(2); return fd; }
static int xmkstemp(char *s) { int fd = mkstemp(s); if (fd < 0) exit(2); return fd; }
static void xlseek(int fd, off_t off, int whence) { if (lseek(fd, off, whence) < 0) exit(2); }
static void xfunc_die(void) { exit(2); }
static void bb_simple_perror_msg(const char *s) { perror(s); }
static void bb_simple_error_msg(const char *s) { fprintf(stderr, "%s\n", s); }
static void fclose_if_not_stdin(FILE *fp) { if (fp != stdin) fclose(fp); }
static int bb_copyfd_eof(int src, int dst) {
    char buf[4096]; ssize_t n;
    while ((n = read(src, buf, sizeof(buf))) > 0) if (write(dst, buf, n) != n) return -1;
    return n < 0 ? -1 : 0;
}
static void bb_simple_error_msg_and_die(const char *s) { fputs(s, stderr); exit(2); }
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
"""
MAIN = r"""
int main(int argc, char **argv) {
    if (argc != 5) return 2;
    INIT_G();
    char *directory_options[] = {"diff", "-r"};
    for (const char *p = argv[3]; *p; p++) {
        switch (*p) {
        case 'D': G.directory = true; G.options = directory_options; G.option_count = 2; break;
        case 'R': break;
        case 's': option_mask32 |= FLAG(s); break;
        case 'c': option_mask32 |= FLAG(c); break;
        case 'C': option_mask32 |= FLAG(C); break;
        case 'e': option_mask32 |= FLAG(e); break;
        case 'f': option_mask32 |= FLAG(f); break;
        case 'u': option_mask32 |= FLAG(u); break;
        case 'U': option_mask32 |= FLAG(U); break;
        case 'q': option_mask32 |= FLAG(q); break;
        case 'B': option_mask32 |= FLAG(B); break;
        case 'b': option_mask32 |= FLAG(b); break;
        case 'w': option_mask32 |= FLAG(w); break;
        case 'i': option_mask32 |= FLAG(i); break;
        case 't': option_mask32 |= FLAG(t); break;
        case 'T': option_mask32 |= FLAG(T); break;
        default: return 2;
        }
    }
    opt_U_context = atoi(argv[4]);
    if (strchr(argv[3], 'R')) {
        char *files[2] = {argv[1], argv[2]};
        print_status(diffreg(files), files);
        return exit_status;
    }
    FILE *fp[2] = {fopen(argv[1], "r"), fopen(argv[2], "r")};
    char *names[2] = {"a", "b"};
    if (!fp[0] || !fp[1]) return 2;
    int status = diff(fp, names);
    fclose(fp[0]);
    fclose(fp[1]);
    free(ptr_to_globals);
    return status;
}
"""


class NormalDiff(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        os.environ["TZ"] = "UTC0"
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
        # Compile BusyBox's actual match/hunk computation and both output paths.
        engine = source[source.index("enum {                  /* print_status()"):source.index("#if ENABLE_FEATURE_DIFF_DIR\nstruct dlist")]
        (cls.work / "test.c").write_text(SHIM + engine + MAIN)
        cls.binary = cls.work / "diff-test"
        subprocess.run([os.environ.get("CC", "cc"), "-std=gnu11", "-Werror",
                        str(cls.work / "test.c"), "-o", str(cls.binary)], check=True)

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def check_diff(self, old, new, expected, options="", context=3, status=1):
        (self.work / "a").write_text(old)
        (self.work / "b").write_text(new)
        result = subprocess.run([str(self.binary), str(self.work / "a"), str(self.work / "b"),
                                 options, str(context)], capture_output=True, text=True)
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
        self.check_diff("a\n", "b\n", "", "q")
        self.check_diff("a\n", "A\n", "", "i", status=0)
        self.check_diff("a b\n", "a  b\n", "", "b", status=0)
        self.check_diff("a b\n", "ab\n", "", "w", status=0)
        self.check_diff("a\n\nb\nc\n", "a\nb\nx\n", "4c3\n< c\n---\n> x\n", "B")

    def test_directory_pair_is_identified_once(self):
        self.check_diff("a\nb\nc\n", "x\nb\ny\n",
                        "diff -r a b\n1c1\n< a\n---\n> x\n3c3\n< c\n---\n> y\n", "D")
        self.check_diff("a\n", "a\n", "", "D", status=0)
        self.check_diff("a\n", "b\n", "", "Dq")

    def test_io_errors_are_not_identical(self):
        (self.work / "b").write_text("b\n")
        for path in [self.work / "absent", self.work]:
            result = subprocess.run([str(self.binary), str(path), str(self.work / "b"), "Rs", "3"],
                                    capture_output=True, text=True)
            self.assertGreater(result.returncode, 1)
            self.assertNotIn("identical", result.stdout)
            self.assertIn("comparison failed", result.stderr)
        locked = self.work / "locked"
        locked.write_text("b\n")
        locked.chmod(0)
        try:
            result = subprocess.run([str(self.binary), str(locked), str(self.work / "b"), "Rs", "3"],
                                    capture_output=True, text=True)
            self.assertGreater(result.returncode, 1)
            self.assertNotIn("identical", result.stdout)
        finally:
            locked.chmod(0o600)

    def test_editor_scripts(self):
        self.check_diff("a\nb\nc\n", "x\nb\ny\n", "3c\ny\n.\n1c\nx\n.\n", "e")
        self.check_diff("a\nb\nc\n", "x\nb\ny\n", "c1\nx\n.\nc3\ny\n.\n", "f")
        self.check_diff("a\n", ".\na\n", "0a\n..\n.\n1s/^\\.\\.$/./\n", "e")
        self.check_diff("a\nx\ny\n", "a\n", "2,3d\n", "e")
        self.check_diff("a\nx\ny\n", "a\n", "d2 3\n", "f")

    def test_context_format(self):
        header = "*** a Thu Jan  1 00:00:00 1970\n--- b Thu Jan  1 00:00:00 1970\n"
        self.check_diff("a\nb\nc\n", "a\nx\nc\n", header +
                        "***************\n*** 1,3 ****\n  a\n! b\n  c\n--- 1,3 ----\n  a\n! x\n  c\n", "c")
        self.check_diff("a\nb\nc\n", "a\nx\nc\n", header +
                        "***************\n*** 2 ****\n! b\n--- 2 ----\n! x\n", "C", context=0)
        self.check_diff("", "x\n", header +
                        "***************\n*** 0 ****\n--- 1 ----\n+ x\n", "c")
        self.check_diff("x\n", "", header +
                        "***************\n*** 1 ****\n- x\n--- 0 ----\n", "c")

    def test_large_context_does_not_overflow(self):
        header = "*** a Thu Jan  1 00:00:00 1970\n--- b Thu Jan  1 00:00:00 1970\n"
        self.check_diff("a\nb\nc\n", "a\nx\nc\n", header +
                        "***************\n*** 1,3 ****\n  a\n! b\n  c\n--- 1,3 ----\n  a\n! x\n  c\n",
                        "C", context=2147483647)

    def test_incomplete_lines_and_tabs(self):
        self.check_diff("a", "b", "1c1\n< a\n\\ No newline at end of file\n---\n> b\n\\ No newline at end of file\n")
        self.check_diff("\tx\n", "y\n", "1c1\n<         x\n---\n> y\n", "t")
        self.check_diff("x\n", "y\n", "1c1\n<\tx\n---\n>\ty\n", "T")


if __name__ == "__main__":
    unittest.main()
