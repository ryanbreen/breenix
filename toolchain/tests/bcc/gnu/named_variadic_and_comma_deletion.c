#include <unit_wrapper.h>

#define SUM(first, args...) total(sizeof((int[]){first, ##args}) / sizeof(int), first, ##args)
#define IDENTITY(args...) args
#define COMMA(head, ...) head, ##__VA_ARGS__
#define NEXT() 20, 21

#if 0
# error "\-"
#endif

static int total(unsigned long count, ...) {
    __builtin_va_list ap;
    __builtin_va_start(ap, count);
    int sum = 0;
    for (unsigned long i = 0; i < count; ++i)
        sum += __builtin_va_arg(ap, int);
    __builtin_va_end(ap);
    return sum;
}

int main(void) {
    int explicit_empty[] = { COMMA(1,) 2 };
    if (sizeof(explicit_empty) != 2 * sizeof(int))
        return 1;
    if (SUM(42) != 42 || SUM(1, NEXT()) != 42 || IDENTITY(42) != 42)
        return 2;
    return WRAPPED_VALUE;
}
