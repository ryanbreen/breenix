/* N1570 6.5.2.1 and 6.7.6.2, array subscripting across two dimensions. */
#include <stdio.h>

int main(void)
{
    int grid[2][3] = {{1, 2, 3}, {4, 5, 6}};
    printf("%d %d\n", grid[1][0], *(*(grid + 1) + 2));
    return 0;
}
