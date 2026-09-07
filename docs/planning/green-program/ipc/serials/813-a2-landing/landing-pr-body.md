Small pipe/FIFO writev requests now gather aggregates up to PIPE_BUF into one transfer; prepared-wait cleanup retains the original thread identity, and the oracle exercises competing writev writers and FIFO no-writer nonblocking EOF. This also carries scoped formatting and the corrected four-caller census for issue 919.

V-1 is corrected inline in docs/planning/green-program/ipc/813-A2-2026-09-07.md: three historical gate logs name implementation 26893a4f, structure.log names base 19427ef with candidate tests, and scoped-fmt.log is empty.

Landing at merged revision c2ff822d: standalone structure 103/103; strict preflight 64/64; ARM strict 1/1 boot, exit 0. A subsequent wrapping-only formatter adjustment passes the seven-file scoped check. Results are recorded in the round document and serials/813-a2-landing. Earlier ARM oracle results remain 3/3 boots with 30/30 arms each.

Deferred V-2: mandatory check 5 expects cargo fmt --check -p kernel to exit 0; whole-kernel formatting remains unverified, with issue 938 retaining the P-13 roster concern. Not claimed: x86 runtime acceptance (927, 937), parent 813 closure, public ARM mkfifo, production-profile acceptance, or general fault-cleanup lock/saturation safety (936). The ARM build retains the previously authorized 559 notice.
