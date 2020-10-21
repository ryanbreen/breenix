use crate::io::Port;
use crate::println;
use core::fmt;
use core::intrinsics::transmute;
use spin::Mutex;

struct Pci {
    address: Port<u32>,
    data: Port<u32>,
}

#[allow(dead_code)]
impl Pci {
    /// Read a 32-bit aligned word from PCI Configuration Address Space.
    /// This is marked as `unsafe` because passing in out-of-range
    /// parameters probably does excitingly horrible things to the
    /// hardware.
    unsafe fn read_config(&mut self, bus: u8, slot: u8, function: u8, offset: u8) -> u32 {
        let address: u32 = 0x80000000
            | (bus as u32) << 16
            | (slot as u32) << 11
            | (function as u32) << 8
            | (offset & 0xFC) as u32;
        self.address.write(address);
        self.data.read()
    }

    unsafe fn read16_config(&mut self, bus: u8, slot: u8, function: u8, offset: u8) -> u16 {
        let val = self.read_config(bus, slot, function, offset & 0b11111100);
        ((val >> ((offset as usize & 0b10) << 3)) & 0xFFFF) as u16
    }

    /// Check for a PCI device, and return information about it if present.
    unsafe fn probe(&mut self, bus: u8, slot: u8, function: u8) -> Option<Device> {
        let config_0 = self.read_config(bus, slot, function, 0);
        // We'll receive all 1's if no device is present.
        if config_0 == 0xFFFFFFFF {
            return None;
        }

        let config_4 = self.read_config(bus, slot, function, 0x8);
        let config_c = self.read_config(bus, slot, function, 0xC);

        Some(Device {
            bus: bus,
            device: slot,
            function: function,
            vendor_id: config_0 as u16,
            device_id: (config_0 >> 16) as u16,
            revision_id: config_4 as u8,
            subsystem_id: self.read16_config(bus, slot, function, 0x2E),
            subsystem_vendor_id: self.read16_config(bus, slot, function, 0x2C),
            subclass: (config_4 >> 16) as u8,
            class_code: DeviceClass::from_u8((config_4 >> 24) as u8),
            multifunction: config_c & 0x800000 != 0,
            bars: [BAR {
                addr: 0,
                size: 0,
                is_io: false,
            }; 6],
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

    pub(crate) vendor_id: u16,
    pub(crate) device_id: u16,
    pub(crate) revision_id: u8,
    pub(crate) subsystem_id: u16,
    pub(crate) subsystem_vendor_id: u16,
    subclass: u8,
    class_code: DeviceClass,
    multifunction: bool,
    bars: [BAR; 6],
}

impl fmt::Display for Device {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}.{}.{}: 0x{:04x} 0x{:04x} 0x{:04x} 0x{:04x} {:?} {:02x}",
            self.bus,
            self.device,
            self.function,
            self.vendor_id,
            self.device_id,
            self.subsystem_id,
            self.subsystem_vendor_id,
            self.class_code,
            self.subclass
        )
    }
}

pub const PCI_MAX_BUS_NUMBER: u8 = 32;
pub const PCI_MAX_DEVICE_NUMBER: u8 = 32;

pub const PCI_CONFIG_ADDRESS_PORT: u16 = 0xCF8;
pub const PCI_CONFIG_ADDRESS_ENABLE: u32 = 1 << 31;

pub const PCI_CONFIG_DATA_PORT: u16 = 0xCFC;
pub const PCI_COMMAND_BUSMASTER: u32 = 1 << 2;

pub const PCI_ID_REGISTER: u32 = 0x00;
pub const PCI_COMMAND_REGISTER: u32 = 0x04;
pub const PCI_CLASS_REGISTER: u32 = 0x08;
pub const PCI_HEADER_REGISTER: u32 = 0x0C;
pub const PCI_BAR0_REGISTER: u32 = 0x10;
pub const PCI_CAPABILITY_LIST_REGISTER: u32 = 0x34;
pub const PCI_INTERRUPT_REGISTER: u32 = 0x3C;

pub const PCI_STATUS_CAPABILITIES_LIST: u32 = 1 << 4;

pub const PCI_BASE_ADDRESS_IO_SPACE: u32 = 1 << 0;
pub const PCI_MEM_BASE_ADDRESS_64BIT: u32 = 1 << 2;
pub const PCI_MEM_PREFETCHABLE: u32 = 1 << 3;
pub const PCI_MEM_BASE_ADDRESS_MASK: u32 = 0xFFFF_FFF0;
pub const PCI_IO_BASE_ADDRESS_MASK: u32 = 0xFFFF_FFFC;

pub const PCI_HEADER_TYPE_MASK: u32 = 0x007F_0000;
pub const PCI_MULTIFUNCTION_MASK: u32 = 0x0080_0000;

pub const PCI_CAP_ID_VNDR: u32 = 0x09;

#[derive(Copy, Clone, Debug)]
pub struct BAR {
    /// a memory space address and its size
    pub size: u64,
    pub addr: u64,
    pub is_io: bool,
}

#[allow(dead_code)]
impl Device {
    fn address(&self, register: u8) -> u32 {
        let lbus = u32::from(self.bus);
        let lslot = u32::from(self.device);
        let lfunc = u32::from(self.function);
        let lregister = u32::from(register);

        return (lbus << 16) | (lslot << 11) | (lfunc << 8) | (lregister << 2) | 0x80000000 as u32;
    }

