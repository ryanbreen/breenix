/* N1570 6.2.4 and 6.7.9, block static storage retains its value. */
#include <stdio.h>

static int next(void)
{
    static int counter;
    return ++counter;
}

int main(void)
{
    int first = next();
    int second = next();
    printf("%d %d\n", first, second);
    return 0;
}
