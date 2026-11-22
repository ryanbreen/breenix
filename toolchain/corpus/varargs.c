/* N1570 6.5.2.2 and 7.16, promotions, successive arguments and va_copy. */
#include <stdarg.h>
#include <stdio.h>

static void describe(int base, ...)
{
    va_list arguments, copy;
    va_start(arguments, base);
    int sum = base;
    for (int i = 0; i < 7; ++i)
        sum += va_arg(arguments, int);
    va_copy(copy, arguments);
    double first = va_arg(arguments, double);
    double copied = va_arg(copy, double);
    const char *word = va_arg(arguments, const char *);
    va_end(copy);
    va_end(arguments);
    printf("%d %d %d %s\n", sum, (int)(first * 2), (int)(copied * 4), word);
}

int main(void)
{
    const char *word = "varargs";
    describe(10, (short)1, 2, 3, 4, 5, 6, 7, (float)2.5, word);
    return 0;
}
