/* N1570 6.2.1 and 6.8.6.1, a forward goto finds a function-scope label. */
#include <stdio.h>

int main(void)
{
    int value = 2;
    {
        value *= 3;
        goto done;
    }
    value = 0;
done:
    printf("%d\n", value);
    return 0;
}
