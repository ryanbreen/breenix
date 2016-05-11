
const VENDOR_ID:u16 = 32902;
const DEVICE_ID:u16 = 4110;

pub fn initialize() {
  use io::pci;
  let network_device:Option<pci::FunctionInfo> = pci::pci_find_device(VENDOR_ID, DEVICE_ID);

  if !network_device.is_some() {
    println!("NET - e1000 not found");
    return;
  }

  println!("NET - Found network device {:?}", network_device);

}