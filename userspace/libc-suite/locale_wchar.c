/*
 * libc suite, locale-wchar category: <locale.h>, <wchar.h> and <wctype.h>. The UTF-8
 * cases use the locale Linux names C.UTF-8; the suite gives the program no LANG or LC_*
 * variables, so setlocale(LC_ALL, "") selects the C locale.
 */
#define _POSIX_C_SOURCE 202405L
#include "case.h"

#include <errno.h>
#include <langinfo.h>
#include <limits.h>
#include <locale.h>
#include <stdio.h>
#include <stdlib.h>
#include <wchar.h>
#include <wctype.h>

static int utf8(void) {
    return setlocale(LC_ALL, "C.UTF-8") != NULL;
}

#define NEED_UTF8() CHECK(utf8(), "setlocale(LC_ALL, \"C.UTF-8\") failed")

static const char *setlocale_startup(void) {
    EXPECT_STR("setlocale(LC_ALL, NULL) at startup", setlocale(LC_ALL, NULL), "C");
    EXPECT_STR("setlocale(LC_CTYPE, NULL) at startup", setlocale(LC_CTYPE, NULL), "C");
    EXPECT_INT("MB_CUR_MAX in the C locale", (long)MB_CUR_MAX, 1);
    return PASS;
}

static const char *setlocale_posix(void) {
    const char *r = setlocale(LC_ALL, "POSIX");
    CHECK(r != NULL, "setlocale(LC_ALL, \"POSIX\") failed");
    r = setlocale(LC_NUMERIC, NULL);
    CHECK(h_streq(r, "C") || h_streq(r, "POSIX"), "LC_NUMERIC after selecting POSIX is neither C nor POSIX");
    EXPECT_STR("setlocale(LC_ALL, \"C\")", setlocale(LC_ALL, "C"), "C");
    return PASS;
}

static const char *setlocale_unknown(void) {
    EXPECT_PTR("setlocale of an unknown locale", setlocale(LC_ALL, "xx_YY.bogus"), NULL);
    EXPECT_STR("locale after a failed setlocale", setlocale(LC_ALL, NULL), "C");
    return PASS;
}

static const char *setlocale_environment(void) {
    EXPECT_STR("setlocale(LC_ALL, \"\") with no locale variables", setlocale(LC_ALL, ""), "C");
    CHECK(setenv("LC_ALL", "C.UTF-8", 1) == 0, "setenv failed");
    CHECK(setenv("LC_CTYPE", "POSIX", 1) == 0, "setenv failed");
    EXPECT_STR("setlocale(LC_CTYPE, \"\") with LC_ALL=C.UTF-8 overriding LC_CTYPE", setlocale(LC_CTYPE, ""), "C.UTF-8");
    CHECK(MB_CUR_MAX >= 4, "MB_CUR_MAX in C.UTF-8 is below 4");
    return PASS;
}

static const char *localeconv_c(void) {
    struct lconv *l = localeconv();
    CHECK(l != NULL, "localeconv returned NULL");
    EXPECT_STR("decimal_point", l->decimal_point, ".");
    EXPECT_STR("thousands_sep", l->thousands_sep, "");
    EXPECT_STR("grouping", l->grouping, "");
    EXPECT_STR("int_curr_symbol", l->int_curr_symbol, "");
    EXPECT_STR("currency_symbol", l->currency_symbol, "");
    EXPECT_STR("mon_decimal_point", l->mon_decimal_point, "");
    EXPECT_INT("frac_digits", l->frac_digits, CHAR_MAX);
    EXPECT_INT("p_cs_precedes", l->p_cs_precedes, CHAR_MAX);
    EXPECT_INT("n_sign_posn", l->n_sign_posn, CHAR_MAX);
    return PASS;
}

static const char *nl_langinfo_codeset(void) {
    NEED_UTF8();
    EXPECT_STR("nl_langinfo(CODESET) in C.UTF-8", nl_langinfo(CODESET), "UTF-8");
    setlocale(LC_ALL, "C");
    EXPECT_STR("nl_langinfo(RADIXCHAR) in C", nl_langinfo(RADIXCHAR), ".");
    return PASS;
}

