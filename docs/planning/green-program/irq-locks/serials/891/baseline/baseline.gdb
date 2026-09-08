set pagination off
set confirm off
set language c
file target/aarch64-breenix-kernel/release/kernel-aarch64
target remote :18910
break kernel/src/task/softirq_tests.rs:200
continue
info threads
p 'kernel::task::softirq_tests::test_softirq::ITERATION_COUNT'
p 'kernel::task::softirq_tests::test_softirq::KSOFTIRQD_PROCESSED'
p 'kernel::task::scheduler::WAKE_SITE_UNBLOCK'
p 'kernel::task::scheduler::ENQUEUE_SAME_LOCK_OK'
x/1gu (char*)&CTX_SWITCH_TOTAL+64
x/1gu (char*)&CTX_SWITCH_TOTAL+128
x/1gu (char*)&CTX_SWITCH_TOTAL+192
x/1gu (char*)&CTX_SWITCH_TOTAL+256
thread apply all bt 5
p 'kernel::task::softirqd::KSOFTIRQD'
break kernel/src/task/softirq_tests.rs:228
continue
info threads
p 'kernel::task::softirq_tests::test_softirq::ITERATION_COUNT'
p 'kernel::task::softirq_tests::test_softirq::KSOFTIRQD_PROCESSED'
p 'kernel::task::scheduler::WAKE_SITE_UNBLOCK'
p 'kernel::task::scheduler::ENQUEUE_SAME_LOCK_OK'
x/1gu (char*)&CTX_SWITCH_TOTAL+64
x/1gu (char*)&CTX_SWITCH_TOTAL+128
x/1gu (char*)&CTX_SWITCH_TOTAL+192
x/1gu (char*)&CTX_SWITCH_TOTAL+256
thread apply all bt 5
p 'kernel::task::softirqd::KSOFTIRQD'
detach
quit
