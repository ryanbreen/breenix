/* Raw architectural CPUID facts; no inferred timer or interrupt policy. */
#include <stdint.h>
#include <stdio.h>

static uint32_t leaf(uint32_t function, uint32_t subleaf)
{
    uint32_t a, b, c, d;
    __asm__ volatile("cpuid" : "=a"(a), "=b"(b), "=c"(c), "=d"(d)
                     : "a"(function), "c"(subleaf));
    printf("%08x %08x: eax=%08x ebx=%08x ecx=%08x edx=%08x\n",
           function, subleaf, a, b, c, d);
    return a;
}

int main(void)
{
    uint32_t maximum = leaf(0, 0);
    const uint32_t functions[] = {1, 7, 0x15, 0x16};
    for (unsigned i = 0; i < sizeof(functions) / sizeof(functions[0]); ++i) {
        if (functions[i] <= maximum)
            leaf(functions[i], 0);
    }
    if (leaf(0x80000000, 0) >= 0x80000007)
        leaf(0x80000007, 0);
    return 0;
}
