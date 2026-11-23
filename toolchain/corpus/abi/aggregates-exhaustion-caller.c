/* AMD64 psABI §3.2.3: INTEGER/SSE eightbytes and MEMORY return. */
struct Mixed { int i; double d; };
struct Large { long first, second, third; };

struct Mixed abi_mixed(long, long, long, long, long, long, struct Mixed, double);
struct Large abi_large(struct Large);

int main(void)
{
    struct Mixed mixed;
    struct Large large;

    mixed.i = 19;
    mixed.d = 2.5;
    mixed = abi_mixed(1, 2, 3, 4, 5, 6, mixed, 7.5);
    if (mixed.i != 40 || mixed.d != 10.0)
        return 1;

    large.first = 11;
    large.second = 13;
    large.third = 17;
    large = abi_large(large);
    return large.first != 17 || large.second != 12 || large.third != 13;
}
