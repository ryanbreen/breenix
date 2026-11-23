#include "sse-float.h"

struct Floats floats(float first, struct Floats value, double last,
                     struct Floats (*callback)(struct Floats))
{
    value.first += first;
    value.second += (float)last;
    return callback(value);
}

struct Doubles doubles(float a, double b, float c, double d, float e, double f, float g,
                       struct Doubles value, double tail, long tag)
{
    if (a != 1.0f || b != 2.0 || c != 3.0f || d != 4.0 || e != 5.0f || f != 6.0) {
        value.lanes[0] = -1.0;
        return value;
    }
    value.lanes[0] += tail;
    value.lanes[1] += (double)tag;
    value.lanes[2] += (double)g;
    return value;
}

Float2 vector(Float2 value)
{
    union FloatBits bits;
    bits.vector = value;
    float first = bits.lanes[0];
    bits.lanes[0] = bits.lanes[1] + 1.0f;
    bits.lanes[1] = first;
    return bits.vector;
}

struct Vectors vectors(float tag, struct Vectors value)
{
    if (tag == 9.0f) {
        Double2 first = value.first;
        value.first = value.second;
        value.second = first;
    }
    return value;
}
