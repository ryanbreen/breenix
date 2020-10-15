use core::ptr;

use crate::println;

use crate::io::pci;
use crate::io::pci::BAR;
use crate::io::drivers::DeviceDriver;

mod constants;
mod hardware;

use self::constants::*;

pub struct E1000 {
    //pci_device: pci::Device,
    hardware: self::hardware::Hardware,
}

#[allow(unused_mut, unused_assignments)]
impl E1000 {
    pub fn new(device: pci::Device) -> E1000 {

        let mut e1000: E1000 = E1000 {
            hardware: self::hardware::Hardware::new(device)
        };

        // We need to memory map base and io.
        //println!("Need to map from {:x} to {:x}", e1000.io_base, e1000.io_base + 8192);

        //println!("Need to map from {:x} to {:x}", e1000.mem_base, e1000.mem_base + 8192);
        //crate::memory::identity_map_range(e1000.io_base, e1000.io_base + 8192);
        //crate::memory::identity_map_range(e1000.mem_base, e1000.mem_base + 8192);

        e1000.initialize();
        e1000
    }
 }

#[allow(non_snake_case)]
impl DeviceDriver for E1000 {
    fn initialize(&mut self) {

        //let e1000 = self.pci_device;
        // let irq = unsafe { e1000.read(0x3C) as u8 & 0xF };
        // let interrupt_pin = unsafe { e1000.read(0x3D) as u8 & 0xF };
        // let cmd = unsafe { e1000.read(0x04) };

        unsafe {

            crate::println!("Read ctrl: {:x}", self.hardware.read_mem(self::constants::CTRL as usize));
            crate::println!("Read status: {:x}", self.hardware.read_mem(self::constants::STATUS as usize));

            self.hardware.write_command(0, 0x4140240);

            self.hardware.acquire_eeprom();

            if self.hardware.checksum_eeprom() {
                let macbytes = self.hardware.read_mac_addr();
                use macaddr::MacAddr;
                let mac = MacAddr::from(macbytes);
                crate::println!("MAC is {}", mac);
            }
        }
        /*

        println!("NET - Found network device that needs {:x} of space, irq is {}, interrupt pin \
                  {}, command {}, MMIO: {}, mem_base: 0x{:x}, io_base: 0x{:x}, eeprom?: {}: MAC: {}",
                 e1000.bar(0).size,
                 irq,
                 interrupt_pin,
                 cmd,
                 true,
                 self.mem_base,
                 self.io_base,
                 eeprom,
                 mac);

        */
    }
}
