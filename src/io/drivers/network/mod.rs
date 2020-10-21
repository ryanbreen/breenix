
pub mod e1000;
pub mod vlan;

use crate::io::pci::DeviceError;

use alloc::boxed::Box;

pub const MAXIMUM_ETHERNET_VLAN_SIZE: u32 = 1522;

pub trait NetworkDriver {
    fn probe(&mut self) -> Result<(), DeviceError>;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::io) enum NetworkInterfaceType {
    Loopback,
    Ethernet,
    Wireless,
}

pub(in crate::io) struct NetworkInterface<D: NetworkDriver> {
    pub (in crate::io) interface_type: NetworkInterfaceType,
    pub (in crate::io) device_driver: Box<D>,
}

#[allow(dead_code)]
impl NetworkInterface {
    pub (in crate::io) fn new(nic_type: NetworkInterfaceType, driver: Box<D>) -> NetworkInterface {
        NetworkInterface {
            interface_type: nic_type,
            //name: create_network_interface_name(nic_type),
            device_driver: driver,
        }
    }
}

/*
impl fmt::Display for NetworkInterface {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.name)
    }
}

pub fn create_network_interface_name(nic_type: NetworkInterfaceType) -> String {
    /*
    let count:usize = ::state().network_interfaces.iter().filter(
        |nic| nic.interface_type == nic_type).count();
    */
    format!("{:?}{}", 0, 0)
}
*/
