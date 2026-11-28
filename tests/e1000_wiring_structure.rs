//! How x86-64 kernel initialization wires the e1000 into interrupts and the
//! network stack. This is boot-order code no host test executes, and x86 QEMU
//! boots are refused (docs/x86-execution-policy.md), so the checks read the
//! source: each names the defect it guards against.

use std::fs;

fn read(path: &str) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

/// The text from `start` up to the next `end` after it.
fn section<'a>(text: &'a str, start: &str, end: &str) -> &'a str {
    let from = text.find(start).unwrap_or_else(|| panic!("missing {start:?}"));
    let rest = &text[from..];
    let to = rest[start.len()..].find(end).map(|at| at + start.len()).unwrap_or(rest.len());
    &rest[..to]
}

/// Without MSI, the e1000's INTx fallback unmasked IRQ 10 whatever line the
/// function's Interrupt Line register named, so a function on IRQ 11 took no
/// interrupt until a later, unrelated unmask.
#[test]
fn intx_fallback_unmasks_the_line_the_function_names() {
    let drivers = read("kernel/src/drivers/mod.rs");
    let fallback = section(&drivers, "match e1000::init() {", "// Broadcom BCM5720");
    assert!(!fallback.contains("enable_irq10()"), "the e1000 fallback assumes IRQ 10:\n{fallback}");
    assert!(fallback.contains("enable_network_intx(e1000::irq_line())"), "{fallback}");
    let helper = section(&drivers, "fn enable_network_intx(", "\n}\n");
    assert!(helper.contains("Some(10) => crate::interrupts::enable_irq10()"), "{helper}");
    assert!(helper.contains("Some(11) => crate::interrupts::enable_irq11()"), "{helper}");
    assert!(helper.contains("pci_intx_routing_known()"), "unknown routing stays masked:\n{helper}");

    let driver = read("kernel/src/drivers/e1000/mod.rs");
    let irq_line = section(&driver, "pub fn irq_line()", "\n}\n");
    assert!(irq_line.contains(".interrupt_line"), "{irq_line}");
}

/// On x86 without fw_cfg the stack waits for DHCP, but only the ARM64 e1000
/// poller and the BCM5720 worker called `dhcp::maintain`, so an x86 e1000 stayed
/// unconfigured. And a receive raised before `net::init` registered NetRx found
/// no handler; with no send to drain it, the frame waited for later traffic.
#[test]
fn x86_e1000_has_a_dhcp_worker_and_an_early_drain() {
    let net = read("kernel/src/net/mod.rs");
    assert!(!net.contains("#[cfg(target_arch = \"aarch64\")]\nmod rx_poll;"), "rx_poll is ARM64 only");
    assert!(!net.contains("#[cfg(target_arch = \"aarch64\")]\npub use rx_poll::init_rx_poll;"));

    let worker = read("kernel/src/net/rx_poll.rs");
    let x86 = section(&worker, "#[cfg(target_arch = \"x86_64\")]\nfn dhcp_worker_fn()", "\n}\n");
    assert!(x86.contains("dhcp::maintain(mac, e1000::link_up())"), "{x86}");
    let start = section(&worker, "pub fn init_rx_poll()", "\n}\n");
    assert!(start.contains("dhcp_worker_fn"), "{start}");
    assert!(start.contains("uses_dhcp()"), "only an interface that takes DHCP: {start}");

    let main = read("kernel/src/main.rs");
    let softirqd = main.find("task::softirqd::init_softirq();").expect("softirqd start");
    let poller = main.find("crate::net::init_rx_poll();").expect("x86 starts the e1000 worker");
    assert!(softirqd < poller, "the worker starts after softirqd");

    let init = section(&net, "#[cfg(target_arch = \"x86_64\")]\npub fn init() {", "\n}\n");
    let registered = init.find("register_net_softirq();").expect("NetRx registered");
    let drained = init.find("raise_softirq(SoftirqType::NetRx)").expect("x86 net::init raises NetRx");
    assert!(registered < drained, "the raise follows the registration:\n{init}");
}