static const char *mbrtowc_lengths(void) {
    wchar_t wc = 0;
    mbstate_t st = { 0 };
    NEED_UTF8();
    EXPECT_INT("mbrtowc of \"A\"", (long)mbrtowc(&wc, "A", 4, &st), 1);
    EXPECT_INT("wide character of \"A\"", wc, 'A');
    EXPECT_INT("mbrtowc of U+00E9", (long)mbrtowc(&wc, "\xc3\xa9", 4, &st), 2);
    EXPECT_INT("wide character of U+00E9", wc, 0xE9);
    EXPECT_INT("mbrtowc of U+20AC", (long)mbrtowc(&wc, "\xe2\x82\xac", 4, &st), 3);
    EXPECT_INT("wide character of U+20AC", wc, 0x20AC);
    EXPECT_INT("mbrtowc of U+1F600", (long)mbrtowc(&wc, "\xf0\x9f\x98\x80", 4, &st), 4);
    EXPECT_INT("wide character of U+1F600", wc, 0x1F600);
    EXPECT_INT("mbrtowc of the NUL byte", (long)mbrtowc(&wc, "", 1, &st), 0);
    EXPECT_INT("wide character of the NUL byte", wc, 0);
    return PASS;
}

static const char *mbrtowc_incomplete(void) {
    wchar_t wc = 0;
    mbstate_t st = { 0 };
    NEED_UTF8();
    EXPECT_INT("mbrtowc of the first two bytes of U+20AC", (long)mbrtowc(&wc, "\xe2\x82", 2, &st), -2);
    CHECK(!mbsinit(&st), "mbsinit is true in the middle of a character");
    EXPECT_INT("mbrtowc of the last byte", (long)mbrtowc(&wc, "\xac", 1, &st), 1);
    EXPECT_INT("wide character completed across calls", wc, 0x20AC);
    CHECK(mbsinit(&st), "mbsinit is false after the character completed");
    return PASS;
}

static const char *mbrtowc_invalid(void) {
    wchar_t wc;
    mbstate_t st = { 0 };
    NEED_UTF8();
    errno = 0;
    EXPECT_RET_ERRNO("mbrtowc of byte 0xff", (long)mbrtowc(&wc, "\xff", 1, &st), -1, EILSEQ);
    mbstate_t s2 = { 0 };
    errno = 0;
    EXPECT_RET_ERRNO("mbrtowc of the overlong C0 80", (long)mbrtowc(&wc, "\xc0\x80", 2, &s2), -1, EILSEQ);
    mbstate_t s3 = { 0 };
    errno = 0;
    EXPECT_RET_ERRNO("mbrtowc of the surrogate ED A0 80", (long)mbrtowc(&wc, "\xed\xa0\x80", 3, &s3), -1, EILSEQ);
    return PASS;
}

static const char *utf8_range(void) {
    wchar_t wc;
    char b[8];
    mbstate_t st = { 0 };
    NEED_UTF8();
    errno = 0;
    EXPECT_RET_ERRNO("mbrtowc of F4 90 80 80, above U+10FFFF", (long)mbrtowc(&wc, "\xf4\x90\x80\x80", 4, &st), -1, EILSEQ);
    mbstate_t s2 = { 0 };
    errno = 0;
    EXPECT_RET_ERRNO("wcrtomb of 0x110000", (long)wcrtomb(b, 0x110000, &s2), -1, EILSEQ);
    mbstate_t s3 = { 0 };
    errno = 0;
    EXPECT_RET_ERRNO("wcrtomb of the surrogate U+D800", (long)wcrtomb(b, 0xD800, &s3), -1, EILSEQ);
    return PASS;
}

static const char *wcrtomb_utf8(void) {
    char b[8];
    mbstate_t st = { 0 };
    NEED_UTF8();
    EXPECT_INT("wcrtomb of U+20AC", (long)wcrtomb(b, 0x20AC, &st), 3);
    EXPECT_MEM("bytes of U+20AC", b, "\xe2\x82\xac", 3);
    EXPECT_INT("wcrtomb of U+10FFFF", (long)wcrtomb(b, 0x10FFFF, &st), 4);
    EXPECT_MEM("bytes of U+10FFFF", b, "\xf4\x8f\xbf\xbf", 4);
    mbstate_t s2 = { 0 };
    EXPECT_INT("wcrtomb(NULL, ...)", (long)wcrtomb(NULL, L'x', &s2), 1);
    return PASS;
}

static const char *wcrtomb_c_locale(void) {
    char b[8];
    mbstate_t st = { 0 };
    EXPECT_INT("wcrtomb of 'A' in the C locale", (long)wcrtomb(b, L'A', &st), 1);
    EXPECT_INT("byte of 'A'", b[0], 'A');
    errno = 0;
    EXPECT_RET_ERRNO("wcrtomb of U+20AC in the C locale", (long)wcrtomb(b, 0x20AC, &st), -1, EILSEQ);
    return PASS;
}

