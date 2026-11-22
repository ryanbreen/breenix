/* N1570 6.5.5 and 6.5.6, division rounds toward zero. */
#include <stdio.h>

int main(void)
{
    int a = -23, b = 5;
    printf("%d %d %d\n", a / b, a % b, a + b * 3);
    return 0;
}
