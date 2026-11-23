static int value;

int main(void) {
    asm ("# basic asm\n\tnop");
    value = 40;
    asm volatile ("" ::: "memory");
#if defined(__x86_64__)
    asm volatile ("add dword [%q0], 2" : : "r" (&value) : "memory", "cc");
#else
    int updated;
    asm volatile ("ldr %w0, [%x1]\n\tadd %w0, %w0, #2\n\tstr %w0, [%x1]"
                  : "=r" (updated) : "r" (&value) : "memory");
#endif
    return value;
}
