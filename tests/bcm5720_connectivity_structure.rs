use std::fs;

#[test]
fn post_lease_failures_report_the_shared_receive_snapshot() {
    let driver = fs::read_to_string("kernel/src/drivers/bcm5720/mod.rs").unwrap();
    let probe = fs::read_to_string("kernel/src/net/dns_probe.rs").unwrap();
    assert!(probe.contains("pub fn run(lease: Lease) -> bool"));
    assert!(probe.contains("ANSWER.load(Ordering::Acquire) && super::icmp::gateway_echo_received()"));
    assert_eq!(driver.matches("if !crate::net::dns_probe::run(lease) {").count(), 2);
    assert_eq!(driver.matches("report_failure(mac, \"connectivity\")").count(), 2);
    assert_eq!(driver.matches("report_failure(mac, \"DHCP\")").count(), 1);
    let helper = driver.split("fn report_failure(").nth(1).expect("shared snapshot helper");
    let helper = helper.split("\n    fn ").next().unwrap();
    for field in ["MBOX_INTERRUPT_0", "status.word(4) & 0xffff", "return_consumer", "std_producer", "rx_errored", "STAT_IN_OCTETS", "STAT_IN_DISCARDS", "RX_BD_INITIATOR_MODE", "RX_MAC_MODE", "phy_link()"] {
        assert!(helper.contains(field), "missing snapshot field {field}");
    }
    assert!(helper.contains("[bcm5720] {reason} failure:"));
    assert!(helper.find("if let Some(").unwrap() > helper.find("};").unwrap());
}
