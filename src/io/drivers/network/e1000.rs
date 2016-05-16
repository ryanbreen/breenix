
use io::pci;

const VENDOR_ID: u16 = 32902;
const DEVICE_ID: u16 = 4110;

pub fn initialize() {
    // let network_device: Option<pci::FunctionInfo> = pci::pci_find_device(VENDOR_ID, DEVICE_ID);
    //
    // if !network_device.is_some() {
    // println!("NET - e1000 not found");
    // return;
    // }
    //
    // let e1000 = network_device.unwrap();
    // let irq = unsafe { e1000.read(0x3C) as u8 & 0xF };
    // let interrupt_pin = unsafe { e1000.read(0x3D) as u8 & 0xF };
    // let cmd = unsafe { e1000.read(0x04) };
    // println!("NET - Found network device that needs {} of space, irq is {}, interrupt pin {}, \
    // command {}",
    // e1000.space_needed(),
    // irq,
    // interrupt_pin,
    // cmd);
    //
}
