/* N1570 6.3.1.1 and 5.2.4.2.1, narrow unsigned operands promote to int. */
#include <limits.h>
#include <stdio.h>

int main(void)
{
    unsigned char a = UCHAR_MAX, b = 1;
    printf("%d\n", a + b);
    return 0;
}
