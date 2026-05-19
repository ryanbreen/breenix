set pagination off
set confirm off
set architecture aarch64
set remotetimeout 10
set mem inaccessible-by-default off
set logging file /Users/wrb/fun/code/breenix.worktrees/ahci-interrupt-driven/turn5-artifacts/gdb-endpoint-state.log
set logging overwrite on
set logging enabled on

echo === TURN5 AHCI level-SPI ENDPOINT STATE ===\n
target remote 127.0.0.1:9600

set $AHCI_IRQ = 0xffff000041201f80
set $AHCI_ISR_COUNT = 0xffff00004121f230
set $AHCI_ISR_LAST_MPIDR = 0xffff000040233148
set $AHCI_POLLED_COMPLETION_COUNT = 0xffff000041201f84
set $TIMER_TICK_COUNT = 0xffff000041b7efd8
set $TIMER_TICK_HW_COUNT = 0xffff000041b7f018
set $TIMER_INTERRUPT_COUNT = 0xffff000041b7f180

printf "ahci_irq=%u\n", *(unsigned int*)$AHCI_IRQ
printf "ahci_isr_count=%u\n", *(unsigned int*)$AHCI_ISR_COUNT
printf "ahci_isr_last_mpidr_aff0=%lu\n", *(unsigned long*)$AHCI_ISR_LAST_MPIDR
printf "ahci_polled_completion_count=%u\n", *(unsigned int*)$AHCI_POLLED_COMPLETION_COUNT
printf "timer_tick_count_cpu0=%lu\n", *(unsigned long*)($TIMER_TICK_COUNT + 0)
printf "timer_tick_count_cpu1=%lu\n", *(unsigned long*)($TIMER_TICK_COUNT + 8)
printf "timer_tick_count_cpu2=%lu\n", *(unsigned long*)($TIMER_TICK_COUNT + 16)
printf "timer_tick_count_cpu3=%lu\n", *(unsigned long*)($TIMER_TICK_COUNT + 24)
printf "timer_tick_hw_count_cpu0=%lu\n", *(unsigned long*)($TIMER_TICK_HW_COUNT + 0)
printf "timer_tick_hw_count_cpu1=%lu\n", *(unsigned long*)($TIMER_TICK_HW_COUNT + 8)
printf "timer_tick_hw_count_cpu2=%lu\n", *(unsigned long*)($TIMER_TICK_HW_COUNT + 16)
printf "timer_tick_hw_count_cpu3=%lu\n", *(unsigned long*)($TIMER_TICK_HW_COUNT + 24)
printf "timer_interrupt_count=%lu\n", *(unsigned long*)$TIMER_INTERRUPT_COUNT
detach
quit
