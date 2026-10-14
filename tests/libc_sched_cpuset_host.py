#!/usr/bin/env python3
"""Check sched.h's GNU CPU-set macros: strict-mode compiles for both targets, and their
bounds, single evaluation and set operations in a host program built on the same header."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
INCLUDE = ROOT / 'libs/libbreenix-libc/include'

PROGRAM = r'''
#define _GNU_SOURCE 1
#include <sched.h>

struct guarded { cpu_set_t set; unsigned long guard[4]; };

static int check(void)
{
    struct guarded g;
    cpu_set_t a, b, c;
    cpu_set_t *big;
    size_t big_size;
    int i = 0, k;

    __builtin_memset(&g, 0xa5, sizeof(g));
    CPU_ZERO(&g.set);
    if (CPU_COUNT(&g.set) != 0) return 1;
    CPU_SET(1024, &g.set);
    CPU_SET(-1, &g.set);
    CPU_SET(1 << 20, &g.set);
    for (k = 0; k < 4; k++)
        if (g.guard[k] != 0xa5a5a5a5a5a5a5a5UL) return 2;
    if (CPU_COUNT(&g.set) != 0 || CPU_ISSET(1024, &g.set) || CPU_ISSET(-1, &g.set)) return 3;
    CPU_CLR(4096, &g.set);
    for (k = 0; k < 4; k++)
        if (g.guard[k] != 0xa5a5a5a5a5a5a5a5UL) return 4;

    CPU_SET(i++, &g.set);
    if (i != 1 || !CPU_ISSET(0, &g.set)) return 5;
    if (CPU_ISSET(i++, &g.set) != 0 || i != 2) return 6;
    CPU_CLR(--i, &g.set);
    if (i != 1) return 7;
    CPU_SET(1023, &g.set);
    CPU_SET(63, &g.set);
    CPU_SET(64, &g.set);
    if (CPU_COUNT(&g.set) != 4 || CPU_ISSET(1023, &g.set) != 1) return 8;

    CPU_ZERO(&a); CPU_ZERO(&b);
    CPU_SET(1, &a); CPU_SET(2, &a); CPU_SET(2, &b); CPU_SET(700, &b);
    CPU_AND(&c, &a, &b);
    if (CPU_COUNT(&c) != 1 || !CPU_ISSET(2, &c)) return 9;
    CPU_OR(&c, &a, &b);
    if (CPU_COUNT(&c) != 3 || !CPU_ISSET(700, &c)) return 10;
    CPU_XOR(&c, &a, &b);
    if (CPU_COUNT(&c) != 2 || CPU_ISSET(2, &c)) return 11;
    if (CPU_EQUAL(&a, &b) || !CPU_EQUAL(&a, &a)) return 12;

    big_size = CPU_ALLOC_SIZE(2048);
    if (big_size != 256 || CPU_ALLOC_SIZE(1) != sizeof(unsigned long) || CPU_ALLOC_SIZE(65) != 16) return 13;
    big = CPU_ALLOC(2048);
    if (!big) return 14;
    CPU_ZERO_S(big_size, big);
    CPU_SET_S(2000, big_size, big);
    CPU_SET_S(2048, big_size, big);
    if (CPU_COUNT_S(big_size, big) != 1 || !CPU_ISSET_S(2000, big_size, big)) return 15;
    if (CPU_ISSET_S(2000, sizeof(cpu_set_t), big)) return 16;
    CPU_CLR_S(2000, big_size, big);
    if (CPU_COUNT_S(big_size, big) != 0) return 17;
    CPU_FREE(big);
    return 0;
}

#if __STDC_VERSION__ >= 201112L
_Static_assert(_Generic(CPU_ISSET(0, (cpu_set_t *)0), int: 1, default: 0), "CPU_ISSET is int");
_Static_assert(sizeof(cpu_set_t) == 128, "Linux's cpu_set_t size");
#endif

int main(void) { return check(); }
'''


class SchedCpusetTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='breenix-sched-')
        self.addCleanup(self.temp.cleanup)
        self.source = Path(self.temp.name) / 'cpuset.c'
        self.source.write_text(PROGRAM)

    def compile_target(self, arch, *flags):
        result = subprocess.run([str(ROOT / 'scripts/breenix-cc'), '--arch', arch, '-fsyntax-only',
                                 '-Wall', '-Wextra', '-Werror', *flags, str(self.source)],
                                text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_header_compiles_in_strict_modes_for_both_targets(self):
        for arch in ('aarch64', 'x86_64'):
            for flags in (('-std=c89', '-pedantic-errors'), ('-std=c11', '-pedantic-errors'),
                          ('-x', 'c++', '-std=c++17')):
                with self.subTest(arch=arch, flags=flags):
                    self.compile_target(arch, *flags)

    def test_macros_on_the_host(self):
        clang = subprocess.check_output([str(ROOT / 'scripts/breenix-cc'), '--arch', 'aarch64', '--tool-dir'],
                                        text=True).strip() + '/clang'
        resource = subprocess.check_output([clang, '-print-resource-dir'], text=True).strip()
        program = Path(self.temp.name) / 'cpuset'
        # Only sched.h's own headers come from Breenix; the host links malloc and free.
        subprocess.run([clang, '-std=c11', '-O1', '-Wall', '-Werror', '-nostdinc', '-isystem', str(INCLUDE),
                        '-isystem', resource + '/include', str(self.source), '-o', str(program)], check=True)
        self.assertEqual(subprocess.run([str(program)]).returncode, 0)


if __name__ == '__main__':
    unittest.main()
