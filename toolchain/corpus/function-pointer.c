/* N1570 6.3.2.1 and 6.5.2.2, indirect calls through a function pointer. */
#include <stdio.h>

static int add(int a, int b) { return a + b; }
static int subtract(int a, int b) { return a - b; }

int main(void)
{
    int (*functions[])(int, int) = {add, subtract};
    printf("%d %d\n", functions[0](9, 4), functions[1](9, 4));
    return 0;
}
