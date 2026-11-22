/* N1570 6.8.5 and 6.8.6.2, for, while, do and continue. */
#include <stdio.h>

int main(void)
{
    int total = 0, remaining = 3;
    for (int n = 1; n <= 7; ++n) {
        if (n % 2 == 0)
            continue;
        total += n;
    }
    while (remaining > 0)
        total += remaining--;
    do {
        total *= 2;
    } while (total < 40);
    printf("%d\n", total);
    return 0;
}
