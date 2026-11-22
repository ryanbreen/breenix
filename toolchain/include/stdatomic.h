/* N1570 §7.17. Stage-0 spellings from Clang 22.1 Language Extensions. */
#ifndef BREENIX_TOOLCHAIN_STDATOMIC_H
#define BREENIX_TOOLCHAIN_STDATOMIC_H

#include <stddef.h>
#include <stdint.h>

/* The covered scalar types use naturally aligned 1-, 2-, 4-, or 8-byte
 * lock-free operations on both stage-0 targets. */
#define ATOMIC_BOOL_LOCK_FREE 2
#define ATOMIC_CHAR_LOCK_FREE 2
#define ATOMIC_CHAR16_T_LOCK_FREE 2
#define ATOMIC_CHAR32_T_LOCK_FREE 2
#define ATOMIC_WCHAR_T_LOCK_FREE 2
#define ATOMIC_SHORT_LOCK_FREE 2
#define ATOMIC_INT_LOCK_FREE 2
#define ATOMIC_LONG_LOCK_FREE 2
#define ATOMIC_LLONG_LOCK_FREE 2
#define ATOMIC_POINTER_LOCK_FREE 2

typedef enum {
    memory_order_relaxed = __ATOMIC_RELAXED,
    memory_order_consume = __ATOMIC_CONSUME,
    memory_order_acquire = __ATOMIC_ACQUIRE,
    memory_order_release = __ATOMIC_RELEASE,
    memory_order_acq_rel = __ATOMIC_ACQ_REL,
    memory_order_seq_cst = __ATOMIC_SEQ_CST
} memory_order;

typedef _Atomic(_Bool) atomic_bool;
typedef _Atomic(char) atomic_char;
typedef _Atomic(signed char) atomic_schar;
typedef _Atomic(unsigned char) atomic_uchar;
typedef _Atomic(short) atomic_short;
typedef _Atomic(unsigned short) atomic_ushort;
typedef _Atomic(int) atomic_int;
typedef _Atomic(unsigned int) atomic_uint;
typedef _Atomic(long) atomic_long;
typedef _Atomic(unsigned long) atomic_ulong;
typedef _Atomic(long long) atomic_llong;
typedef _Atomic(unsigned long long) atomic_ullong;

typedef _Atomic(uint_least16_t) atomic_char16_t;
typedef _Atomic(uint_least32_t) atomic_char32_t;
typedef _Atomic(wchar_t) atomic_wchar_t;
typedef _Atomic(int_least8_t) atomic_int_least8_t;
typedef _Atomic(uint_least8_t) atomic_uint_least8_t;
typedef _Atomic(int_least16_t) atomic_int_least16_t;
typedef _Atomic(uint_least16_t) atomic_uint_least16_t;
typedef _Atomic(int_least32_t) atomic_int_least32_t;
typedef _Atomic(uint_least32_t) atomic_uint_least32_t;
typedef _Atomic(int_least64_t) atomic_int_least64_t;
typedef _Atomic(uint_least64_t) atomic_uint_least64_t;
typedef _Atomic(int_fast8_t) atomic_int_fast8_t;
typedef _Atomic(uint_fast8_t) atomic_uint_fast8_t;
typedef _Atomic(int_fast16_t) atomic_int_fast16_t;
typedef _Atomic(uint_fast16_t) atomic_uint_fast16_t;
typedef _Atomic(int_fast32_t) atomic_int_fast32_t;
typedef _Atomic(uint_fast32_t) atomic_uint_fast32_t;
typedef _Atomic(int_fast64_t) atomic_int_fast64_t;
typedef _Atomic(uint_fast64_t) atomic_uint_fast64_t;
typedef _Atomic(intptr_t) atomic_intptr_t;
typedef _Atomic(uintptr_t) atomic_uintptr_t;
typedef _Atomic(size_t) atomic_size_t;
typedef _Atomic(ptrdiff_t) atomic_ptrdiff_t;
typedef _Atomic(intmax_t) atomic_intmax_t;
typedef _Atomic(uintmax_t) atomic_uintmax_t;

