#include "sse-float.h"

int puts(const char *);

static struct Floats callback(struct Floats value)
{
    float first = value.first;
    value.first = value.second + 2.0f;
    value.second = first - 1.0f;
    return value;
}

int main(void)
{
    struct Floats pair;
    pair.first = 2.5f;
    pair.second = 3.25f;
    pair = floats(0.5f, pair, 1.25, callback);
    if (pair.first != 6.5f || pair.second != 2.0f)
        return 1;

    struct Doubles wide;
    wide.lanes[0] = 11.5;
    wide.lanes[1] = 13.25;
    wide.lanes[2] = 17.0;
    wide = doubles(1.0f, 2.0, 3.0f, 4.0, 5.0f, 6.0, 7.0f, wide, 8.5, 19);
    if (wide.lanes[0] != 20.0 || wide.lanes[1] != 32.25 || wide.lanes[2] != 24.0)
        return 2;

    union FloatBits scalar;
    scalar.lanes[0] = 3.25f;
    scalar.lanes[1] = 5.5f;
    scalar.vector = vector(scalar.vector);
    if (scalar.lanes[0] != 6.5f || scalar.lanes[1] != 3.25f)
        return 3;

    union DoubleBits lanes;
    struct Vectors value;
    lanes.lanes[0] = 19.0;
    lanes.lanes[1] = 23.25;
    value.first = lanes.vector;
    lanes.lanes[0] = 29.5;
    lanes.lanes[1] = 31.0;
    value.second = lanes.vector;
    value = vectors(9.0f, value);
    lanes.vector = value.first;
    if (lanes.lanes[0] != 29.5 || lanes.lanes[1] != 31.0)
        return 4;
    lanes.vector = value.second;
    if (lanes.lanes[0] != 19.0 || lanes.lanes[1] != 23.25)
        return 5;

    puts("sse-float");
    return 42;
}
