/* Independently specified checks for N1570 §§5.2.4.2, 7.9, and 7.15–7.23. */
#include <stddef.h>
#include <stdarg.h>
#include <stdint.h>
#include <limits.h>
#include <float.h>
#include <stdbool.h>
#include <stdalign.h>
#include <stdnoreturn.h>
#include <iso646.h>
#include <stdatomic.h>

struct sample {
    char tag;
    double values[2];
};

_Static_assert(CHAR_BIT == 8 && sizeof(void *) == 8, "LP64 byte and pointer");
_Static_assert(sizeof(int) == 4 && sizeof(long) == 8, "LP64 integer widths");
_Static_assert(CHAR_MIN == ((char)-1 < 0 ? -128 : 0), "plain char limits");
_Static_assert(CHAR_MAX == ((char)-1 < 0 ? 127 : 255), "plain char maximum");
_Static_assert(INT_MIN == -2147483647 - 1 && ULONG_MAX == 0xffffffffffffffffUL,
               "integer extrema");
_Static_assert(_Generic((size_t)0, unsigned long: 1, default: 0), "size_t ABI");
_Static_assert(_Generic((ptrdiff_t)0, long: 1, default: 0), "ptrdiff_t ABI");
_Static_assert(_Generic(offsetof(struct sample, values[1]), size_t: 1, default: 0),
               "offsetof result type");
_Static_assert(offsetof(struct sample, values[1]) == 16, "nested offset");
_Static_assert(alignof(max_align_t) >= alignof(long double), "fundamental alignment");
_Static_assert(__alignas_is_defined == 1 && __alignof_is_defined == 1, "alignment macros");
_Static_assert(sizeof(intptr_t) == 8 && PTRDIFF_MAX == INT64_MAX, "pointer limits");
_Static_assert(SIZE_MAX == UINT64_MAX && INT64_MIN == (-0x7fffffffffffffffL - 1),
               "64-bit extrema");
_Static_assert(_Generic(UINT8_C(1), int: 1, default: 0), "promoted uint_least8_t");
_Static_assert(_Generic(INT64_C(1), int64_t: 1, default: 0), "signed 64-bit literal");
_Static_assert(_Generic(UINT64_C(1), uint64_t: 1, default: 0), "unsigned 64-bit literal");
_Static_assert(INT_LEAST16_MIN <= -32768 && UINT_FAST32_MAX >= 0xffffffffU,
               "least and fast ranges");
_Static_assert(FLT_RADIX == 2 && FLT_MANT_DIG == 24 && DBL_MANT_DIG == 53,
               "binary32 and binary64 precision");
_Static_assert(FLT_EVAL_METHOD == 0 && LDBL_DECIMAL_DIG == DECIMAL_DIG,
               "floating evaluation and round-trip precision");
_Static_assert(_Generic(FLT_TRUE_MIN, float: 1, default: 0), "float subnormal type");
_Static_assert(_Generic(LDBL_EPSILON, long double: 1, default: 0), "long double type");
_Static_assert(true and not false and ((3 xor 1) == 2), "Boolean and alternative tokens");
_Static_assert(ATOMIC_BOOL_LOCK_FREE == 2 && ATOMIC_POINTER_LOCK_FREE == 2,
               "scalar lock-free promises");

#if defined(__ELF__)
_Static_assert(sizeof(long double) == 16 && alignof(long double) == 16,
               "ELF long double storage");
_Static_assert(alignof(max_align_t) == 16, "ELF maximum fundamental alignment");
_Static_assert(WINT_MIN == 0 && WINT_MAX == 0xffffffffU, "wint_t range");
#if defined(__x86_64__)
_Static_assert(CHAR_MIN == -128 && WCHAR_MIN == INT_MIN && WCHAR_MAX == INT_MAX,
               "AMD64 character types");
_Static_assert(LDBL_MANT_DIG == 64 && LDBL_DECIMAL_DIG == 21, "AMD64 x87 extended");
_Static_assert(sizeof(va_list) == 24, "AMD64 variadic register-save descriptor");
#elif defined(__aarch64__)
_Static_assert(CHAR_MIN == 0 && WCHAR_MIN == 0 && WCHAR_MAX == 0xffffffffU,
               "AAPCS64 character types");
_Static_assert(LDBL_MANT_DIG == 113 && LDBL_DECIMAL_DIG == 36, "AAPCS64 binary128");
_Static_assert(sizeof(va_list) == 32, "AAPCS64 variadic register-save descriptor");
#else
#error Unexpected ELF test target
#endif
#endif

noreturn void lp64_probe_termination(void);

static bool read_varargs(int tag, ...)
{
    va_list args;
    va_list copy;
    va_start(args, tag);
    const int integer = va_arg(args, int);
    va_copy(copy, args);
    const double first = va_arg(args, double);
    const double second = va_arg(copy, double);
    int *const pointer = va_arg(copy, int *);
    va_end(copy);
    va_end(args);
    return integer == 17 && first == 1.5 && second == 1.5 &&
           pointer != NULL && *pointer == tag;
}

int main(void)
{
    alignas(max_align_t) int tag = 42;
    if (!read_varargs(tag, 17, 1.5, &tag))
        return 1;

    atomic_int counter = ATOMIC_VAR_INIT(5);
    if (!atomic_is_lock_free(&counter) ||
        atomic_fetch_add_explicit(&counter, 3, memory_order_relaxed) != 5)
        return 2;

    int expected = 7;
    if (atomic_compare_exchange_strong(&counter, &expected, 10) || expected != 8 ||
        !atomic_compare_exchange_strong(&counter, &expected, 9) || atomic_load(&counter) != 9)
        return 3;

    atomic_flag flag = ATOMIC_FLAG_INIT;
    if (atomic_flag_test_and_set_explicit(&flag, memory_order_acquire) ||
        !atomic_flag_test_and_set(&flag))
        return 4;
    atomic_flag_clear_explicit(&flag, memory_order_release);
    return atomic_flag_test_and_set(&flag) ? 5 : 0;
}
