//! Everything below shamelessly stolen from emk's toyos-rs
//! Interface to our PCI devices.
//!
//! As usual, this is heavily inspired by http://wiki.osdev.org/Pci

use alloc::boxed::Box;
use core::fmt;
use core::intrinsics::transmute;
use core::iter::Iterator;
use spin::Mutex;
use io::Port;
use io::drivers::DeviceDriver;
use io::drivers::network::{NetworkInterface,NetworkInterfaceType};

use collections::Vec;

struct Pci {
    address: Port<u32>,
    data: Port<u32>,
}

impl Pci {
    /// Read a 32-bit aligned word from PCI Configuration Address Space.
    /// This is marked as `unsafe` because passing in out-of-range
    /// parameters probably does excitingly horrible things to the
    /// hardware.
    unsafe fn read_config(&mut self, bus: u8, slot: u8, function: u8, offset: u8) -> u32 {
        let address: u32 = 0x80000000 | (bus as u32) << 16 | (slot as u32) << 11 |
                           (function as u32) << 8 |
                           (offset & 0xFC) as u32;
        self.address.write(address);
        self.data.read()
    }

    /// Check for a PCI device, and return information about it if present.
    unsafe fn probe(&mut self, bus: u8, slot: u8, function: u8) -> Option<Device> {
        let config_0 = self.read_config(bus, slot, function, 0);
        // We'll receive all 1's if no device is present.
        if config_0 == 0xFFFFFFFF {
            return None;
        }

        println!("Found device {}-{}-{}", bus, slot, function);

        let config_4 = self.read_config(bus, slot, function, 0x8);
        let config_c = self.read_config(bus, slot, function, 0xC);

        Some(Device {
            bus: bus,
            device: slot,
            function: function,
            vendor_id: config_0 as u16,
            device_id: (config_0 >> 16) as u16,
            revision_id: config_4 as u8,
            subclass: (config_4 >> 16) as u8,
            class_code: DeviceClass::from_u8((config_4 >> 24) as u8),
            multifunction: config_c & 0x800000 != 0,
            bars: [0; 6],
        })
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
#[repr(u8)]
#[allow(dead_code)]
pub enum DeviceClass {
    Legacy = 0x00,
    MassStorage = 0x01,
    Network = 0x02,
    Display = 0x03,
    Multimedia = 0x04,
    Memory = 0x05,
    BridgeDevice = 0x06,
    SimpleCommunication = 0x07,
    BaseSystemPeripheral = 0x08,
    InputDevice = 0x09,
    DockingStation = 0x0A,
    Processor = 0x0B,
    SerialBus = 0x0C,
    Wireless = 0x0D,
    IntelligentIO = 0x0E,
    SatelliteCommunication = 0x0F,
    EncryptionDecryption = 0x10,
    DataAndSignalProcessing = 0x11,
    Unknown,
}

impl DeviceClass {
    fn from_u8(c: u8) -> DeviceClass {
        if c <= DeviceClass::DataAndSignalProcessing as u8 {
            unsafe { transmute(c) }
        } else {
            DeviceClass::Unknown
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub struct Device {
    bus: u8,
    device: u8,
    function: u8,

    vendor_id: u16,
    device_id: u16,
    revision_id: u8,
    subclass: u8,
    class_code: DeviceClass,
    multifunction: bool,
    bars: [u32; 6],
}

impl fmt::Display for Device {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f,
               "{}.{}.{}: {:04x} {:04x} {:?} {:02x}",
               self.bus,
               self.device,
               self.function,
               self.vendor_id,
               self.device_id,
               self.class_code,
               self.subclass)
    }
}

impl Device {
    fn address(&self, offset: u32) -> u32 {
        return 1 << 31 | (self.bus as u32) << 16 | (self.device as u32) << 11 |
               (self.function as u32) << 8 | (offset as u32 & 0xFC);
    }

    /// Read
    pub unsafe fn read(&self, offset: u32) -> u32 {
        let address = self.address(offset);
        PCI.lock().address.write(address);
        return PCI.lock().data.read();
    }

    /// Write
    pub unsafe fn write(&self, offset: u32, value: u32) {
        let address = self.address(offset);
        PCI.lock().address.write(address);
        PCI.lock().data.write(value);
    }

    pub unsafe fn flag(&self, offset: u32, flag: u32, toggle: bool) {
        let mut value = self.read(offset);
        if toggle {
            value |= flag;
        } else {
            value &= 0xFFFFFFFF - flag;
        }
        self.write(offset, value);
    }

    pub fn bar(&self, idx:usize) -> u32 {
        self.bars[idx]
    }
}

static PCI: Mutex<Pci> = Mutex::new(Pci {
    address: unsafe { Port::new(0xCF8) },
    data: unsafe { Port::new(0xCFC) },
});

const MAX_BUS: u8 = 255;
const MAX_DEVICE: u8 = 31;
const MAX_FUNCTION: u8 = 7;

pub fn pci_find_device(vendor_id: u16, device_id: u16) -> Option<Device> {
    for dev in ::state().devices.iter() {
        if dev.device_id == device_id && dev.vendor_id == vendor_id {
            return Some(*dev);
        }
    }

    None
}


fn initialize_device(bus: u8, dev: u8) {

    let mut func: u8 = 0;

    for func in 0..MAX_FUNCTION {

        unsafe {
            let device = PCI.lock().probe(bus, dev, func);

            match device {
                Some(d) => {
                    println!("Found device {:?}", device);
                    ::state().devices.push(d);
                }
                None => {}
            }
        }
    }

}

fn initialize_bus(bus: u8) {
    for dev in 0..MAX_DEVICE {
        initialize_device(bus, dev);
    }
}

pub fn initialize() {

    for bus in 0..MAX_BUS {
        initialize_bus(bus);
    }

    println!("Discovered {} devices", ::state().devices.len());

    for dev in ::state().devices.iter_mut() {

        // Populate BARs for each device.
        unsafe {
            for i in 0..6 {
                let bar = dev.read(i * 4 + 0x10);
                if bar > 0 {
                    println!(" BAR{}: {:x} {:b}", i, bar, bar);
                    dev.bars[i as usize] = bar;
                    dev.write(i * 4 + 0x10, 0xFFFFFFFF);
                    let size = (0xFFFFFFFF - (dev.read(i * 4 + 0x10) & 0xFFFFFFF0)) + 1;
                    dev.write(i * 4 + 0x10, bar);
                    if size > 0 {
                        println!(" size: {}", size);
                        dev.bars[i as usize] = size;
                    }
                }
            }
        }

        match dev.device_id {
            4369 => {
                println!("{}-{}-{} VGA {:?}",
                         dev.bus,
                         dev.device,
                         dev.function,
                         dev.class_code)
            }
            4663 => {
                println!("{}-{}-{} 82440LX/EX {:?}",
                         dev.bus,
                         dev.device,
                         dev.function,
                         dev.class_code)
            }
            4110 => {
                println!("{}-{}-{} Intel Pro 1000/MT {:?}",
                         dev.bus,
                         dev.device,
                         dev.function,
                         dev.class_code);
                use io::drivers::network::e1000::E1000;
                let e1000 = E1000::new(*dev);
                let nic:NetworkInterface = NetworkInterface::new(NetworkInterfaceType::Ethernet, Box::new(e1000));
                println!("Registered as {}", nic);
                ::state().network_interfaces.push(nic);
            }
            28672 => {
                println!("{}-{}-{} PIIX3 PCI-to-ISA Bridge (Triton II) {:?}",
                         dev.bus,
                         dev.device,
                         dev.function,
                         dev.class_code)
            }
            28688 => {
                println!("{}-{}-{} PIIX3 IDE Interface (Triton II) {:?}",
                         dev.bus,
                         dev.device,
                         dev.function,
                         dev.class_code)
            }
            28947 => {
                println!("{}-{}-{} PIIX4/4E/4M Power Management Controller {:?}",
                         dev.bus,
                         dev.device,
                         dev.function,
                         dev.class_code)
            }
            _ => println!("{:?}", dev),
        }
    }

}