static const char *mbstowcs_round_trip(void) {
    wchar_t w[16];
    char b[16];
    NEED_UTF8();
    EXPECT_INT("mbstowcs(NULL, \"h\\xc3\\xa9llo\", 0)", (long)mbstowcs(NULL, "h\xc3\xa9llo", 0), 5);
    EXPECT_INT("mbstowcs", (long)mbstowcs(w, "h\xc3\xa9llo", 16), 5);
    EXPECT_INT("second wide character", w[1], 0xE9);
    EXPECT_INT("terminator", w[5], 0);
    EXPECT_INT("wcstombs", (long)wcstombs(b, w, sizeof b), 6);
    EXPECT_STR("bytes after the round trip", b, "h\xc3\xa9llo");
    errno = 0;
    EXPECT_RET_ERRNO("mbstowcs of an invalid sequence", (long)mbstowcs(w, "a\xff", 16), -1, EILSEQ);
    return PASS;
}

static const char *mbsrtowcs_source(void) {
    wchar_t w[4];
    const char *src = "ab\xc3\xa9";
    mbstate_t st = { 0 };
    NEED_UTF8();
    EXPECT_INT("mbsrtowcs into room for 2", (long)mbsrtowcs(w, &src, 2, &st), 2);
    EXPECT_STR("source after 2 wide characters", src, "\xc3\xa9");
    EXPECT_INT("mbsrtowcs of the rest", (long)mbsrtowcs(w, &src, 4, &st), 1);
    EXPECT_PTR("source after reaching the terminator", src, NULL);
    EXPECT_INT("wide character", w[0], 0xE9);
    return PASS;
}

static const char *mblen_mbtowc(void) {
    wchar_t wc;
    NEED_UTF8();
    EXPECT_INT("mblen(NULL, 0) in a stateless encoding", mblen(NULL, 0), 0);
    EXPECT_INT("mblen of U+00E9", mblen("\xc3\xa9", 2), 2);
    EXPECT_INT("mblen of an incomplete character", mblen("\xc3", 1), -1);
    EXPECT_INT("mbtowc of U+00E9", mbtowc(&wc, "\xc3\xa9", 2), 2);
    EXPECT_INT("wide character", wc, 0xE9);
    EXPECT_INT("mbtowc of the empty string", mbtowc(&wc, "", 1), 0);
    return PASS;
}

static const char *btowc_wctob(void) {
    NEED_UTF8();
    EXPECT_INT("btowc('A')", (long)btowc('A'), 'A');
    EXPECT_INT("btowc(EOF)", (long)btowc(EOF), (long)WEOF);
    EXPECT_INT("btowc(0xc3), a lead byte", (long)btowc(0xc3), (long)WEOF);
    EXPECT_INT("wctob(L'A')", wctob(L'A'), 'A');
    EXPECT_INT("wctob(U+00E9)", wctob(0xE9), EOF);
    return PASS;
}

static const char *wcs_functions(void) {
    wchar_t b[16];
    EXPECT_INT("wcslen", (long)wcslen(L"hello"), 5);
    CHECK(wcscmp(L"abc", L"abd") < 0, "wcscmp(L\"abc\", L\"abd\") is not negative");
    CHECK(wcsncmp(L"abcx", L"abcy", 3) == 0, "wcsncmp looked past n");
    wcscpy(b, L"ab");
    wcscat(b, L"cd");
    CHECK(wcscmp(b, L"abcd") == 0, "wcscpy and wcscat did not build L\"abcd\"");
    const wchar_t *s = L"hello world";
    EXPECT_PTR("wcschr", wcschr(s, L'o'), s + 4);
    EXPECT_PTR("wcsrchr", wcsrchr(s, L'o'), s + 7);
    EXPECT_PTR("wcsstr", wcsstr(s, L"wor"), s + 6);
    EXPECT_INT("wcsspn", (long)wcsspn(s, L"leho"), 5);
    return PASS;
}

