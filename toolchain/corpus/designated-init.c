/* N1570 6.7.9, array designators and implicit zero initialization. */
#include <stdio.h>

int main(void)
{
    int a[5] = {[3] = 19, [1] = 7};
    printf("%d %d %d %d %d\n", a[0], a[1], a[2], a[3], a[4]);
    return 0;
}
