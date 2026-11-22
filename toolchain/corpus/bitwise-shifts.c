/* N1570 6.5.7 and 6.5.10–6.5.12, unsigned shifts and masks. */
#include <stdio.h>

int main(void)
{
    unsigned word = (5U << 4) | 3U;
    printf("%u %u\n", (word ^ 7U) & 15U, word >> 4);
    return 0;
}
