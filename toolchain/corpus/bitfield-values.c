/* N1570 6.7.2.1, unsigned field values and promotions. */
#include <stdio.h>

struct fields { unsigned low : 3; unsigned high : 5; };

int main(void)
{
    struct fields word = {3, 17};
    word.low += 2;
    word.high ^= 3;
    printf("%d %d\n", word.low, word.high);
    return 0;
}
