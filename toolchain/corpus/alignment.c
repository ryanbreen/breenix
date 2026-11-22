/* N1570 6.5.3.4, 6.7.5, 7.15 and 7.19, requested alignment and member offset. */
#include <stdalign.h>
#include <stddef.h>
#include <stdint.h>

struct slot { char tag; alignas(16) unsigned data; };
_Static_assert(alignof(struct slot) >= 16, "member sets structure alignment");
_Static_assert(offsetof(struct slot, data) % 16 == 0, "member offset");

int main(void)
{
    struct slot item = {0, 42};
    return (uintptr_t)&item.data % 16 == 0 && item.data == 42 ? 0 : 1;
}
