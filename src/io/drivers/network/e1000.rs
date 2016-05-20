
use io::pci;
use io::drivers::network::MacAddr;

const VENDOR_ID: u16 = 32902;
const DEVICE_ID: u16 = 4110;

const RAL0: u32 = 0x5400;
const RAH0: u32 = 0x5404;

pub fn initialize() {
    let network_device: Option<pci::Device> = pci::pci_find_device(VENDOR_ID, DEVICE_ID);

    if !network_device.is_some() {
        println!("NET - e1000 not found");
        return;
    }

    let e1000 = network_device.unwrap();
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
    }

    println!("NET - Found network device that needs {} of space, irq is {}, interrupt pin {}, \
        command {}, MAC: {}",
             e1000.bar(0),
             irq,
             interrupt_pin,
             cmd,
             mac);

}
