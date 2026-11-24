static int calls;

static int next(void) {
    return ++calls + 19;
}

int main(void) {
    int result;
    __asm__ __volatile__ ("# tied operand" : "=r" (result) : "0" (next()));
    int values[2] = {0, 0};
    int index = 0;
    values[0] = result;
    asm (
#if defined(__x86_64__)
        "add %k0, %k1"
#else
        "add %w0, %w0, %w1"
#endif
        : "+r" (values[index++]) : "r" (22) : "cc"
    );
    return calls == 1 && index == 1 ? values[0] : 1;
}
