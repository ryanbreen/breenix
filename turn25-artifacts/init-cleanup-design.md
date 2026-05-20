# Turn 25 init cleanup design

Substep 4 deletes only the synchronous boot-time waits in `init_common()`: the
100-iteration gateway ARP `process_rx()`/spin loop and the ICMP echo health
check plus its 20-iteration `process_rx()`/spin loop. The gateway ARP request is
still sent once as an asynchronous cache-prime, but `net::init()` no longer
waits for any inbound packet before returning. The on-demand ARP loop in
`send_ipv4()` is intentionally left for Substep 5.

The new aarch64 sequence is: driver probe has already set up queues, `net::init`
registers NetRx, `init_common()` initializes the ARP cache, then arms PCI
MSI-X/MMIO network IRQs before sending the gateway ARP request. That means a
gateway reply is handled by the normal IRQ -> NetRx softirq path instead of by
init polling. Driver comments are updated to describe this earlier enablement
because the old comments explicitly documented the stale "enable after ARP/ICMP
polling" contract.
