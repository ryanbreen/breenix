#if defined(USE_ALLOCA_HEADER)
#include <alloca.h>
#else
void *alloca(unsigned long);
#endif

static int store(unsigned char *bytes, unsigned long index, int value) {
    bytes[index] = value;
    return bytes[index];
}

int main(void) {
    volatile unsigned long size = 65;
    unsigned char *saved;
    {
        saved = alloca(size);
        saved[64] = 19;
    }
    unsigned char *second = alloca(size);
    return saved[64] + store(second, 64, 23) != 42 || saved[64] != 19;
}