#define ATOMIC_VAR_INIT(value) value
#define atomic_init(object, value) __c11_atomic_init((object), (value))

/* Stage 0 treats consume as acquire, so dropping the dependency adds no fence. */
#define kill_dependency(value) (value)

static inline void atomic_thread_fence(memory_order order)
{
    __c11_atomic_thread_fence(order);
}

static inline void atomic_signal_fence(memory_order order)
{
    __c11_atomic_signal_fence(order);
}

/* The builtin queries the atomic type's size. Evaluate the pointer once. */
#define atomic_is_lock_free(object) \
    ((void)(object), __c11_atomic_is_lock_free(sizeof *(object)))

#define atomic_store_explicit(object, value, order) \
    __c11_atomic_store((object), (value), (order))
#define atomic_load_explicit(object, order) \
    __c11_atomic_load((object), (order))
#define atomic_exchange_explicit(object, value, order) \
    __c11_atomic_exchange((object), (value), (order))
#define atomic_compare_exchange_strong_explicit(object, expected, desired, success, failure) \
    __c11_atomic_compare_exchange_strong((object), (expected), (desired), (success), (failure))
#define atomic_compare_exchange_weak_explicit(object, expected, desired, success, failure) \
    __c11_atomic_compare_exchange_weak((object), (expected), (desired), (success), (failure))

#define atomic_fetch_add_explicit(object, value, order) \
    __c11_atomic_fetch_add((object), (value), (order))
#define atomic_fetch_sub_explicit(object, value, order) \
    __c11_atomic_fetch_sub((object), (value), (order))
#define atomic_fetch_or_explicit(object, value, order) \
    __c11_atomic_fetch_or((object), (value), (order))
#define atomic_fetch_xor_explicit(object, value, order) \
    __c11_atomic_fetch_xor((object), (value), (order))
#define atomic_fetch_and_explicit(object, value, order) \
    __c11_atomic_fetch_and((object), (value), (order))

#define atomic_store(object, value) atomic_store_explicit(object, value, memory_order_seq_cst)
#define atomic_load(object) atomic_load_explicit(object, memory_order_seq_cst)
#define atomic_exchange(object, value) atomic_exchange_explicit(object, value, memory_order_seq_cst)
#define atomic_compare_exchange_strong(object, expected, desired) \
    atomic_compare_exchange_strong_explicit(object, expected, desired, \
                                            memory_order_seq_cst, memory_order_seq_cst)
#define atomic_compare_exchange_weak(object, expected, desired) \
    atomic_compare_exchange_weak_explicit(object, expected, desired, \
                                          memory_order_seq_cst, memory_order_seq_cst)

#define atomic_fetch_add(object, value) atomic_fetch_add_explicit(object, value, memory_order_seq_cst)
#define atomic_fetch_sub(object, value) atomic_fetch_sub_explicit(object, value, memory_order_seq_cst)
#define atomic_fetch_or(object, value) atomic_fetch_or_explicit(object, value, memory_order_seq_cst)
#define atomic_fetch_xor(object, value) atomic_fetch_xor_explicit(object, value, memory_order_seq_cst)
#define atomic_fetch_and(object, value) atomic_fetch_and_explicit(object, value, memory_order_seq_cst)

typedef struct {
    atomic_bool __set;
} atomic_flag;

#define ATOMIC_FLAG_INIT { 0 }

static inline _Bool atomic_flag_test_and_set_explicit(volatile atomic_flag *object,
                                                     memory_order order)
{
    return atomic_exchange_explicit(&object->__set, 1, order);
}

static inline void atomic_flag_clear_explicit(volatile atomic_flag *object, memory_order order)
{
    atomic_store_explicit(&object->__set, 0, order);
}

static inline _Bool atomic_flag_test_and_set(volatile atomic_flag *object)
{
    return atomic_flag_test_and_set_explicit(object, memory_order_seq_cst);
}

static inline void atomic_flag_clear(volatile atomic_flag *object)
{
    atomic_flag_clear_explicit(object, memory_order_seq_cst);
}

#endif
