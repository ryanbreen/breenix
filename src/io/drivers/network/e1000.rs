
use core::ptr;

use io::pci;
use io::drivers::DeviceDriver;
use io::drivers::network::MacAddr;

const REG_EEPROM: usize = 0x0014;

pub struct E1000 {
    pci_device: pci::Device,
    bar0_type: u8,
    mem_type: u8,
    io_base: usize,
    mem_base: usize,
    initialized: bool,
}

impl E1000 {
    pub fn new(device: pci::Device) -> E1000 {
        unsafe {
            let bar0:u8 = device.bar(0) as u8;
            let bar0_type:u8 = bar0 & 1;
            let mut mem_type:u8 = 0;
            if bar0_type == 0 {
                mem_type = (bar0 >> 1) & 0x03;
            }

            let mut e1000: E1000 = E1000 {
                bar0_type: bar0_type,
                mem_type: mem_type,
                pci_device: device,
                io_base: (device.bar(0x1) & !1) as usize,
                mem_base: (device.bar(0x0) & !3) as usize,
                initialized: false,
            };

            // We need to memory map base.
            ::memory::identity_map_range(e1000.io_base, e1000.io_base + 8192);
            ::memory::identity_map_range(e1000.mem_base, e1000.mem_base + 8192);

            e1000.initialize();
            e1000
        }
    }

    unsafe fn write_command(&self, offset: usize, val: u32) {
        if self.bar0_type == 0 {
            println!("Attempting to write {} to 0x{:x}", val, self.io_base + offset);
            ptr::write_volatile((self.io_base + offset) as *const u32 as *mut _, val);
        } else {
            println!("Write failed because we only know how to party MMIO style");
        }
    }

    unsafe fn read_command(&self, offset: usize) -> u32 {
        if self.bar0_type == 0 {
            ptr::read_volatile((self.io_base + offset) as *const u32)
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

    unsafe fn eeprom_read(&mut self, addr: u8) -> u32 {
        let mut tmp:u32 = 0;
        if self.detect_eeprom() {
            self.write_command( REG_EEPROM, (1) | (addr as u32) << 8) ;
            loop {
                tmp = self.read_command(REG_EEPROM);
                if tmp & (1 << 4) != 0 {
                    break;
                }
            }
        }
        else
        {
            self.write_command( REG_EEPROM, (1) | (addr as u32) << 2);
            loop {
                tmp = self.read_command(REG_EEPROM);
                if tmp & (1 << 1) != 0 {
                    break;
                }
            }
        }
        ((tmp >> 16) as u32) & 0xFFFF
    }

    fn read_mac(&mut self) -> MacAddr {
        if self.detect_eeprom() {
            unsafe {
                let mut temp:u32;
                temp = self.eeprom_read(0);
                let mut mac:[u8;6] = [0;6];
                mac[0] = (temp & 0xff) as u8;
                mac[1] = (temp >> 8) as u8;
                temp = self.eeprom_read(1);
                mac[2] = (temp & 0xff) as u8;
                mac[3] = (temp >> 8) as u8;
                temp = self.eeprom_read(2);
                mac[4] = (temp & 0xff) as u8;
                mac[5] = (temp >> 8) as u8;

                MacAddr {
                    bytes: mac
                }
            }
        } else {
            let mut mac:[u8;6] = [0;6];
            MacAddr {
                bytes: mac
            }
        }
    }
}

#[allow(non_snake_case)]
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
        let mac = self.read_mac();

        println!("NET - Found network device that needs {} of space, irq is {}, interrupt pin \
                  {}, command {}, MMIO: {}, mem_base: 0x{:x}, io_base: 0x{:x}, eeprom?: {}: MAC: {}",
                 e1000.bar(0),
                 irq,
                 interrupt_pin,
                 cmd,
                 self.bar0_type,
                 self.mem_base,
                 self.io_base,
                 eeprom,
                 mac);
    }
}
