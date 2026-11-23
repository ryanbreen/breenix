typedef unsigned short __attribute__((__may_alias__)) alias_half;
typedef alias_half nested_half;
typedef unsigned short plain_half;

static unsigned int update(unsigned int *whole, nested_half *part)
{
    *whole = 0x12345678U;
    *part = 0x002aU;
    return *whole;
}

int main(void)
{
    unsigned int value = 0;
    _Static_assert(sizeof(nested_half) == sizeof(plain_half), "same object representation");
    _Static_assert(_Generic((nested_half *)0, plain_half *: 1, default: 0),
                   "may_alias does not change C type compatibility");
    return update(&value, (nested_half *)&value) == 0x1234002aU ? 42 : 1;
}
