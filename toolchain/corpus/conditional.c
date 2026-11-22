/* N1570 6.5.15, one conditional arm is evaluated. */
#include <stdio.h>

int main(void)
{
    int a = 4, b = 7;
    int first = a < b ? a++ : b++;
    int second = a > b ? a++ : b++;
    printf("%d %d %d %d\n", first, second, a, b);
    return 0;
}
