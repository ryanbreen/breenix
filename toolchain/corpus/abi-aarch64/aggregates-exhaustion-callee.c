#include "aggregates-exhaustion.h"

union Aligned aligned(long first, union Aligned value, long last)
{
    value.words[0] += first;
    value.words[1] += last;
    return value;
}

struct Large exhausted(long a, long b, long c, long d, long e, long f, long g,
                       struct Pair pair, long tail, struct Large value, struct Floats floats)
{
    if (a != 1 || b != 2 || c != 3 || d != 4 || e != 5 || f != 6 || g != 7 ||
        floats.lanes[0] != 0.5f || floats.lanes[1] != 1.25f || floats.lanes[2] != 2.75f) {
        value.first = -1;
        return value;
    }
    value.first += pair.first;
    value.second += pair.second;
    value.third += tail;
    return value;
}
