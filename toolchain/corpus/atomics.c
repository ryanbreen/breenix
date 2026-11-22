/* N1570 7.17.3 and 7.17.7–7.17.8, fetch, compare-exchange and flag operations. */
#include <stdatomic.h>
#include <stdio.h>

int main(void)
{
    atomic_uint value;
    atomic_flag flag = ATOMIC_FLAG_INIT;
    atomic_init(&value, 4U);
    unsigned old = atomic_fetch_add_explicit(&value, 5U, memory_order_relaxed);
    unsigned expected = 4U;
    int miss = atomic_compare_exchange_strong(&value, &expected, 13U);
    unsigned observed = expected;
    int hit = atomic_compare_exchange_strong(&value, &expected, 13U);
    int first = atomic_flag_test_and_set_explicit(&flag, memory_order_acquire);
    int second = atomic_flag_test_and_set_explicit(&flag, memory_order_acquire);
    atomic_flag_clear_explicit(&flag, memory_order_release);
    int cleared = atomic_flag_test_and_set(&flag);
    unsigned end = atomic_load_explicit(&value, memory_order_acquire);
    printf("%u %d %u %d %d %d %d %u\n", old, miss, observed, hit, first, second, cleared, end);
    return 0;
}
