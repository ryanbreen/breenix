/* N1570 6.2.1, block scope hides an outer declaration. */
#include <stdio.h>

int main(void)
{
    int value = 17;
    {
        int value = 21;
        printf("%d ", value);
    }
    printf("%d\n", value);
    return 0;
}
