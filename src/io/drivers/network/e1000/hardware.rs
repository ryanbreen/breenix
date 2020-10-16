use core::ptr;

use crate::println;

use crate::io::pci;
use crate::io::pci::BAR;

use crate::io::drivers::network::e1000::constants::*;

pub(in crate::io::drivers::network::e1000) struct Hardware {
    io_base: u64,
    hw_addr: u64,
    pub(in crate::io::drivers::network::e1000) vendor_id: u16,
    pub(in crate::io::drivers::network::e1000) device_id: u16,
    pub(in crate::io::drivers::network::e1000) subsystem_vendor_id: u16,
    pub(in crate::io::drivers::network::e1000) subsystem_id: u16,
    pub(in crate::io::drivers::network::e1000) revision_id: u8,
    pub(in crate::io::drivers::network::e1000) mac_type: MacType,
    pub(in crate::io::drivers::network::e1000) media_type: MediaType,
    pub(in crate::io::drivers::network::e1000) bus_type: BusType,
    pub(in crate::io::drivers::network::e1000) bus_speed: BusSpeed,
    pub(in crate::io::drivers::network::e1000) bus_width: BusWidth,
    pub(in crate::io::drivers::network::e1000) mtu: u16,
    pub(in crate::io::drivers::network::e1000) max_frame_size: u32,
    pub(in crate::io::drivers::network::e1000) fc_high_water: u16,
    pub(in crate::io::drivers::network::e1000) fc_low_water: u16,
    pub(in crate::io::drivers::network::e1000) fc_pause_time: u16,
    pub(in crate::io::drivers::network::e1000) fc_send_xon: bool,
    pub(in crate::io::drivers::network::e1000) fc: FlowControlSettings,
    pub(in crate::io::drivers::network::e1000) wait_autoneg_complete: bool,
    pub(in crate::io::drivers::network::e1000) tbi_compatibility_en: bool,
    pub(in crate::io::drivers::network::e1000) adaptive_ifs: bool,
    pub(in crate::io::drivers::network::e1000) mdix: u8,
    pub(in crate::io::drivers::network::e1000) disable_polarity_correction: bool,
    pub(in crate::io::drivers::network::e1000) master_slave: MasterSlaveType,
    pub(in crate::io::drivers::network::e1000) ledctl_default: u32,
    pub(in crate::io::drivers::network::e1000) ledctl_mode1: u32,
    pub(in crate::io::drivers::network::e1000) ledctl_mode2: u32,
}

#[allow(unused_mut, unused_assignments)]
impl Hardware {
    pub fn new(device: pci::Device) -> Hardware {
        let mut hardware = Hardware {
            io_base: device.bar(0x1).addr,
            hw_addr: device.bar(0x0).addr,
            /* below vendor fields are pulled from Linux running on Qemu */
            vendor_id: 0x8086, /* Intel */
            device_id: 0x100e, /* e1000 */
            subsystem_vendor_id: 0x1af4,
            subsystem_id: 0x1100,
            bus_type: BusType::e1000_bus_type_unknown,
            bus_speed: BusSpeed::e1000_bus_speed_unknown,
            bus_width: BusWidth::e1000_bus_width_unknown,
            mac_type: MacType::e1000_82540,
            media_type: MediaType::e1000_media_type_copper,
            revision_id: 0x3,
            mtu: 0x5dc,
            max_frame_size: 0x5ee,
            fc_high_water: 0,
            fc_low_water: 0,
            fc_pause_time: 0,
            fc_send_xon: false,
            fc: FlowControlSettings::E1000_FC_DEFAULT,
            wait_autoneg_complete: false,
            tbi_compatibility_en: true,
            adaptive_ifs: true,
            mdix: AUTO_ALL_MODES,
            disable_polarity_correction: false,
            master_slave: MasterSlaveType::e1000_ms_hw_default,
            ledctl_default: 0,
            ledctl_mode1: 0,
            ledctl_mode2: 0,
        };

        use x86_64::structures::paging::PageTableFlags;
        crate::memory::identity_map_range(
            device.bar(0x0).addr,
            device.bar(0x0).size,
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
        );

        hardware.populate_bus_info();

        hardware
    }

    pub fn populate_bus_info(&mut self) -> Result<(), ()> {
        let status = self.read(STATUS)?;

        self.bus_type = match status & E1000_STATUS_PCIX_MODE {
            0 => BusType::e1000_bus_type_pci,
            _ => BusType::e1000_bus_type_pcix,
        };

        self.bus_speed = match status & E1000_STATUS_PCI66 {
            0 => BusSpeed::e1000_bus_speed_33,
            _ => BusSpeed::e1000_bus_speed_66,
        };

        self.bus_width = match status & E1000_STATUS_BUS64 {
            0 => BusWidth::e1000_bus_width_32,
            _ => BusWidth::e1000_bus_width_64,
        };

        Ok(())
    }

