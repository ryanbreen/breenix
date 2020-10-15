use core::fmt;

pub mod e1000;
//pub mod rtl8139;

use alloc::boxed::Box;
use alloc::string::String;

use crate::format;
use crate::io::drivers::DeviceDriver;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NetworkInterfaceType {
    Loopback,
    Ethernet,
    Wireless,
}

pub struct NetworkInterface {
    interface_type: NetworkInterfaceType,
    name: String,
    device_driver: Box<DeviceDriver>,
}

#[allow(dead_code)]
impl NetworkInterface {
    pub fn new(nic_type: NetworkInterfaceType, driver: Box<DeviceDriver>) -> NetworkInterface {
        NetworkInterface {
            interface_type: nic_type,
            name: create_network_interface_name(nic_type),
            device_driver: driver,
        }
    }

    pub fn get_device(&self) -> &DeviceDriver {
        self.device_driver.as_ref()
    }
}

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
