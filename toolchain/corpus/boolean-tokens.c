/* N1570 7.9 and 7.18, alternative tokens and conversion to bool. */
#include <iso646.h>
#include <stdbool.h>
#include <stdio.h>

int main(void)
{
    bool nonzero = 7;
    bool zero = 0;
    printf("%d %d\n", nonzero and not zero, nonzero xor true);
    return 0;
}