    pub fn write_command(&self, offset: u32, val: u32) {
        unsafe {
            ptr::write_volatile((self.io_base + offset as u64) as *const u32 as *mut _, val);
        }
    }

    pub fn read_command(&self, offset: u32) -> u32 {
        unsafe { ptr::read_volatile((self.io_base + offset as u64) as *const u32) }
    }

    pub fn write(&self, offset: u32, val: u32) -> Result<(), ()> {
        unsafe {
            ptr::write_volatile((self.hw_addr + offset as u64) as *const u32 as *mut _, val);
        }
        Ok(())
    }

    pub fn read(&self, offset: u32) -> Result<(u32), ()> {
        Ok(unsafe { ptr::read_volatile((self.hw_addr + offset as u64) as *const u32) })
    }

    pub fn acquire_eeprom(&self) -> Result<(), ()> {
        let mut eecd: u32 = 0;
        let mut i = 0;

        eecd = self.read(CTRL_EECD)?;

        /* Request EEPROM Access */
        eecd |= E1000_EECD_REQ;
        self.write(CTRL_EECD, eecd)?;
        eecd = self.read(CTRL_EECD)?;
        while (eecd & E1000_EECD_GNT == 0 && (i < E1000_EEPROM_GRANT_ATTEMPTS)) {
            i += 1;
            // udelay(5);
            eecd = self.read(CTRL_EECD)?;
        }

        if (eecd & E1000_EECD_GNT == 0) {
            panic!("Failed to acquire eeprom");
        }

        /* Setup EEPROM for Read/Write */

        /* Clear SK and DI */
        eecd = eecd & !(E1000_EECD_DI | E1000_EECD_SK);
        self.write(CTRL_EECD, eecd)?;

        /* Set CS */
        eecd = eecd | E1000_EECD_CS;
        self.write(CTRL_EECD, eecd)?;

        eecd = self.read(CTRL_EECD)?;

        Ok(())
    }

    fn write_flush(&self) -> Result<(), ()> {
        // write flush
        self.read(STATUS)?;
        Ok(())
    }

    fn delay(&self) {
        //crate::delay!(EEPROM_DELAY_USEC);
        /*
        for i in 0..1 {
            //udelay(eeprom->delay_usec);
        }*/
    }

    fn standby_eeprom(&self) -> Result<(), ()> {
        let mut eecd: u32 = self.read(CTRL_EECD)?;

        eecd &= !(E1000_EECD_CS | E1000_EECD_SK);
        self.write(CTRL_EECD, eecd)?;
        self.write_flush()?;
        self.delay();

        /* Clock high */
        eecd |= E1000_EECD_SK;
        self.write(CTRL_EECD, eecd)?;
        self.write_flush()?;
        self.delay();

        /* Select EEPROM */
        eecd |= E1000_EECD_CS;
        self.write(CTRL_EECD, eecd)?;
        self.write_flush()?;
        self.delay();

        /* Clock low */
        eecd &= !E1000_EECD_SK;
        self.write(CTRL_EECD, eecd)?;
        self.write_flush()?;
        self.delay();

        Ok(())
    }

    fn raise_ee_clk(&self, eecd: u32) -> Result<(u32), ()> {
        /*
         * Raise the clock input to the EEPROM (by setting the SK bit), and then
         * wait <delay> microseconds.
         */
        let mut new_eecd = eecd | E1000_EECD_SK;
        self.write(CTRL_EECD, new_eecd)?;

        self.write_flush()?;
        self.delay();
        Ok(new_eecd)
    }

    fn lower_ee_clk(&self, eecd: u32) -> Result<u32, ()> {
        /*
         * Raise the clock input to the EEPROM (by setting the SK bit), and then
         * wait <delay> microseconds.
         */
        let mut new_eecd = eecd & !E1000_EECD_SK;
        self.write(CTRL_EECD, new_eecd)?;

        self.write_flush()?;
        self.delay();
        Ok(new_eecd)
    }

