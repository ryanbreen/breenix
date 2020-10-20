
pub mod e1000;
pub mod vlan;

use crate::io::pci::DeviceError;

pub const MAXIMUM_ETHERNET_VLAN_SIZE: u32 = 1522;

pub trait NetworkDriver {
    fn probe(&mut self) -> Result<(), DeviceError>;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NetworkInterfaceType {
    Loopback,
    Ethernet,
    Wireless,
}

pub struct NetworkInterface<T: NetworkDriver> {
    interface_type: NetworkInterfaceType,
    //name: String,
    device_driver: T,
}

#[allow(dead_code)]
impl<T: NetworkDriver> NetworkInterface<T> {
    pub fn new(nic_type: NetworkInterfaceType, driver: T) -> Result<NetworkInterface<T>, ()> {
        let mut nic = NetworkInterface {
            interface_type: nic_type,
            //name: create_network_interface_name(nic_type),
            device_driver: driver,
        };

        let res = nic.device_driver.probe();
        if !res.is_ok() {
            panic!("Failed to boot nic");
        }

        Ok(nic)
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
