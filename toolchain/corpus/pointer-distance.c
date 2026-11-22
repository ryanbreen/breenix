/* N1570 6.5.6 and 7.19, subtracting pointers into the same array. */
#include <stddef.h>
#include <stdio.h>

int main(void)
{
    int a[] = {3, 5, 8, 13};
    ptrdiff_t distance = a + 4 - (a + 1);
    printf("%td %d\n", distance, *(a + distance));
    return 0;
}
