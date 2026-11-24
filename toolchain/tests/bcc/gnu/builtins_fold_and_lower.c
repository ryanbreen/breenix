struct record {
    char byte;
    int values[3];
};

typedef int array[4];

_Static_assert(__builtin_types_compatible_p(const int, int), "top qualifier");
_Static_assert(__builtin_types_compatible_p(array, int[]), "array compatibility");
_Static_assert(!__builtin_types_compatible_p(const int *, int *), "pointed-to qualifier");
_Static_assert(__builtin_offsetof(struct record, values[2]) == 12, "nested offset");
_Static_assert(__builtin_constant_p(1 + 2), "constant operand");
_Static_assert(__builtin_bswap16(0x1205) == 0x0512, "16-bit constant");
_Static_assert(__builtin_bswap32(0x12030405U) == 0x05040312U, "32-bit constant");
_Static_assert(__builtin_bswap64(0x1020304050607080UL) == 0x8070605040302010UL, "64-bit constant");
_Static_assert(__builtin_clz(1U) == 31, "32-bit leading zeros");
_Static_assert(__builtin_ctzll(1ULL << 49) == 49, "64-bit trailing zeros");
_Static_assert(__builtin_popcountl(0xF000000000000003UL) == 6, "population count");

int main(void) {
    volatile unsigned long wide = 0x1020304050607080UL;
    volatile unsigned narrow = 0x100U;
    if (__builtin_constant_p(narrow++))
        return 1;
    if (__builtin_expect(narrow++, 0x100) != 0x100 || narrow != 0x101)
        return 2;
    if (__builtin_bswap16(narrow++) != 0x0101 || narrow != 0x102 ||
        __builtin_bswap32(narrow) != 0x02010000U ||
        __builtin_bswap64(wide) != 0x8070605040302010UL)
        return 3;
    if (__builtin_clz(narrow) != 23 || __builtin_clzl(wide) != 3 ||
        __builtin_ctz(narrow) != 1 || __builtin_ctzll(wide) != 7 ||
        __builtin_popcount(narrow) != 2 || __builtin_popcountl(wide) != 13)
        return 4;
    if (narrow == 0x102)
        return 42;
    __builtin_unreachable();
}
