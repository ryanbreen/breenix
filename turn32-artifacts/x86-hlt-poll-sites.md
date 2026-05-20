# Turn 32 x86 hlt-loop polling audit

All three `net::process_rx()` calls in `kernel/src/main.rs` are x86-only
test-mode idle-loop NIC polling workarounds. The nearby `net::drain_loopback_queue()`
calls are loopback-specific drains and should be kept.

## Site 1

- Function: `dns_test_only_main()`
- Pre-edit range: `kernel/src/main.rs:805-815`
- Context: idle loop after `x86_64::instructions::interrupts::enable_and_hlt()`
  and `task::scheduler::yield_current()`.
- Stale comment: `Poll for received packets (workaround for softirq timing)`.
- Decision: delete `net::process_rx()` and the stale comment. Keep
  `net::drain_loopback_queue()` because it drains localhost packets, not NIC RX.

## Site 2

- Function: `blocking_recv_test_main()`
- Pre-edit range: `kernel/src/main.rs:871-881`
- Context: idle loop after `x86_64::instructions::interrupts::enable_and_hlt()`
  and `task::scheduler::yield_current()`.
- Stale comment: `Poll for received packets (workaround for softirq timing)`.
- Decision: delete `net::process_rx()` and the stale comment. Keep
  `net::drain_loopback_queue()` because it drains localhost packets, not NIC RX.

## Site 3

- Function: `nonblock_eagain_test_main()`
- Pre-edit range: `kernel/src/main.rs:940-950`
- Context: idle loop after `x86_64::instructions::interrupts::enable_and_hlt()`
  and `task::scheduler::yield_current()`.
- Stale comment: `Poll for received packets (workaround for softirq timing)`.
- Decision: delete `net::process_rx()` and the stale comment. Keep
  `net::drain_loopback_queue()` because it drains localhost packets, not NIC RX.
