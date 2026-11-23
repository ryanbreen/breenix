/* AMD64 psABI §§3.1.2, 3.2.3: extended precision in memory and ST(0). */
long double abi_x87(int, long double, double, long double);

int main(void)
{
    long double result = abi_x87(3, 0x1.0000000000000002p0L, 0.5,
                                 0x1.0000000000000004p0L);
    return result != 0x1.000000000000000ap0L;
}
