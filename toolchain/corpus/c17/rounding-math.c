/* C17 corpus: rounding-math.
 * With FENV_ACCESS on, an arithmetic constant expression of floating type
 * outside a static initializer is evaluated as if at execution time (F.8.4p1,
 * F.8.5p1, 6.6p8), so each result and its exception flags follow the rounding
 * direction fesetround selects, and a build may not fold them at translation
 * time. A static initializer is evaluated at translation time under round to
 * nearest (F.8.2p1; F.8.4 footnote 366). Each line prints the mode, the
 * operation, the binary64 bit pattern (or nan) and the raised flags.
 * Clauses: N1570 6.6p8, 7.6.1p2, 7.6.2.1p2, 7.6.2.5p2, 7.6.3.2p2, 7.24.2.1p2, F.3p1, F.8.1p1,
 *   F.8.2p1, F.8.3p1, F.8.4p1, F.8.5p1, F.9.2p1;
 *   IEEE 754-2019 3.4, 4.3.1, 4.3.2, 5.4.1, 6.3, 7.2, 7.3, 7.4, 7.5, 7.6.
 * Expectations: rounding-math.expect, written by hand from these clauses.
 */
#include <fenv.h>
#include <float.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#pragma STDC FENV_ACCESS ON

static const double one_third = 1.0 / 3.0;

static const char *const flag_names[] = { "invalid", "divbyzero", "overflow", "underflow", "inexact" };
static const int flag_values[] = { FE_INVALID, FE_DIVBYZERO, FE_OVERFLOW, FE_UNDERFLOW, FE_INEXACT };

static void report(const char *mode, const char *operation, double value, int raised)
{
    printf("%s %s ", mode, operation);
    if (value != value) {
        fputs("nan", stdout);
    } else {
        uint64_t bits;
        memcpy(&bits, &value, sizeof bits);
        for (int shift = 60; shift >= 0; shift -= 4)
            putchar("0123456789abcdef"[(bits >> shift) & 0xf]);
    }
    int printed = 0;
    for (int i = 0; i < 5; ++i) {
        if (raised & flag_values[i]) {
            printf("%s%s", printed ? "|" : " ", flag_names[i]);
            printed = 1;
        }
    }
    puts(printed ? "" : " none");
}

/* Clear the flags, evaluate once, and read the flags before printing. */
#define MEASURE(operation, expression)                       \
    do {                                                     \
        if (feclearexcept(FE_ALL_EXCEPT) != 0)               \
            return 2;                                        \
        double result = (expression);                        \
        int raised = fetestexcept(FE_ALL_EXCEPT);            \
        report(mode, operation, result, raised);             \
    } while (0)

int main(void)
{
    static const int modes[] = { FE_TONEAREST, FE_DOWNWARD, FE_UPWARD, FE_TOWARDZERO };
    static const char *const names[] = { "nearest", "downward", "upward", "towardzero" };

    for (int i = 0; i < 4; ++i) {
        const char *mode = names[i];
        if (fesetround(modes[i]) != 0 || fegetround() != modes[i])
            return 1;
        /* 1/3 lies 1/3 ulp above 0x1.5555555555555p-2. */
        MEASURE("third", 1.0 / 3.0);
        MEASURE("negthird", -1.0 / 3.0);
        /* Translation time, round to nearest; reading it raises nothing. */
        MEASURE("static", one_third);
        /* Overflow gives infinity or the largest finite value by direction. */
        MEASURE("overflow", DBL_MAX * 2.0);
        /* 2^-1022 / 3 is 0x5555555555555.555... units of 2^-1074. */
        MEASURE("underflow", DBL_MIN / 3.0);
        MEASURE("pole", 1.0 / 0.0);
        MEASURE("invalid", 0.0 / 0.0);
        /* An exact zero difference is -0 only when rounding downward. */
        MEASURE("zerodiff", 0.0 - 0.0);
        /* 1e16 = 5e15 * 2 and 1e16 + 1 is halfway to 1e16 + 2; no reassociation. */
        MEASURE("absorb", (1e16 + 1.0) - 1e16);
        /* 2^53 + 1 is halfway between 2^53 and 2^53 + 2. */
        MEASURE("convert", (double)9007199254740993LL);
        MEASURE("exact", 1.0 + 2.0);
    }
    if (fesetround(FE_TONEAREST) != 0)
        return 1;
    return 0;
}