static const char *wmem_functions(void) {
    wchar_t a[6] = { 1, 2, 3, 4, 5, 0 }, b[6];
    EXPECT_PTR("wmemcpy return", wmemcpy(b, a, 6), b);
    EXPECT_INT("wmemcmp of equal arrays", wmemcmp(a, b, 6), 0);
    wmemmove(a + 1, a, 4);
    CHECK(a[1] == 1 && a[4] == 4 && a[5] == 0, "wmemmove to an overlapping later address");
    wmemset(b, 0x20AC, 3);
    CHECK(b[0] == 0x20AC && b[2] == 0x20AC && b[3] == 4, "wmemset");
    EXPECT_PTR("wmemchr", wmemchr(b, 4, 6), b + 3);
    CHECK(wmemcmp(L"a", L"b", 1) < 0, "wmemcmp(L\"a\", L\"b\", 1) is not negative");
    return PASS;
}

static const char *wcstol_wcstod(void) {
    wchar_t *end;
    const wchar_t *s = L"  -123x";
    EXPECT_INT("wcstol(L\"  -123x\", 10)", wcstol(s, &end, 10), -123);
    EXPECT_PTR("wcstol end pointer", end, s + 6);
    EXPECT_INT("wcstol(L\"ff\", 16)", wcstol(L"ff", &end, 16), 255);
    EXPECT_DBL("wcstod(L\"2.5e1\")", wcstod(L"2.5e1", &end), 25.0);
    errno = 0;
    EXPECT_RET_ERRNO("wcstol of a value above LONG_MAX", wcstol(L"99999999999999999999", &end, 10), LONG_MAX, ERANGE);
    return PASS;
}

static const char *wctype_utf8(void) {
    NEED_UTF8();
    CHECK(iswalpha(0xE9), "iswalpha(U+00E9) is false in C.UTF-8");
    CHECK(iswupper(0xC9) && iswlower(0xE9), "iswupper(U+00C9) or iswlower(U+00E9) is false");
    EXPECT_INT("towupper(U+00E9)", (long)towupper(0xE9), 0xC9);
    EXPECT_INT("towlower(U+0391)", (long)towlower(0x391), 0x3B1);
    CHECK(!iswdigit(0x663), "iswdigit is true for ARABIC-INDIC DIGIT THREE");
    CHECK(iswdigit(L'7'), "iswdigit(L'7') is false");
    CHECK(!iswalpha(0x20AC), "iswalpha(U+20AC) is true");
    return PASS;
}

static const char *wctype_c_locale(void) {
    CHECK(iswalpha(L'a') && !iswalpha(L'1'), "iswalpha in the C locale");
    CHECK(iswspace(L' ') && iswspace(L'\t') && !iswspace(L'x'), "iswspace in the C locale");
    CHECK(!iswalpha(0xE9), "iswalpha(U+00E9) is true in the C locale");
    EXPECT_INT("towupper(L'a')", (long)towupper(L'a'), 'A');
    EXPECT_INT("towupper(WEOF)", (long)towupper(WEOF), (long)WEOF);
    return PASS;
}

static const char *wctype_wctrans(void) {
    wctype_t alpha = wctype("alpha");
    CHECK(alpha != 0, "wctype(\"alpha\") is 0");
    CHECK(iswctype(L'q', alpha) && !iswctype(L'1', alpha), "iswctype with wctype(\"alpha\")");
    EXPECT_INT("wctype(\"bogus\")", (long)wctype("bogus"), 0);
    wctrans_t up = wctrans("toupper");
    CHECK(up != 0, "wctrans(\"toupper\") is 0");
    EXPECT_INT("towctrans(L'q', toupper)", (long)towctrans(L'q', up), 'Q');
    EXPECT_INT("wctrans(\"bogus\")", (long)(size_t)wctrans("bogus"), 0);
    return PASS;
}

static const char *fwide_orientation(void) {
    FILE *f = fopen("w", "w+");
    CHECK(f != NULL, "fopen failed");
    EXPECT_INT("fwide of a new stream", fwide(f, 0), 0);
    EXPECT_INT("fputwc", (long)fputwc(L'x', f), 'x');
    CHECK(fwide(f, 0) > 0, "the stream is not wide-oriented after fputwc");
    CHECK(fwide(f, -1) > 0, "fwide changed an oriented stream's orientation");
    FILE *g = fopen("b", "w+");
    CHECK(g != NULL, "fopen failed");
    fputc('x', g);
    CHECK(fwide(g, 0) < 0, "the stream is not byte-oriented after fputc");
    return PASS;
}

