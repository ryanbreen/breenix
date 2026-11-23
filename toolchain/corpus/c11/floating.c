/* N1570 5.2.4.2.2, 7.6, F.3, F.7 and F.8.4. */
#include <fenv.h>
#include <float.h>
#include <stdio.h>

#pragma STDC FENV_ACCESS ON

int main(void)
{
    int modes[] = { FE_TONEAREST, FE_DOWNWARD, FE_UPWARD, FE_TOWARDZERO };
    int rounds[] = { 1, 3, 2, 0 };
    const char *names[] = { "nearest", "downward", "upward", "toward zero" };
    volatile double half_ulp = 0x1p-53;
    volatile double float_halfway = 0x1.000001p0;
    volatile long double extended_half_ulp = 0x1p-64L;

    if (FLT_EVAL_METHOD != 0 || FLT_RADIX != 2 ||
        DBL_EPSILON != 0x1p-52 || 0.1 != 0x1.999999999999ap-4)
        return 1;

    for (int i = 0; i < 4; ++i) {
        if (fesetround(modes[i]) || fegetround() != modes[i] || FLT_ROUNDS != rounds[i])
            return 2;
        if (feclearexcept(FE_ALL_EXCEPT))
            return 3;
        double positive = 1.0 + half_ulp;
        double negative = -1.0 - half_ulp;
        if (positive != (modes[i] == FE_UPWARD ? 0x1.0000000000001p0 : 1.0) ||
            negative != (modes[i] == FE_DOWNWARD ? -0x1.0000000000001p0 : -1.0) ||
            fetestexcept(FE_ALL_EXCEPT) != FE_INEXACT)
            return 4;

        if (feclearexcept(FE_ALL_EXCEPT))
            return 5;
        float narrow = (float)float_halfway;
        if (narrow != (modes[i] == FE_UPWARD ? 0x1.000002p0f : 1.0f) ||
            fetestexcept(FE_ALL_EXCEPT) != FE_INEXACT)
            return 6;

        if (feclearexcept(FE_ALL_EXCEPT))
            return 7;
        long double extended = 1.0L + extended_half_ulp;
        if (extended != (modes[i] == FE_UPWARD ? 0x1.0000000000000002p0L : 1.0L) ||
            fetestexcept(FE_ALL_EXCEPT) != FE_INEXACT)
            return 8;
        puts(names[i]);
    }

    if (fesetround(FE_TONEAREST) || feclearexcept(FE_ALL_EXCEPT))
        return 9;
    volatile double zero = 0.0;
    volatile double quotient = 1.0 / zero;
    if (fetestexcept(FE_ALL_EXCEPT) != FE_DIVBYZERO || quotient <= DBL_MAX)
        return 10;
    puts("divide by zero");
    return 0;
}
