use core::fmt;

pub mod e1000;
pub mod rtl8139;

use alloc::boxed::Box;
use alloc::String;

use io::drivers::DeviceDriver;

// Boosted from redox
//

#[derive(Copy, Clone)]
pub struct MacAddr {
    pub bytes: [u8; 6],
}

#[allow(dead_code)]
impl MacAddr {
    pub fn equals(&self, other: Self) -> bool {
        for i in 0..6 {
            if self.bytes[i] != other.bytes[i] {
                return false;
            }
        }
        true
    }

    pub fn to_string(&self) -> String {
        format!("{:02x}::{:02x}::{:02x}::{:02x}::{:02x}::{:02x}",
                self.bytes[0],
                self.bytes[1],
                self.bytes[2],
                self.bytes[3],
                self.bytes[4],
                self.bytes[5])
    }
}

impl fmt::Display for MacAddr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.to_string())
    }
}

#[derive(Clone,Copy,Debug,PartialEq)]
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
    pub fn new(nic_type:NetworkInterfaceType, driver: Box<DeviceDriver>) -> NetworkInterface {
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
    let count:usize = ::state().network_interfaces.iter().filter(
        |nic| nic.interface_type == nic_type).count();
    format!("{:?}{}", nic_type, count)
}
