# Carrier and DHCP lease continuity

The runtime BCM5720 worker treats an MDIO read error as unknown carrier.
`phy_link()` retains its two BMSR reads: Clause 22's latch-low bit records a
past loss on the first read; the second supplies current link status. A read
error breaks the down streak and retains the last carrier state.

Down requires three successful down samples. At the worker's minimum one-second
spacing, those observations span at least two seconds. Clause 28's
`break_link_timer` is 1200–1500 ms; two observations span only one second and
can both fall inside that autonegotiation break. Three span beyond it. This is
a debounce policy; negotiation may take longer than two seconds.
The [IEEE Auto-Negotiation timing table](https://ieee802.org/3/1TPCESG/public/Lo_02_0514.pdf)
lists the break interval for 1000BASE-T. A successful up sample takes effect
immediately.

The DHCP lifecycle follows [RFC 2131](https://www.rfc-editor.org/rfc/rfc2131.html),
sections 3.7, 4.1, 4.4.2 and 4.4.5:

- Carrier loss keeps the unexpired address and its original deadlines.
- Carrier return broadcasts INIT-REBOOT DHCPREQUEST: source=0 and ciaddr=0,
  requested-address option present, server-identifier option absent. The cached
  address remains configured during revalidation; it does not run DISCOVER or
  a fresh address probe. Revalidation sends at most seven requests. Retry spacing
  doubles from approximately 4 seconds, with +/-1 second jitter and a 64-second
  cap. After unanswered revalidation it retains the original lease and resumes
  the normal T1 renewal schedule. T1 may end revalidation earlier.
- T1 requests unicast renewal with ciaddr set; T2 requests broadcast rebinding.
  Both clear the BOOTP BROADCAST flag because the configured client can receive
  unicast replies.
  Neither removes the address. Retry waits stop at the next state deadline,
  including when that deadline passed during the preceding exchange.
- NAK or expiry removes the lease and clears ARP. Readers enforce expiry
  independently of the maintenance worker. Acquisition then uses DISCOVER,
  REQUEST and the existing conflict probe.

Worker-context records include carrier transitions, failed-sample streaks and
their recovery, lease state changes, drops and acquisitions. Retries do not emit log lines.
Lease records include the address and monotonic T1, T2 and expiry deadlines.
`BCM5720_PHY_READ_FAILED` in `/proc/trace/counters` counts runtime carrier
sample errors, including each sample in a streak. Logs are emitted after
releasing network and port guards; no interrupt path gains logging.

`tests/bcm5720_carrier.rs` exercises the production debounce.
`tests/dhcp_client.rs` drives the production DHCP client with a deterministic
wire peer and clock, covering renewal, rebinding, expiry, NAK, retained-link
revalidation, retry spacing and caps, the renew/rebind 60-second minimum,
INIT-REBOOT source address, and reacquisition after expiry. Its old carrier-discard assertion
is replaced by retained-address behavior as required by this change's brief;
the procfs no-address case now expires the lease to establish its prerequisite.

The earlier SSH timeout's carrier trigger remains a hypothesis: its capture
contained no lease lifecycle records. Hardware evidence must distinguish a
quiet successful session from an observed PHY failure or lease event; absence
of a repeated timeout alone does not establish the old occurrence's cause.
