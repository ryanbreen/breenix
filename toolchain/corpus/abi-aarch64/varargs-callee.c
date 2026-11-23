/* N1570 §7.16.1 and AAPCS64 2025Q1 §§14.1–14.4. */
#include "varargs.h"

int inspect(long marker, double named, ...)
{
    va_list ap, copy;
    va_start(ap, named);
    struct Doubles in_regs = va_arg(ap, struct Doubles);
    struct Pair pair = va_arg(ap, struct Pair);
    struct Large in_gp = va_arg(ap, struct Large);
    int *ptr = va_arg(ap, int *);
    va_copy(copy, ap);
    int first = take_int(&ap);
    int copied = va_arg(copy, int);
    int second = va_arg(ap, int);
    int third = va_arg(ap, int);
    double simd1 = va_arg(ap, double);
    double simd2 = va_arg(ap, double);
    double simd3 = va_arg(ap, double);
    double simd4 = va_arg(ap, double);
    int spilled = va_arg(ap, int);
    struct Doubles on_stack = va_arg(ap, struct Doubles);
    double after_hfa = va_arg(ap, double);
    struct Large indirect = va_arg(ap, struct Large);
    int copy_next = va_arg(copy, int);
    double copy_fp = va_arg(copy, double);
    va_end(copy);
    va_end(ap);

    if (marker != 23 || named != 0.25 || in_regs.first != 0.5 || in_regs.second != 1.5)
        return 1;
    if (pair.first != 31 || pair.second != 37 || *ptr != 53)
        return 2;
    if (in_gp.first != 41 || in_gp.second != 43 || in_gp.third != 47)
        return 3;
    if (first != -3 || copied != -3 || second != 5 || third != 7 ||
        copy_next != 5 || copy_fp != 2.5)
        return 4;
    if (simd1 != 2.5 || simd2 != 3.5 || simd3 != 4.5 || simd4 != 5.5)
        return 5;
    if (spilled != 61 || on_stack.first != 9.5 || on_stack.second != 10.5 ||
        after_hfa != 11.5)
        return 6;
    if (indirect.first != 41 || indirect.second != 43 || indirect.third != 47)
        return 7;
    return 0;
}

int after_named_stack(long a, long b, long c, long d, long e, long f, long g, long h,
                      long named, ...)
{
    va_list ap;
    va_start(ap, named);
    struct Pair spilled = va_arg(ap, struct Pair);
    int next = va_arg(ap, int);
    double fp = va_arg(ap, double);
    va_end(ap);
    return a + b + c + d + e + f + g + h == 36 && named == 67 &&
           spilled.first == 31 && spilled.second == 37 && next == 71 && fp == 12.5;
}
