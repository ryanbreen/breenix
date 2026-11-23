#include "aggregates-exhaustion.h"

int puts(const char *);

int main(void)
{
    union Aligned value;
    value.words[0] = 29;
    value.words[1] = 31;
    value = aligned(23, value, 37);
    if (value.words[0] != 52 || value.words[1] != 68)
        return 1;

    struct Pair pair;
    pair.first = 17;
    pair.second = 19;
    struct Large large;
    large.first = 29;
    large.second = 31;
    large.third = 37;
    struct Floats floats;
    floats.lanes[0] = 0.5f;
    floats.lanes[1] = 1.25f;
    floats.lanes[2] = 2.75f;

    struct Large result;
    result = exhausted(1, 2, 3, 4, 5, 6, 7, pair, 23, large, floats);
    if (result.first != 46 || result.second != 50 || result.third != 60)
        return 2;
    if (large.first != 29 || large.second != 31 || large.third != 37)
        return 3;

    puts("aggregates-exhaustion");
    return 42;
}
