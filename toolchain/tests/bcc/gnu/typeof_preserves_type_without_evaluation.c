static int count;
static long next(void) { ++count; return 0; }
int main(void) {
    const unsigned char bytes[7] = { 3 };
    typeof(bytes) copy = { 5 };
    __typeof__(next()) n = sizeof(copy);
    __typeof__(long *) p = &n;
    _Static_assert(_Generic(&copy, const unsigned char (*)[7]: 1, default: 0), "array and const");
    return *p + sizeof(n) + copy[0] + bytes[0] + 19 + count;
}
