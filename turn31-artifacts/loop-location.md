# Turn 31 Loop Location

Function: `send_ipv4()` in `kernel/src/net/mod.rs`.

Pre-edit line range: `kernel/src/net/mod.rs:638-665`.

The synchronous on-demand ARP polling loop starts after an ARP cache miss at line 638. It sends an ARP request and then runs a `for _ in 0..50` loop at lines 650-664. Each iteration calls `process_rx()` and then spins `0..500_000` with `core::hint::spin_loop()` before checking `arp::lookup(&next_hop)`. On success it logs `NET: On-demand ARP resolved gateway MAC` and sends the packet immediately from inside the loop. On exhaustion it returns `Err("ARP lookup failed - gateway did not respond")`.
