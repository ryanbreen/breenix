set pagination off
set confirm off
set language c
file /root/breenix-927a/target/x86_64-unknown-none/release/deps/artifact/kernel-53a4eb09c66a2845/bin/kernel-53a4eb09c66a2845
add-symbol-file /root/breenix-927a/target/x86_64-unknown-none/release/deps/artifact/kernel-53a4eb09c66a2845/bin/kernel-53a4eb09c66a2845 -o 0x10000000000
target remote 127.0.0.1:1927
info registers rip rsp rax rdi rsi
bt 10
printf "PT_ROOTS_RETIRED="
x/gd 0x1000040ce80
printf "TEARDOWN_ENTRY_EXIT="
x/gd 0x100003fd800
printf "PT_RETIRE_BUDGET_REQUEUED="
x/gd 0x1000040df80
printf "PT_RETIRE_FRAMES_LOST="
x/gd 0x1000040d700
printf "RECLAIM_CONTEXT_VIOLATIONS="
x/gd 0x10000401c00
detach
quit
