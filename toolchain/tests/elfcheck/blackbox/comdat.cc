// Black-box input for elfcheck: an out-of-line inline function becomes a
// COMDAT section group in clang's x86-64 relocatable output, so elfcheck's
// group checks see a producer other than the hand-built fixtures.

inline int scaled(int value)
{
    return value * 3 + 1;
}

int use_scaled(int value)
{
    return scaled(value) + scaled(value + 1);
}
