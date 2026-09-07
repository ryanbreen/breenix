set pagination off
set confirm off
set language c
file /root/breenix-927a/target/x86_64-unknown-none/release/deps/artifact/kernel-53a4eb09c66a2845/bin/kernel-53a4eb09c66a2845
add-symbol-file /root/breenix-927a/target/x86_64-unknown-none/release/deps/artifact/kernel-53a4eb09c66a2845/bin/kernel-53a4eb09c66a2845 -o 0x10000000000
target remote 127.0.0.1:1927
hbreak *0x1000025aa80
continue
delete 1
hbreak *0x100000de870
continue
printf "FIRST COHORT DRAIN\n"
bt 5
printf "PT_ROOTS_RETIRED="
x/gd 0x1000040ce80
printf "TEARDOWN_ENTRY_EXIT="
x/gd 0x100003fd800
printf "PT_RETIRE_BUDGET_REQUEUED="
x/gd 0x1000040df80
printf "PT_RETIRE_FRAMES_LOST="
x/gd 0x1000040d700
printf "kernel::task::process_task::BOOT_RECLAIM_TEST_OWNER="
x/gd 0x10000414d68
printf "kernel::task::process_task::BOOT_RECLAIM_PASS_START="
x/gd 0x10000414d88
printf "kernel::task::process_task::BOOT_RECLAIM_PASS_SELECTIONS="
x/gd 0x10000414d90
delete 2
detach
quit
