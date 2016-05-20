
use io::pci;
use io::drivers::network::MacAddr;

const VENDOR_ID: u16 = 32902;
const DEVICE_ID: u16 = 4110;

const RAL0: u32 = 0x5400;
const RAH0: u32 = 0x5404;

pub struct E1000 {
    pci_device: pci::Device,
    initialized: bool,
}

impl E1000 {
    pub fn new(device: pci::Device) -> E1000 {
        let mut e1000: E1000 = E1000 {
            pci_device: device,
            initialized: false,
        };

        e1000.initialize();
        e1000
    }

    fn initialize(&mut self) {

        let e1000 = self.pci_device;
        let irq = unsafe { e1000.read(0x3C) as u8 & 0xF };
        let interrupt_pin = unsafe { e1000.read(0x3D) as u8 & 0xF };
        let cmd = unsafe { e1000.read(0x04) };

        let mac;

        unsafe {
            let mac_low = e1000.read(RAL0);
            let mac_high = e1000.read(RAH0);
            mac = MacAddr {
                bytes: [mac_low as u8,
                        (mac_low >> 8) as u8,
                        (mac_low >> 16) as u8,
                        (mac_low >> 24) as u8,
                        mac_high as u8,
                        (mac_high >> 8) as u8],
            };

            e1000.flag(4, 4, true);
        }

        println!("NET - Found network device that needs {} of space, irq is {}, interrupt pin \
                  {}, command {}, MAC: {}",
                 e1000.bar(0),
                 irq,
                 interrupt_pin,
                 cmd,
                 mac);
    }
}
