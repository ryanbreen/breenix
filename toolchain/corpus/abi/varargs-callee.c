/* N1570 §7.16: independent va_copy cursors. AMD64 psABI §3.5.7. */
#include <stdarg.h>

int abi_variadic(int base, ...)
{
    va_list args, copy;
    int sum = base;
    va_start(args, base);
    for (int i = 0; i < 6; ++i)
        sum += va_arg(args, int);
    va_copy(copy, args);
    double first = va_arg(args, double);
    int tail = va_arg(args, int);
    double copied = va_arg(copy, double);
    va_end(copy);
    va_end(args);
    return sum + tail + (int)(first + copied);
}
