/* AAPCS64 2025Q1 §§5.5, 6.8.2, 6.9 and appendix 14. */
#include <stdarg.h>
#include "x87-long-double.h"

long double bias = 0.25L;

struct Quads turn(struct Quads value)
{
    static long double one = 1.0L;
    value.first -= one;
    value.second = -value.second;
    return value;
}

long double many(long double a, long double b, long double c, long double d,
                 long double e, long double f, long double g, long double h,
                 long double i, struct Quads tail)
{
    static long double previous;
    if (a != 1.0L || b != 2.0L || c != 3.0L || d != 4.0L ||
        e != 5.0L || f != 6.0L || g != 7.0L || h != 8.0L || i != 9.0L ||
        tail.first != 0x1.0000000000000000000000000001p0L || tail.second != 2.5L)
        return 0.0L;
    previous = (a + i) * tail.second / 2.0L;
    long double old = previous++;
    --previous;
    if (previous != old)
        return 0.0L;
    return old + 11.0L;
}

int variadic(long a, long b, long c, long d, long e, long f, long g, long h,
             long double named, ...)
{
    va_list ap;
    va_start(ap, named);
    struct Quads pair = va_arg(ap, struct Quads);
    long double fp_c = va_arg(ap, long double);
    long double fp_d = va_arg(ap, long double);
    long double fp_e = va_arg(ap, long double);
    long double fp_f = va_arg(ap, long double);
    long double fp_g = va_arg(ap, long double);
    long x = va_arg(ap, long);
    long double spilled = va_arg(ap, long double);
    long y = va_arg(ap, long);
    va_end(ap);
    return a + b + c + d + e + f + g + h == 36 && named == 1.0L &&
           pair.first == 0x1.0000000000000000000000000001p0L &&
           pair.second == 2.5L && fp_c == 3.0L && fp_d == 4.0L && fp_e == 5.0L &&
           fp_f == 6.0L && fp_g == 7.0L && x == 8 && y == 9 && spilled == 9.5L;
}

int conversions(int i, long l, unsigned u, unsigned long ul, float f, double d)
{
    long double qi = i, ql = l, qu = u, qul = ul, qf = f, qd = d;
    if ((int)qi != i || (long)ql != l || (unsigned)qu != u ||
        (unsigned long)qul != ul || (float)qf != f || (double)qd != d)
        return 0;
    long double zero = -0.0L;
    unsigned char *bits = (unsigned char *)&zero;
    if (zero || bits[15] != 0x80)
        return 0;
    /* Make a quiet NaN by setting the exponent and the quiet bit. */
    bits[13] = 0x80;
    bits[14] = 0xff;
    bits[15] = 0x7f;
    return !(qi < ql) && !(qi <= ql) && qi > ql && qi >= ql &&
           zero != ql && !(zero == ql) && !(zero < ql) && !(zero <= ql) &&
           !(zero > ql) && !(zero >= ql) && (_Bool)zero && !(_Bool)(-0.0L);
}