    /// Read
    pub unsafe fn read(&self, register: u8) -> u32 {
        let address = self.address(register);
        PCI.lock().address.write(address);
        return PCI.lock().data.read();
    }

    /// Write
    pub unsafe fn write(&self, offset: u8, value: u32) {
        let address = self.address(offset);
        PCI.lock().address.write(address);
        PCI.lock().data.write(value);
    }

    /// Decode an u32 to BAR.
    unsafe fn decode_bar(&self, register: u8) -> BAR {
        // read bar address
        let addr = self.read(register);
        // write to get length
        self.write(register, 0xFFFF_FFFF);
        // read back length
        let length = self.read(register);
        // restore original value
        self.write(register, addr);
        match addr & 0x01 {
            0 => {
                // memory space bar
                BAR {
                    addr: (addr & 0xFFFF_FFF0) as u64,
                    size: (!(length & 0xFFFF_FFF0)).wrapping_add(1) as u64,
                    is_io: false,
                }
            }
            _ => {
                // io space bar
                BAR {
                    addr: (addr & 0xFFFF_FFFC) as u64,
                    size: (!(length & 0xFFFF_FFFC)).wrapping_add(1) as u64,
                    is_io: true,
                }
            }
        }
    }

    fn load_bars(&mut self) {
        unsafe {
            // Populate BARs
            self.bars[0] = self.decode_bar(4);
            self.bars[1] = self.decode_bar(5);
            self.bars[2] = self.decode_bar(6);
            self.bars[3] = self.decode_bar(7);
            self.bars[4] = self.decode_bar(8);
            self.bars[5] = self.decode_bar(9);
        }
    }

    pub fn bar(&self, idx: usize) -> BAR {
        self.bars[idx]
    }
}

static PCI: Mutex<Pci> = Mutex::new(Pci {
    address: unsafe { Port::new(0xCF8) },
    data: unsafe { Port::new(0xCFC) },
});

#[allow(dead_code)]
const MAX_BUS: u8 = 255;

#[allow(dead_code)]
const MAX_DEVICE: u8 = 31;

#[allow(dead_code)]
const MAX_FUNCTION: u8 = 7;

fn device_specific_init(dev: &mut Device) {
    match dev.device_id {
        0x1111 => {
            /*
            println!("{}-{}-{} VGA {}",
                        dev.bus,
                        dev.device,
                        dev.function,
                        dev)
                        */
        }
        0x1237 => {
            /*
            println!("{}-{}-{} 82440LX/EX {}",
                        dev.bus,
                        dev.device,
                        dev.function,
                        dev)
                        */
        }
        0x100E | 0x100F => {
            println!(
                "{}-{}-{} Intel Pro 1000/MT {}", dev.bus, dev.device, dev.function, dev
            );

            use crate::io::drivers::network::{NetworkInterface,NetworkInterfaceType};
            use crate::io::drivers::network::e1000::E1000;

            let e1000 = E1000::new(dev);
            let mut nic = NetworkInterface::new(NetworkInterfaceType::Ethernet, alloc::boxed::Box::new(e1000));

            let res = nic.device_driver.probe();

            println!("Got good nic? {}", res.is_ok());
            //let nic: NetworkInterface =
            //    NetworkInterface::new(NetworkInterfaceType::Ethernet, Box::new(e1000));
            //println!("Registered as {}", nic);
            //::state().network_interfaces.push(nic);
        }
        0x8139 => {
            println!(
                "{}-{}-{} RTL8139 Fast Ethernet NIC {}",
                dev.bus, dev.device, dev.function, dev
            );
            /*
            use crate::io::drivers::network::rtl8139::Rtl8139;
            let rtl = Rtl8139::new(*dev);
            let nic:NetworkInterface = NetworkInterface::new(NetworkInterfaceType::Ethernet, Box::new(rtl));
            */
            //println!("Registered as {}", nic);
            //::state().network_interfaces.push(nic);
        }
        0x7000 => {
            /*
            println!("{}-{}-{} PIIX3 PCI-to-ISA Bridge (Triton II) {}",
                        dev.bus,
                        dev.device,
                        dev.function,
                        dev)
                        */
        }
        0x7010 => {
            /*
            println!("{}-{}-{} PIIX3 IDE Interface (Triton II) {}",
                        dev.bus,
                        dev.device,
                        dev.function,
                        dev)
                        */
        }
        0x7113 => {
            /*
            println!("{}-{}-{} PIIX4/4E/4M Power Management Controller {}",
                        dev.bus,
                        dev.device,
                        dev.function,
                        dev)
                        */
        }
        _ => println!("{}", dev),
    }
}

pub fn initialize() {
    for bus in 0..MAX_BUS {
        for dev in 0..MAX_DEVICE {
            for func in 0..MAX_FUNCTION {
                unsafe {
                    let device = PCI.lock().probe(bus, dev, func);

                    match device {
                        Some(mut d) => {
                            d.load_bars();
                            device_specific_init(&mut d);
                            //println!("Loaded device {}", d);
                        }
                        None => {}
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceErrorCause {
    InitializationFailure,
    UnexpectedStuff,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceError {
    pub cause: DeviceErrorCause,
}