static const char *fputws_fgetws(void) {
    char bytes[16];
    wchar_t w[16];
    NEED_UTF8();
    FILE *f = fopen("w", "w+");
    CHECK(f != NULL, "fopen failed");
    CHECK(fputws(L"h\xe9!", f) >= 0, "fputws failed");
    EXPECT_INT("fflush", fflush(f), 0);
    h_read_file("w", bytes, sizeof bytes);
    EXPECT_STR("bytes fputws wrote", bytes, "h\xc3\xa9!");
    rewind(f);
    CHECK(fgetws(w, 16, f) != NULL, "fgetws failed");
    CHECK(w[0] == L'h' && w[1] == 0xE9 && w[2] == L'!' && w[3] == 0, "fgetws did not read back h, U+00E9, !");
    return PASS;
}

static const char *fgetwc_eilseq(void) {
    NEED_UTF8();
    CHECK(h_put_file("bad", "a\xff") == 0, "could not create the file");
    FILE *f = fopen("bad", "r");
    CHECK(f != NULL, "fopen failed");
    EXPECT_INT("fgetwc", (long)fgetwc(f), 'a');
    errno = 0;
    EXPECT_RET_ERRNO("fgetwc of byte 0xff", (long)fgetwc(f), (long)WEOF, EILSEQ);
    CHECK(ferror(f), "ferror is clear after an encoding error");
    return PASS;
}

static const char *swprintf_wide(void) {
    wchar_t b[32];
    NEED_UTF8();
    EXPECT_INT("swprintf", swprintf(b, 32, L"%d-%ls-%lc", 7, L"x\xe9", (wint_t)0x20AC), 6);
    CHECK(wcscmp(b, L"7-x\xe9-\x20ac") == 0, "swprintf output differs from L\"7-x\\xe9-\\x20ac\"");
    CHECK(swprintf(b, 3, L"%s", "abcdef") < 0, "swprintf that does not fit returned a non-negative count");
    char nb[32];
    snprintf(nb, sizeof nb, "%ls", L"\xe9");
    EXPECT_STR("snprintf of %ls with U+00E9", nb, "\xc3\xa9");
    return PASS;
}

static const char *uselocale_thread(void) {
    locale_t u = newlocale(LC_CTYPE_MASK, "C.UTF-8", (locale_t)0);
    CHECK(u != (locale_t)0, "newlocale(LC_CTYPE_MASK, \"C.UTF-8\") failed");
    locale_t old = uselocale(u);
    CHECK(old == LC_GLOBAL_LOCALE, "uselocale did not report the global locale as the previous one");
    CHECK(MB_CUR_MAX >= 4, "MB_CUR_MAX under uselocale(C.UTF-8) is below 4");
    CHECK(iswalpha_l(0xE9, u), "iswalpha_l(U+00E9, C.UTF-8) is false");
    uselocale(LC_GLOBAL_LOCALE);
    EXPECT_INT("MB_CUR_MAX back in the global C locale", (long)MB_CUR_MAX, 1);
    EXPECT_STR("global locale", setlocale(LC_CTYPE, NULL), "C");
    freelocale(u);
    return PASS;
}

const struct libc_case libc_cases[] = {
    { "setlocale-startup", setlocale_startup },
    { "setlocale-posix", setlocale_posix },
    { "setlocale-unknown", setlocale_unknown },
    { "setlocale-environment", setlocale_environment },
    { "localeconv-c", localeconv_c },
    { "nl-langinfo-codeset", nl_langinfo_codeset },
    { "mbrtowc-lengths", mbrtowc_lengths },
    { "mbrtowc-incomplete", mbrtowc_incomplete },
    { "mbrtowc-invalid", mbrtowc_invalid },
    { "utf8-range", utf8_range },
    { "wcrtomb-utf8", wcrtomb_utf8 },
    { "wcrtomb-c-locale", wcrtomb_c_locale },
    { "mbstowcs-round-trip", mbstowcs_round_trip },
    { "mbsrtowcs-source", mbsrtowcs_source },
    { "mblen-mbtowc", mblen_mbtowc },
    { "btowc-wctob", btowc_wctob },
    { "wcs-functions", wcs_functions },
    { "wmem-functions", wmem_functions },
    { "wcstol-wcstod", wcstol_wcstod },
    { "wctype-utf8", wctype_utf8 },
    { "wctype-c-locale", wctype_c_locale },
    { "wctype-wctrans", wctype_wctrans },
    { "fwide-orientation", fwide_orientation },
    { "fputws-fgetws", fputws_fgetws },
    { "fgetwc-eilseq", fgetwc_eilseq },
    { "swprintf-wide", swprintf_wide },
    { "uselocale-thread", uselocale_thread },
    { 0, 0 },
};
