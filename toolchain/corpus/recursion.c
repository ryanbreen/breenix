/* N1570 6.5.2.2 paragraph 11, direct recursive calls. */
#include <stdio.h>

static unsigned factorial(unsigned n)
{
    return n < 2 ? 1 : n * factorial(n - 1);
}

int main(void)
{
    printf("%u\n", factorial(6));
    return 0;
}
