use core::ptr;

use crate::println;

use crate::io::pci;
use crate::io::pci::BAR;

use crate::io::drivers::network::e1000::constants::*;

pub struct Hardware {
    io_base: usize,
    mem_base: usize,
}

#[allow(unused_mut, unused_assignments)]
impl Hardware {
    pub fn new(device: pci::Device) -> Hardware {
        let hardware = Hardware {
            io_base: device.bar(0x1).addr as usize,
            mem_base: device.bar(0x0).addr as usize,
        };

        use x86_64::structures::paging::PageTableFlags;
        crate::memory::identity_map_range(
            device.bar(0x0).addr,
            device.bar(0x0).size,
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
        );

        hardware
    }

    pub unsafe fn write_command(&self, offset: usize, val: u32) {
        ptr::write_volatile((self.io_base + offset) as *const u32 as *mut _, val);
    }

    unsafe fn read_command(&self, offset: usize) -> u32 {
        ptr::read_volatile((self.io_base + offset) as *const u32)
    }

    unsafe fn write_mem(&self, offset: usize, val: u32) {
        ptr::write_volatile((self.mem_base + offset) as *const u32 as *mut _, val);
    }

    pub unsafe fn read_mem(&self, offset: usize) -> u32 {
        ptr::read_volatile((self.mem_base + offset) as *const u32)
    }

    pub unsafe fn acquire_eeprom(&self) {
        let mut eecd: u32 = 0;
        let mut i = 0;

        eecd = self.read_mem(CTRL_EECD);

        /* Request EEPROM Access */
        eecd |= E1000_EECD_REQ;
        self.write_mem(CTRL_EECD, eecd);
        eecd = self.read_mem(CTRL_EECD);
        while (eecd & E1000_EECD_GNT == 0 && (i < E1000_EEPROM_GRANT_ATTEMPTS)) {
            i += 1;
            // udelay(5);
            eecd = self.read_mem(CTRL_EECD);
        }

        if (eecd & E1000_EECD_GNT == 0) {
            panic!("Failed to acquire eeprom");
        }

        /* Setup EEPROM for Read/Write */

        /* Clear SK and DI */
        eecd = eecd & !(E1000_EECD_DI | E1000_EECD_SK);
        self.write_mem(CTRL_EECD, eecd);

        /* Set CS */
        eecd = eecd | E1000_EECD_CS;
        self.write_mem(CTRL_EECD, eecd);

        eecd = self.read_mem(CTRL_EECD);
    }

    unsafe fn write_flush(&self) {
        // write flush
        self.read_mem(STATUS);
    }

    unsafe fn delay(&self) {
        for i in 0..1 {
            //udelay(eeprom->delay_usec);
        }
    }

    unsafe fn standby_eeprom(&self) {
        let mut eecd: u32 = self.read_mem(CTRL_EECD);

        eecd &= !(E1000_EECD_CS | E1000_EECD_SK);
        self.write_mem(CTRL_EECD, eecd);
        self.write_flush();
        self.delay();

        /* Clock high */
        eecd |= E1000_EECD_SK;
        self.write_mem(CTRL_EECD, eecd);
        self.write_flush();
        self.delay();

        /* Select EEPROM */
        eecd |= E1000_EECD_CS;
        self.write_mem(CTRL_EECD, eecd);
        self.write_flush();
        self.delay();

        /* Clock low */
        eecd &= !E1000_EECD_SK;
        self.write_mem(CTRL_EECD, eecd);
        self.write_flush();
        self.delay();
    }

    unsafe fn raise_ee_clk(&self, eecd: u32) -> u32 {
        /*
         * Raise the clock input to the EEPROM (by setting the SK bit), and then
         * wait <delay> microseconds.
         */
        let mut new_eecd = eecd | E1000_EECD_SK;
        self.write_mem(CTRL_EECD, new_eecd);

        self.write_flush();
        self.delay();
        new_eecd
    }

    unsafe fn lower_ee_clk(&self, eecd: u32) -> u32 {
        /*
         * Raise the clock input to the EEPROM (by setting the SK bit), and then
         * wait <delay> microseconds.
         */
        let mut new_eecd = eecd & !E1000_EECD_SK;
        self.write_mem(CTRL_EECD, new_eecd);

        self.write_flush();
        self.delay();
        new_eecd
    }

