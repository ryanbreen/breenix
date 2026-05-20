set pagination off
set confirm off
set architecture aarch64
set remotetimeout 10
set mem inaccessible-by-default off
set logging file /Users/wrb/fun/code/breenix/turn2-artifacts/parallels-gdb2/gdb-t38.log
set logging overwrite on
set logging enabled on
target remote 127.0.0.1:9611
printf "snapshot=t38\n"
printf "poll_count=%lu\n", *(unsigned long*)0xffff0000402cf058
printf "event_count=%lu\n", *(unsigned long*)0xffff0000402cf060
printf "msi_event_count=%lu\n", *(unsigned long*)0xffff0000402cf0a8
printf "psc_count=%lu\n", *(unsigned long*)0xffff0000402cf0a0
printf "kbd_event_count=%lu\n", *(unsigned long*)0xffff0000402cf068
printf "nkro_event_count=%lu\n", *(unsigned long*)0xffff0000402cf070
printf "xfer_other_count=%lu\n", *(unsigned long*)0xffff0000402cf088
printf "xo_err_count=%lu\n", *(unsigned long*)0xffff0000402cf098
printf "endpoint_reset_count=%lu\n", *(unsigned long*)0xffff0000402cf118
printf "endpoint_reset_fail_count=%lu\n", *(unsigned long*)0xffff0000402cf120
printf "diag_spi_enable_count=%lu\n", *(unsigned long*)0xffff0000402cf0f0
printf "spi_activated=%u\n", *(unsigned char*)0xffff0000402d66d4
printf "xhci_initialized=%u\n", *(unsigned char*)0xffff0000402cf050
printf "xhci_irq=%u\n", *(unsigned int*)0xffff0000402cf054
printf "event_ring_dequeue=%lu\n", *(unsigned long*)0xffff0000402d6710
printf "event_ring_cycle=%u\n", *(unsigned char*)0xffff0000402362e4
printf "hid_trbs_queued=%u\n", *(unsigned char*)0xffff0000402d66e8
printf "needs_reset_kbd_boot=%u\n", *(unsigned char*)0xffff0000402d66d8
printf "needs_reset_kbd_nkro=%u\n", *(unsigned char*)0xffff0000402d66dc
printf "needs_reset_mouse=%u\n", *(unsigned char*)0xffff0000402d66e0
printf "needs_reset_mouse2=%u\n", *(unsigned char*)0xffff0000402d66e4
detach
quit
