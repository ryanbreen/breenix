use core::ptr;

use crate::println;

use crate::io::drivers::DeviceDriver;
use crate::io::pci;
use crate::io::pci::BAR;

mod constants;
mod hardware;
mod params;

use self::constants::*;

pub struct E1000 {
    //pci_device: pci::Device,
    hardware: self::hardware::Hardware,
}

#[allow(unused_mut, unused_assignments)]
impl E1000 {
    pub fn new(device: pci::Device) -> E1000 {
        let mut e1000: E1000 = E1000 {
            hardware: self::hardware::Hardware::new(device),
        };

        // We need to memory map base and io.
        //println!("Need to map from {:x} to {:x}", e1000.io_base, e1000.io_base + 8192);

        //println!("Need to map from {:x} to {:x}", e1000.mem_base, e1000.mem_base + 8192);
        //crate::memory::identity_map_range(e1000.io_base, e1000.io_base + 8192);
        //crate::memory::identity_map_range(e1000.mem_base, e1000.mem_base + 8192);

        e1000.initialize();
        e1000
    }

    fn reset(&mut self) -> Result<(), ()> {
        let pba: u32 = PBA as u32;
        self.hardware.write(PBA, PBA_48K)?;

        /* flow control settings:
         * The high water mark must be low enough to fit one full frame
         * (or the size used for early receive) above it in the Rx FIFO.
         * Set it to the lower of:
         * - 90% of the Rx FIFO size, and
         * - the full Rx FIFO size minus the early receive size (for parts
         *   with ERT support assuming ERT set to E1000_ERT_2048), or
         * - the full Rx FIFO size minus one full frame
         */
        use core::cmp;
        let hwm = cmp::min(
            ((pba << 10) * 9 / 10),
            ((pba << 10) - self.hardware.max_frame_size),
        );
        self.hardware.fc_high_water = (hwm as u16) & 0xFFF8;
        self.hardware.fc_low_water = self.hardware.fc_high_water - 8;
        self.hardware.fc_pause_time = FC_PAUSE_TIME;
        self.hardware.fc_send_xon = true;
        self.hardware.fc = FlowControlSettings::E1000_FC_DEFAULT;

        self.hardware.reset()?;

        self.hardware.init()?;
        Ok(())
    }
}

#[allow(non_snake_case)]
impl DeviceDriver for E1000 {
    fn initialize(&mut self) -> Result<(), ()> {
        self.hardware.reset()?;

        self.hardware.acquire_eeprom()?;

        if self.hardware.checksum_eeprom()? {
            self.hardware.load_mac_addr()?;
            crate::println!("MAC is {}", self.hardware.mac);

            let control_port = self
                .hardware
                .read_eeprom(self::constants::EEPROM_INIT_CONTROL3_PORT_A, 1)?;

            let mut wol = 0;

            println!("Control port is {:x}", control_port);

            if (control_port & EEPROM_APME != 0) {
                //pr_info("need to frob the beanflute\n");
                wol |= WUFC_MAG;
            }

            println!("wol is {:x}", wol);

            self.reset()?;
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
        Ok(())
    }
}