    unsafe fn shift_in_ee_bits(&self, count: u16) -> u16 {
        let mut eecd: u32;
        let mut data: u16 = 0;

        /*
         * In order to read a register from the EEPROM, we need to shift 'count'
         * bits in from the EEPROM. Bits are "shifted in" by raising the clock
         * input to the EEPROM (setting the SK bit), and then reading the value
         * of the "DO" bit.  During this "shifting in" process the "DI" bit
         * should always be clear.
         */
        eecd = self.read_mem(CTRL_EECD);

        eecd &= !(E1000_EECD_DO | E1000_EECD_DI);

        for i in 0..count {
            data = data << 1;
            self.raise_ee_clk(eecd);

            eecd = self.read_mem(CTRL_EECD);

            eecd &= !(E1000_EECD_DI);
            if (eecd & E1000_EECD_DO != 0) {
                data |= 1;
            }

            self.lower_ee_clk(eecd);
        }

        data
    }

    unsafe fn shift_out_ee_bits(&self, data: u32, count: u32) {
        let mut eecd: u32;
        let mut mask: u32;

        /*
         * We need to shift "count" bits out to the EEPROM. So, value in the
         * "data" parameter will be shifted out to the EEPROM one bit at a time.
         * In order to do this, "data" must be broken down into bits.
         */
        mask = 0x01 << (count - 1);
        eecd = self.read_mem(CTRL_EECD);

        eecd = eecd & !E1000_EECD_DO;

        while (mask != 0) {
            /*
             * A "1" is shifted out to the EEPROM by setting bit "DI" to a
             * "1", and then raising and then lowering the clock (the SK bit
             * controls the clock input to the EEPROM).  A "0" is shifted
             * out to the EEPROM by setting "DI" to "0" and then raising and
             * then lowering the clock.
             */
            eecd &= !E1000_EECD_DI;

            if (data & mask != 0) {
                eecd = eecd | E1000_EECD_DI;
            }

            self.write_mem(CTRL_EECD, eecd);

            // write flush
            self.read_mem(STATUS);

            eecd = self.raise_ee_clk(eecd);
            eecd = self.lower_ee_clk(eecd);

            mask = mask >> 1;
        }

        /* We leave the "DI" bit set to "0" when we leave this routine. */
        eecd &= !E1000_EECD_DI;
        self.write_mem(CTRL_EECD, eecd);

        eecd = self.read_mem(CTRL_EECD);
    }

    unsafe fn read_eeprom(&self, offset: u16, words: u16) -> u16 {
        let mut data: u16 = 0;
        for i in 0..words {
            /* Send the READ command (opcode + addr)  */
            self.shift_out_ee_bits(EEPROM_READ_OPCODE_MICROWIRE, 3);

            self.shift_out_ee_bits(offset as u32 + i as u32, 6);

            /*
             * Read the data.  For microwire, each word requires the
             * overhead of eeprom setup and tear-down.
             */
            data = data | (self.shift_in_ee_bits(16) << (8 * i));
            self.standby_eeprom();
        }
        data
    }

    /**
     * Verifies that the EEPROM has a valid checksum
     *
     * Reads the first 64 16 bit words of the EEPROM and sums the values read.
     * If the the sum of the 64 16 bit words is 0xBABA, the EEPROM's checksum is
     * valid.
     */
    pub unsafe fn checksum_eeprom(&self) -> bool {
        let mut checksum: u16 = 0;
        for i in 0..EEPROM_CHECKSUM_REG + 1 {
            let data: u16 = self.read_eeprom(i, 1);
            // crate::println!("data at {} is {:x}", i, data);
            checksum = checksum.wrapping_add(data);
        }

        crate::println!("eeprom checksum is {:x}", checksum);

        (checksum == EEPROM_SUM)
    }

    /*
     * Reads the adapter's MAC address from the EEPROM and inverts the LSB for the
     * second function of dual function devices
     */
    pub unsafe fn read_mac_addr(&self) -> [u8; 6] {
        let mut mac: [u8; 6] = [0; 6];

        let mut offset: u16 = 0;
        let mut eeprom_data: u16 = 0;

        for i in (0..NODE_ADDRESS_SIZE).step_by(2) {
            offset = i as u16 >> 1;
            eeprom_data = self.read_eeprom(offset, 1);
            mac[i] = eeprom_data as u8 & 0x00FF;
            mac[i + 1] = eeprom_data.wrapping_shr(8) as u8;
        }

        mac
    }
}
