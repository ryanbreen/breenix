/* N1570 6.5.1.1, 6.5.2.2p6, 6.7.2.1p13, 6.7.4, 6.7.5, 6.7.10 and 6.9.1. */
#include <stdint.h>
#include <stdio.h>

/* N1570 7.1.4p2 and 7.22.4.4. */
_Noreturn void exit(int);

struct slot {
    char tag;
    union {
        struct { _Alignas(32) int value; };
        long other;
    };
    _Static_assert(_Alignof(int) <= 32, "member declaration");
};

_Static_assert(_Alignof(struct slot) == 32, "file declaration");
static _Alignas(32) int file_value;

static _Noreturn void fail(void)
{
    exit(1);
}

static int next(int *calls)
{
    return ++*calls;
}

static int promoted();
static int promoted(n, fraction)
    short n;
    float fraction;
{
    return n + (int)(fraction * 2);
}

int main(void)
{
    _Static_assert(_Alignof(struct slot[2]) == 32, "block declaration");
    struct slot item = { .value = 7 };
    _Alignas(struct slot) int local = 9;
    int calls = 0;
    _Generic(next(&calls), int: item.value, default: local) += local;
    int result = _Generic(1.0, int: next(&calls), double: promoted((short)2, 1.5f));
    if ((uintptr_t)&item.value % 32 || (uintptr_t)&local % 32 ||
        (uintptr_t)&file_value % 32 || calls != 0)
        fail();
    printf("%d %d %d\n", item.value, local, result);
    return 0;
}
