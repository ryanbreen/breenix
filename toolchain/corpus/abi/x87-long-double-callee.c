/* AMD64 psABI §3.2.3: x87 arguments in memory, scalar return in ST(0). */
long double abi_x87(int tag, long double a, double scale, long double b)
{
    /* The low bits would disappear if a and b went through binary64. */
    return (a - 1.0L) * tag + (b - 1.0L) + (long double)scale * 2.0L;
}
