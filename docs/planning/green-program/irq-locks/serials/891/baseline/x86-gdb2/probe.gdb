set pagination off
set confirm off
add-symbol-file target/baseline-891/kernel -o 0x10000000000
target remote :18911
hbreak kernel::task::softirq_tests::test_softirq
continue
x/1wu &'kernel::task::softirq_tests::test_softirq::ITERATION_COUNT'
x/1bu &'kernel::task::softirq_tests::test_softirq::KSOFTIRQD_PROCESSED'
x/1gu &'kernel::task::scheduler::WAKE_SITE_UNBLOCK'
x/1gu &'kernel::task::scheduler::ENQUEUE_SAME_LOCK_OK'
hbreak core::panicking::panic_fmt
finish
x/1wu &'kernel::task::softirq_tests::test_softirq::ITERATION_COUNT'
x/1bu &'kernel::task::softirq_tests::test_softirq::KSOFTIRQD_PROCESSED'
x/1gu &'kernel::task::scheduler::WAKE_SITE_UNBLOCK'
x/1gu &'kernel::task::scheduler::ENQUEUE_SAME_LOCK_OK'
x/1gu (char*)&CTX_SWITCH_TOTAL+64
bt 8
detach
quit
