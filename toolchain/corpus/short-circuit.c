/* N1570 6.5.13 and 6.5.14, unselected right operands are not evaluated. */
#include <stdio.h>

int main(void)
{
    int state = 0;
    int x = 0 && ++state;
    int y = 1 || ++state;
    int z = x || ++state;
    printf("%d %d %d %d\n", x, y, z, state);
    return 0;
}
