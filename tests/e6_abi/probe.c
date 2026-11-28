#include <stdarg.h>
#include <stdint.h>

struct mixed { double fp; uint32_t integer; };
struct hfa { float x, y, z; };

extern double rust_fp(float, double, struct mixed);
extern struct hfa rust_hfa(struct hfa);
extern double rust_var(int, ...);
extern double rust_caller(void);
#if defined(__aarch64__)
extern uint64_t rust_packed(const uint8_t *);
extern uint64_t rust_atomic(const uint64_t *);
extern uint64_t rust_device(const uint64_t *);
#endif

double c_fp(float a, double b, struct mixed m) {
    return a + b + m.fp + m.integer;
}

struct hfa c_hfa(struct hfa h) {
    return (struct hfa){h.z, h.x, h.y};
}

double c_var(int tag, ...) {
    va_list ap;
    va_start(ap, tag);
    double sum = tag;
    for (int i = 1; i <= 10; ++i) {
        sum += i * va_arg(ap, double);
        sum += i * va_arg(ap, int);
    }
    va_end(ap);
    return sum;
}

#if defined(__x86_64__)
__attribute__((noinline)) long double c_long_double(unsigned input) {
    return input + 0.5L;
}
#endif

int check(void) {
    if (rust_fp(1.5f, 2.5, (struct mixed){4.25, 8}) != 16.25)
        return 1;
    struct hfa h = rust_hfa((struct hfa){1.25f, 2.5f, 3.75f});
    if (h.x != 3.75f || h.y != 1.25f || h.z != 2.5f)
        return 2;
    if (rust_var(7, 1.0, 11, 2.0, 12, 3.0, 13, 4.0, 14, 5.0, 15,
                6.0, 16, 7.0, 17, 8.0, 18, 9.0, 19, 10.0, 20) != 1327.0)
        return 3;
    if (rust_caller() != 1343.25)
        return 4;
#if defined(__x86_64__)
    volatile unsigned input = 7;
    if (c_long_double(input) != 7.5L)
        return 5;
#else
    _Alignas(8) static const uint8_t bytes[] = {0, 1, 2, 3, 4, 5, 6, 7, 8};
    _Alignas(8) static const uint64_t aligned = 0x0123456789abcdef;
    if (rust_packed(bytes) != UINT64_C(0x0807060504030201))
        return 6;
    if (rust_atomic(&aligned) != aligned || rust_device(&aligned) != aligned)
        return 7;
#endif
    return 0;
}
