/* AAPCS64 2025Q1 §§6.8.2 and 6.9, and IEEE 754-2019 binary128. */
#include "x87-long-double.h"

int puts(const char *);

int main(void)
{
    struct Quads input = {0x1.0000000000000000000000000001p0L, 2.5L};
    struct Quads output = turn(input);
    if (output.first != 0x1p-112L || output.second != -2.5L)
        return 1;
    if (bias != 0.25L)
        return 2;
    if (many(1.0L, 2.0L, 3.0L, 4.0L, 5.0L, 6.0L, 7.0L, 8.0L, 9.0L, input) != 23.5L)
        return 3;
    if (!variadic(1, 2, 3, 4, 5, 6, 7, 8, 1.0L, input, 3.0L, 4.0L, 5.0L, 6.0L, 7.0L, 8L, 9.5L, 9L))
        return 4;
    if (!conversions(-42, -4294967297L, 4294967295U, 18446744073709551615UL, 0.75f, 0.125))
        return 5;
    puts("binary128");
    return 42;
}
