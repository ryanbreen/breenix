pub mod e1000;
pub mod vlan;

use crate::io::pci::DeviceError;

use alloc::boxed::Box;

#[allow(unused_variables)]
#[allow(dead_code)]
pub(in crate::io::drivers::network) mod constants;

pub trait NetworkDriver {
    fn probe(&mut self) -> Result<(), DeviceError>;
    fn open(&mut self) -> Result<(), DeviceError>;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::io) enum NetworkInterfaceType {
    Loopback,
    Ethernet,
    Wireless,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::io) struct NetworkDeviceData {
    mtu: u32,
    min_mtu: u32,
    max_mtu: u32,
    carrier_online: bool,
    carrier_down_count: u64,
}

impl NetworkDeviceData {
    pub(in crate::io) fn defaults() -> NetworkDeviceData {
        NetworkDeviceData {
            mtu: 0,
            min_mtu: 0,
            max_mtu: 0,
            carrier_online: false,
            carrier_down_count: 0,
        }
    }
}

pub(in crate::io) struct NetworkInterface<D: NetworkDriver> {
    pub(in crate::io) interface_type: NetworkInterfaceType,
    pub(in crate::io) device_driver: Box<D>,
}

#[allow(dead_code)]
impl<D: NetworkDriver> NetworkInterface<D> {
    pub(in crate::io) fn new(
        nic_type: NetworkInterfaceType,
        driver: Box<D>,
    ) -> NetworkInterface<D> {
        NetworkInterface {
            interface_type: nic_type,
            device_driver: driver,
        }
    }

    pub(in crate::io) fn up(&mut self) -> Result<(), ()> {
        let res = self.device_driver.probe();
        crate::println!("Got good nic? {}", res.is_ok());
        let res2 = self.device_driver.open();
        Ok(())
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
