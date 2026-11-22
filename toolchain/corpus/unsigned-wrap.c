/* N1570 6.2.5 paragraph 9 and 7.20.2.1, arithmetic modulo 2^32. */
#include <stdint.h>

int main(void)
{
    uint32_t value = UINT32_MAX;
    value += 3;
    return value == 2 ? 0 : 1;
}