    fn shift_in_ee_bits(&self, count: u16) -> Result<u16, ()> {
        let mut eecd: u32;
        let mut data: u16 = 0;

        /*
         * In order to read a register from the EEPROM, we need to shift 'count'
         * bits in from the EEPROM. Bits are "shifted in" by raising the clock
         * input to the EEPROM (setting the SK bit), and then reading the value
         * of the "DO" bit.  During this "shifting in" process the "DI" bit
         * should always be clear.
         */
        eecd = self.read(CTRL_EECD)?;

        eecd &= !(E1000_EECD_DO | E1000_EECD_DI);

        for i in 0..count {
            data = data << 1;
            self.raise_ee_clk(eecd)?;

            eecd = self.read(CTRL_EECD)?;

            eecd &= !(E1000_EECD_DI);
            if (eecd & E1000_EECD_DO != 0) {
                data |= 1;
            }

            self.lower_ee_clk(eecd)?;
        }

        Ok(data)
    }

    fn shift_out_ee_bits(&self, data: u32, count: u32) -> Result<(), ()> {
        let mut eecd: u32;
        let mut mask: u32;

        /*
         * We need to shift "count" bits out to the EEPROM. So, value in the
         * "data" parameter will be shifted out to the EEPROM one bit at a time.
         * In order to do this, "data" must be broken down into bits.
         */
        mask = 0x01 << (count - 1);
        eecd = self.read(CTRL_EECD)?;

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

            self.write(CTRL_EECD, eecd)?;

            // write flush
            self.read(STATUS)?;

            eecd = self.raise_ee_clk(eecd)?;
            eecd = self.lower_ee_clk(eecd)?;

            mask = mask >> 1;
        }

        /* We leave the "DI" bit set to "0" when we leave this routine. */
        eecd &= !E1000_EECD_DI;
        self.write(CTRL_EECD, eecd)?;

