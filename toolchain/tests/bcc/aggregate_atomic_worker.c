/* N1570 6.3.2.1, 6.5.16.1 and 7.17.7. Test input compiled by bcc. */
#include <stdatomic.h>

struct Counts { unsigned long count, sum, inverse; };
static _Atomic(struct Counts) shared;
_Static_assert(sizeof shared == 24, "three LP64 words");

struct __attribute__((packed)) Packed {
    char before;
    atomic_uint value;
    char after;
};
_Alignas(8) static struct Packed packed;

struct Small { unsigned int value; };
static _Atomic(struct Small) small;

int initialize(void) {
    struct Counts zero = {0, 0, ~0UL};
    atomic_init(&shared, zero);
    struct Counts seed = {1, 3, ~1UL};
    shared = seed;
    struct Counts old = atomic_exchange_explicit(&shared, zero, memory_order_acq_rel);
    volatile unsigned long bytes = sizeof shared;
    if (__c11_atomic_is_lock_free(bytes) || !__c11_atomic_is_lock_free(8) ||
        atomic_is_lock_free(&shared) || old.count != 1 ||
        old.sum != 3 || old.inverse != ~1UL)
        return 1;

    packed.before = 11;
    packed.after = 22;
    atomic_uint *ptr = &packed.value;
    atomic_init(ptr, 4);
    if (atomic_is_lock_free(ptr) || atomic_fetch_add(ptr, 2) != 4 ||
        packed.value++ != 6 || (packed.value += 5) != 12 ||
        atomic_exchange(ptr, 7) != 12 ||
        atomic_load(ptr) != 7 || packed.before != 11 || packed.after != 22)
        return 2;

    struct Small next = {19};
    atomic_store(&small, next);
    struct Small expected = {18};
    if (!atomic_is_lock_free(&small) ||
        atomic_compare_exchange_strong(&small, &expected, next) ||
        expected.value != 19)
        return 3;
    next.value = 42;
    if (!atomic_compare_exchange_strong(&small, &expected, next))
        return 4;
    struct Small got = small;
    return got.value != 42;
}

int worker(unsigned int iterations) {
    for (unsigned int i = 0; i < iterations; ++i) {
        struct Counts expected = atomic_load_explicit(&shared, memory_order_acquire);
        unsigned int attempts = 0;
        for (;;) {
            if (expected.sum != 3 * expected.count || expected.inverse != ~expected.count)
                return 1;
            struct Counts desired = {expected.count + 1, expected.sum + 3, ~(expected.count + 1)};
            if (atomic_compare_exchange_weak_explicit(&shared, &expected, desired,
                                                      memory_order_acq_rel, memory_order_acquire))
                break;
            if (++attempts == 100000)
                return 2;
        }
    }
    return 0;
}

int verify(unsigned long count) {
    struct Counts observed = shared;
    return observed.count != count || observed.sum != 3 * count || observed.inverse != ~count;
}
