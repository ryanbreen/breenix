/* N1570 6.3.1.4 and 6.5.5, conversion discards the fractional part. */
#include <stdio.h>

static double half(double value)
{
    return value / 2.0;
}

int main(void)
{
    int positive = (int)half(7.5);
    int negative = (int)half(-7.5);
    printf("%d %d\n", positive, negative);
    return 0;
}
