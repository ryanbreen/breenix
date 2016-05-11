
use io::pci;
use io::pci::FunctionInfo;

const VENDOR_ID:u16 = 32902;
const DEVICE_ID:u16 = 4110;

pub fn initialize() {
  let network_device:Option<pci::FunctionInfo> = pci::pci_find_device(VENDOR_ID, DEVICE_ID);

  if !network_device.is_some() {
    println!("NET - e1000 not found");
    return;
  }

  let e1000 = network_device.unwrap();
  println!("NET - Found network device that needs {} of space", e1000.space_needed());

}