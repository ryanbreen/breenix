/* AMD64 psABI §3.2.3: a MEMORY fallback rolls back register assignment. */
struct Mixed { int i; double d; };
struct Large { long first, second, third; };

struct Mixed abi_mixed(long a, long b, long c, long d, long e, long f,
                       struct Mixed spilled, double tail)
{
    /* The mixed argument is on the stack, so tail still takes XMM0. */
    spilled.i += (int)(a + b + c + d + e + f);
    spilled.d += tail;
    return spilled;
}

struct Large abi_large(struct Large value)
{
    struct Large result;
    result.first = value.third;
    result.second = value.first + 1;
    result.third = value.second;
    return result;
}
