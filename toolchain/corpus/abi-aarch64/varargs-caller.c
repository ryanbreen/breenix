/* AAPCS64 2025Q1 §6.8.2 and appendix 14. */
#include "varargs.h"

int puts(const char *);

int take_int(va_list *list)
{
    return va_arg(*list, int);
}

int main(void)
{
    struct Pair pair;
    pair.first = 31;
    pair.second = 37;
    struct Large large;
    large.first = 41;
    large.second = 43;
    large.third = 47;
    struct Doubles in_regs, on_stack;
    in_regs.first = 0.5;
    in_regs.second = 1.5;
    on_stack.first = 9.5;
    on_stack.second = 10.5;
    int value = 53;
    int status = inspect(23, 0.25, in_regs, pair, large, &value, (short)-3, 5, 7,
                         2.5, 3.5, 4.5, 5.5, 61, on_stack, 11.5, large);
    if (status != 0)
        return status;
    if (!after_named_stack(1, 2, 3, 4, 5, 6, 7, 8, 67, pair, 71, 12.5))
        return 10;
    puts("varargs");
    return 42;
}
