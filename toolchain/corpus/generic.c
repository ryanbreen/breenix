/* N1570 6.5.1.1, selecting by type without evaluating the controlling expression. */
#include <stdio.h>

static int next(int *calls)
{
    return ++*calls;
}

int main(void)
{
    int calls = 0;
    const char *kind = _Generic(next(&calls), int: "int", default: "other");
    printf("%s %d\n", kind, calls);
    return next(&calls) == 1 ? 0 : 1;
}
