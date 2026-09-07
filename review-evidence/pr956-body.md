The redo spends the wake budget in guest execution and permits bounded extensions only for measured starvation in the window just spent. Initial wake/dispatch failures remain failures, and the strict scorer requires the budget fields and rejects loopback FAIL.

R233 accepts this implementation for LANDING, subject to the requested merged-tip gates. Issue 586 stays OPEN and its battery roster line does NOT retire on this merge. Issue 954 closes on this merge: R232 replaces the impossible green permanent-suppression requirement with the recorded delayed-wake control.

The delayed-wake triad in docs/planning/green-program/network/586-PR3-2026-09-07.md is: measured guard, wake FAIL with 0 extensions; unconditional grants, PASS with 3 extensions and 600 ms; production-guard ratchet, red with exit 101. Permanent suppression remains red under both policies. The mutated PASS is not a genuine starvation receipt.

NOT-CLAIMED: No genuine verdict=starved extensions>0 PASS receipt was obtained.

The extension-deleted starvation red also remains unreproduced. Issue 955 tracks the separate UDP oracle failure; issues 945 and 559 retain the stock-toolchain work. Historical gates, raw serials, the lane-local core patch, both R182 recordings, and limitations are recorded in docs/planning/green-program/network/586-PR3-2026-09-07.md. Fresh landing gates are pending.
