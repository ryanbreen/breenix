#!/usr/bin/env python3
"""Compile this lane's headers in strict ISO C and in POSIX mode for one or both arches.

Strict C17: a program that includes only <wchar.h>, <wctype.h>, <ctype.h>,
<locale.h> and <time.h> may still define names that belong to other headers
(C17 7.1.3), so those headers must not declare them. POSIX mode: the types
POSIX.1-2024 adds to these headers are present.

  libc_locale_headers_host.py [--arch aarch64|x86_64]   (default: both)
"""
from pathlib import Path
import argparse
import subprocess
import tempfile

parser = argparse.ArgumentParser()
parser.add_argument('--arch', choices=('aarch64', 'x86_64'), action='append')
arches = parser.parse_args().arch or ['aarch64', 'x86_64']

root = Path(__file__).resolve().parents[1]
include = root / 'libs/libbreenix-libc/include'
resource = subprocess.run(['clang', '-print-resource-dir'], capture_output=True, text=True,
                          check=True).stdout.strip()

STRICT = r'''
#include <wchar.h>
#include <wctype.h>
#include <ctype.h>
#include <locale.h>
#include <time.h>
typedef int int8_t; typedef int FILE; typedef int va_list; typedef int locale_t;
typedef int ptrdiff_t; typedef int uintptr_t; typedef int max_align_t;
static int printf, stdin, fopen, setvbuf, wcwidth, wcpcpy, mbsnrtowcs;
int INT8_MAX, SIZE_MAX, EOF, BUFSIZ, offsetof, LC_CTYPE_MASK;
int main(void) {
    wchar_t w[2] = { 0 }; mbstate_t st = { 0 }; void *p = NULL;
    (void)w; (void)st; (void)p; (void)printf; (void)stdin; (void)fopen; (void)setvbuf;
    (void)wcwidth; (void)wcpcpy; (void)mbsnrtowcs;
    return (int)wcslen(L"x") + LC_ALL + iswalpha(L'a') + isalpha('a') + (int)WEOF +
           (localeconv() != NULL);
}
'''

POSIX = r'''
#define _POSIX_C_SOURCE 202405L
#include <wchar.h>
#include <wctype.h>
#include <ctype.h>
#include <langinfo.h>
#include <locale.h>
#include <stdio.h>
#include <stdarg.h>
int f(FILE *fp, va_list ap, locale_t loc, wctype_t t, mbstate_t *st) {
    const char *s = "x";
    return vfwprintf(fp, L"%d", ap) + wcwidth(L'a') + wcscasecmp_l(L"a", L"A", loc) +
           iswctype_l(1, t, loc) + isalpha_l('a', loc) + nl_langinfo(CODESET)[0] +
           LC_ALL_MASK + (int)mbsnrtowcs(NULL, &s, 1, 0, st) + (getlocalename_l(LC_CTYPE, loc) != NULL);
}
int main(void) { return 0; }
'''

with tempfile.TemporaryDirectory(prefix='breenix-locale-headers-') as directory:
    for name, text in (('strict', STRICT), ('posix', POSIX)):
        source = Path(directory) / f'{name}.c'
        source.write_text(text)
        for arch in arches:
            subprocess.run(['clang', f'--target={arch}-unknown-linux-gnu', '-nostdinc', '-isystem',
                            str(include), '-isystem', f'{resource}/include', '-std=c17', '-Wall',
                            '-Werror', '-fsyntax-only', str(source)], check=True)
            print(f'{name} {arch}: ok')
