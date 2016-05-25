
use core::ptr;

use io::pci;
use io::drivers::DeviceDriver;
use io::drivers::network::MacAddr;

const VENDOR_ID: u16 = 32902;
const DEVICE_ID: u16 = 4110;

const REG_EEPROM: usize = 0x0014;

pub struct E1000 {
    pci_device: pci::Device,
    base: usize,
    mmio: bool,
    initialized: bool,
}

impl E1000 {
    pub fn new(device: pci::Device) -> E1000 {
        unsafe {
            let base = device.read(0x10) as usize;
            let mut e1000: E1000 = E1000 {
                pci_device: device,
                base: base & 0xFFFFFFF0,
                mmio: base & 0x1 == 0,
                initialized: false,
            };

            // We need to memory map base.
            ::memory::identity_map_range(base, base + 8192);

            e1000.initialize();
            e1000
        }
    }

    unsafe fn write_command(&self, offset: usize, val: u32) {
        if self.mmio {
            println!("Attempting to write {} to 0x{:x}", val, (self.base + offset));
            ptr::write((self.base + offset) as *mut u32, val);
        } else {
            println!("Write failed because we only know how to party MMIO style");
        }
    }

    unsafe fn read_command(&self, offset: usize) -> u32 {
        if self.mmio {
            ptr::read((self.base + offset) as *mut u32)
        } else {
            println!("Write failed because we only know how to party MMIO style");
            0
        }
    }

    fn detect_eeprom(&mut self) -> bool {
        unsafe {
            let mut val:u32 = 0;

            self.write_command(REG_EEPROM, 0x1);

            for _ in 0..1000 {
                val = self.read_command(REG_EEPROM);
                if val & 0x10 != 0 {
                    return true;
                }
            }
            return false;
        }
    }
}

impl DeviceDriver for E1000 {
    fn initialize(&mut self) {

        let e1000 = self.pci_device;
        let irq = unsafe { e1000.read(0x3C) as u8 & 0xF };
        let interrupt_pin = unsafe { e1000.read(0x3D) as u8 & 0xF };
        let cmd = unsafe { e1000.read(0x04) };

        let CTRL: u32 = 0x00;
        let CTRL_LRST: u32 = 1 << 3;
        let CTRL_ASDE: u32 = 1 << 5;
        let CTRL_SLU: u32 = 1 << 6;
        let CTRL_ILOS: u32 = 1 << 7;
        let CTRL_VME: u32 = 1 << 30;
        let CTRL_PHY_RST: u32 = 1 << 31;

        let FCAL: u32 = 0x28;
        let FCAH: u32 = 0x2C;
        let FCT: u32 = 0x30;
        let FCTTV: u32 = 0x170;

        unsafe {

            // Enable auto negotiate, link, clear reset, do not Invert Loss-Of Signal
            e1000.flag(CTRL, CTRL_ASDE | CTRL_SLU, true);
            e1000.flag(CTRL, CTRL_LRST, false);
            e1000.flag(CTRL, CTRL_PHY_RST, false);
            e1000.flag(CTRL, CTRL_ILOS, false);

            // No flow control
            e1000.write(FCAH, 0);
            e1000.write(FCAL, 0);
            e1000.write(FCT, 0);
            e1000.write(FCTTV, 0);

            // Do not use VLANs
            e1000.flag(CTRL, CTRL_VME, false);

            e1000.flag(4, 4, true);
        }

        let eeprom = self.detect_eeprom();

        println!("NET - Found network device that needs {} of space, irq is {}, interrupt pin \
                  {}, command {}, MMIO: {}, base: {:x}, eeprom?: {}",
                 e1000.bar(0),
                 irq,
                 interrupt_pin,
                 cmd,
                 self.mmio,
                 self.base,
                 eeprom);
    }
}