        self.read(CTRL_EECD)?;
        Ok(())
    }

    pub fn read_eeprom(&self, offset: u16, words: u16) -> Result<u16, ()> {
        let mut data: u16 = 0;
        for i in 0..words {
            /* Send the READ command (opcode + addr)  */
            self.shift_out_ee_bits(EEPROM_READ_OPCODE_MICROWIRE, 3)?;

            self.shift_out_ee_bits(offset as u32 + i as u32, 6)?;

            /*
             * Read the data.  For microwire, each word requires the
             * overhead of eeprom setup and tear-down.
             */
            data = data | (self.shift_in_ee_bits(16)? << (8 * i));
            self.standby_eeprom()?;
        }

        Ok(data)
    }

    /**
     * Verifies that the EEPROM has a valid checksum
     *
     * Reads the first 64 16 bit words of the EEPROM and sums the values read.
     * If the the sum of the 64 16 bit words is 0xBABA, the EEPROM's checksum is
     * valid.
     */
    pub fn checksum_eeprom(&self) -> Result<(bool), ()> {
        let mut checksum: u16 = 0;
        for i in 0..EEPROM_CHECKSUM_REG + 1 {
            let data: u16 = self.read_eeprom(i, 1)?;
            // crate::println!("data at {} is {:x}", i, data);
            checksum = checksum.wrapping_add(data);
        }

        crate::println!("eeprom checksum is {:x}", checksum);

        Ok(checksum == EEPROM_SUM)
    }

    /*
     * Reads the adapter's MAC address from the EEPROM and inverts the LSB for the
     * second function of dual function devices
     */
    pub fn read_mac_addr(&self) -> Result<[u8; 6], ()> {
        let mut mac: [u8; 6] = [0; 6];

        let mut offset: u16 = 0;
        let mut eeprom_data: u16 = 0;

        for i in (0..NODE_ADDRESS_SIZE).step_by(2) {
            offset = i as u16 >> 1;
            eeprom_data = self.read_eeprom(offset, 1)?;
            mac[i] = eeprom_data as u8 & 0x00FF;
            mac[i + 1] = eeprom_data.wrapping_shr(8) as u8;
        }

        Ok(mac)
    }

    pub fn reset(&mut self) -> Result<(), ()> {
        /* Clear interrupt mask to stop board from generating interrupts */
        self.write(E1000_IMC, 0xffffffff)?;

        /*
         * Disable the Transmit and Receive units.  Then delay to allow
         * any pending transactions to complete before we hit the MAC with
         * the global reset.
         */
        self.write(E1000_RCTL, 0)?;
        self.write(E1000_TCTL, E1000_TCTL_PSP as u32)?;
        self.write_flush()?;

        /* The tbi_compatibility_on Flag must be cleared when Rctl is cleared. */
        self.tbi_compatibility_en = false;

        /*
         * Delay to allow any outstanding PCI transactions to complete before
         * resetting the device
         */
        self.delay(); // FIXME: should be 10 msec

        let ctrl = self.read(CTRL)?;
        println!("In reset, ctrl is {:x}", ctrl);

        /*
         * This controller can't ack the 64-bit write when issuing the
         * reset, so use IO-mapping as a workaround to issue the reset
         */
        self.write_command(CTRL, (ctrl | E1000_CTRL_RST));

        self.delay(); // FIXME: should be 5 msec

        /* Disable HW ARPs on ASF enabled adapters */
        let mut manc: u32 = self.read(E1000_MANC)?;
        manc &= !(E1000_MANC_ARP_EN);
        println!("Writing manc {:x}", manc);
        self.write(E1000_MANC, manc)?;

        /* Clear interrupt mask to stop board from generating interrupts */
        self.write(E1000_IMC, 0xffffffff)?;

        /* Clear any pending interrupt events. */
        let icr: u32 = self.read(E1000_ICR)?;
        println!("ICR is {:x}", icr);

        Ok(())
    }

    /**
     * Performs basic configuration of the adapter.
     *
     * Assumes that the controller has previously been reset and is in a
     * post-reset uninitialized state. Initializes the receive address registers,
     * multicast table, and VLAN filter table. Calls routines to setup link
     * configuration and flow control settings. Clears all on-chip counters. Leaves
     * the transmit and receive units disabled and uninitialized.
     */
    pub fn init(&mut self) -> Result<(), ()> {
        /* Initialize Identification LED */
        self.id_led_init()?;

        /* Disabling VLAN filtering. */
        self.write(E1000_VET, 0)?;

        //e1000_clear_vfta(hw);

        Ok(())
    }

    fn id_led_init(&mut self) -> Result<(), ()> {
        let ledctl_mask: u32 = 0x000000FF;
        let led_mask: u16 = 0x0F;

        let mut temp: u16;

        let mut ledctl: u32 = self.read(E1000_LEDCTL)?;

        self.ledctl_default = ledctl;
        self.ledctl_mode1 = ledctl;
        self.ledctl_mode2 = ledctl;

        let mut eeprom_data: u16 = self.read_eeprom(EEPROM_ID_LED_SETTINGS, 1)?;

        if ((eeprom_data == ID_LED_RESERVED_0000) || (eeprom_data == ID_LED_RESERVED_FFFF)) {
            eeprom_data = ID_LED_DEFAULT;
        }

        for i in 0..4 {
            temp = (eeprom_data >> (i << 2)) & led_mask;
            match temp {
                ID_LED_ON1_DEF2 | ID_LED_ON1_ON2 | ID_LED_ON1_OFF2 => {
                    self.ledctl_mode1 &= !(ledctl_mask << (i << 3));
                    self.ledctl_mode1 |= E1000_LEDCTL_MODE_LED_ON << (i << 3);
                }
                ID_LED_OFF1_DEF2 | ID_LED_OFF1_ON2 | ID_LED_OFF1_OFF2 => {
                    self.ledctl_mode1 &= !(ledctl_mask << (i << 3));
                    self.ledctl_mode1 |= E1000_LEDCTL_MODE_LED_OFF << (i << 3);
                }
                _ => {}
            };

            match temp {
                ID_LED_DEF1_ON2 | ID_LED_ON1_ON2 | ID_LED_OFF1_ON2 => {
                    self.ledctl_mode2 &= !(ledctl_mask << (i << 3));
                    self.ledctl_mode2 |= E1000_LEDCTL_MODE_LED_ON << (i << 3);
                }
                ID_LED_DEF1_OFF2 | ID_LED_ON1_OFF2 | ID_LED_OFF1_OFF2 => {
                    self.ledctl_mode2 &= !(ledctl_mask << (i << 3));
                    self.ledctl_mode2 |= E1000_LEDCTL_MODE_LED_OFF << (i << 3);
                }
                _ => {}
            }
        }

        println!("{:x} {:x}", self.ledctl_mode1, self.ledctl_mode2);

        Ok(())
    }

    /**
     * e1000_clear_vfta - Clears the VLAN filer table
     * @hw: Struct containing variables accessed by shared code
     */
    fn clear_vfta(&self) -> Result<(), ()> {
	    let vfta_offset: u32 = 0;
	    let mut vfta_bit_in_reg: u32 = 0;

        for offset in 0..E1000_VLAN_FILTER_TBL_SIZE {
            /*
             * If the offset we want to clear is the same offset of the
             * manageability VLAN ID, then clear all bits except that of the
             * manageability unit
             */
            let vfta_value:u32 =
                match offset == vfta_offset {
                    true => vfta_bit_in_reg,
                    _ => 0,
            };
            
            println!("Writing {:x} to {:x}", vfta_value, E1000_VFTA + (offset << 2));
            self.write(E1000_VFTA + (offset << 2), vfta_value)?;
            self.write_flush()?
        }

        Ok(());
    }
}
