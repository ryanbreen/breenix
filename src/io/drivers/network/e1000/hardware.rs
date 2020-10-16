use core::ptr;

use macaddr::MacAddr;

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
    pub(in crate::io::drivers::network::e1000) mac: MacAddr,
    pub(in crate::io::drivers::network::e1000) media_type: MediaType,
    pub(in crate::io::drivers::network::e1000) bus_type: BusType,
    pub(in crate::io::drivers::network::e1000) bus_speed: BusSpeed,
    pub(in crate::io::drivers::network::e1000) bus_width: BusWidth,
    pub(in crate::io::drivers::network::e1000) phy_id: u32,
    pub(in crate::io::drivers::network::e1000) mtu: u16,
    pub(in crate::io::drivers::network::e1000) max_frame_size: u32,
    pub(in crate::io::drivers::network::e1000) fc_high_water: u16,
    pub(in crate::io::drivers::network::e1000) fc_low_water: u16,
    pub(in crate::io::drivers::network::e1000) fc_pause_time: u16,
    pub(in crate::io::drivers::network::e1000) fc_send_xon: bool,
    pub(in crate::io::drivers::network::e1000) fc: FlowControlSettings,
    pub(in crate::io::drivers::network::e1000) original_fc: FlowControlSettings,
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
            phy_id: 0,
            mac_type: MacType::e1000_82540,
            mac: MacAddr::from([0, 0, 0, 0, 0, 0]),
            media_type: MediaType::e1000_media_type_copper,
            revision_id: 0x3,
            mtu: 0x5dc,
            max_frame_size: 0x5ee,
            fc_high_water: 0,
            fc_low_water: 0,
            fc_pause_time: 0,
            fc_send_xon: false,
            fc: FlowControlSettings::E1000_FC_DEFAULT,
            original_fc: FlowControlSettings::E1000_FC_DEFAULT,
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

    pub fn write_array(&self, offset: u32, idx: u32, val: u32) -> Result<(), ()> {
        self.write(offset + (idx << 2), val)
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
    pub(in crate::io::drivers::network::e1000) fn load_mac_addr(&mut self) -> Result<(), ()> {
        let mut macbytes: [u8; 6] = [0; 6];

        let mut offset: u16 = 0;
        let mut eeprom_data: u16 = 0;

        for i in (0..NODE_ADDRESS_SIZE).step_by(2) {
            offset = i as u16 >> 1;
            eeprom_data = self.read_eeprom(offset, 1)?;
            macbytes[i] = eeprom_data as u8 & 0x00FF;
            macbytes[i + 1] = eeprom_data.wrapping_shr(8) as u8;
        }

        self.mac = MacAddr::from(macbytes);

        Ok(())
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

        self.clear_vfta()?;

        /*
         * Setup the receive address. This involves initializing all of the
         * Receive Address Registers (RARs 0 - 15).
         */
        self.init_rx_addrs()?;

        println!("Zeroing the MTA");
        for i in 0..E1000_MC_TBL_SIZE {
            self.write_array(E1000_MTA, i, 0)?;
            /*
             * use write flush to prevent Memory Write Block (MWB) from
             * occurring when accessing our register space
             */
            self.write_flush()?;
        }

        self.setup_link()?;

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
     * e1000_clear_vfta - Clear the VLAN filer table
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
            let vfta_value: u32 = match offset == vfta_offset {
                true => vfta_bit_in_reg,
                _ => 0,
            };

            self.write_array(E1000_VFTA, offset, vfta_value)?;
            self.write_flush()?
        }

        Ok(())
    }

    /**
     * e1000_init_rx_addrs - Initializes receive address filters.
     *
     * Places the MAC address in receive address register 0 and clears the rest
     * of the receive address registers. Clears the multicast table. Assumes
     * the receiver is in reset when the routine is called.
     */
    fn init_rx_addrs(&self) -> Result<(), ()> {
        /* Setup the receive address. */
        self.rar_set(self.mac.as_bytes(), 0);

        /*
         * Zero out the following 14 receive addresses. RAR[15] is for
         * manageability
         */
        for i in 1..E1000_RAR_ENTRIES as u32 {
            self.write_array(E1000_RA, (i << 1), 0)?;
            self.write_flush()?;
            self.write_array(E1000_RA, ((i << 1) + 1), 0)?;
            self.write_flush()?;
        }
        Ok(())
    }

    /**
     * e1000_rar_set - Puts an ethernet address into a receive address register.
     * @addr: Address to put into receive address register
     * @index: Receive address register to write
     */
    fn rar_set(&self, addr: &[u8], index: u32) -> Result<(), ()> {
        let mut rar_low: u32 = 0;
        let mut rar_high: u32 = 0;

        /*
         * HW expects these in little endian so we reverse the byte order
         * from network order (big endian) to little endian
         */
        rar_low = (addr[0] as u32
            | ((addr[1] as u32) << 8)
            | ((addr[2] as u32) << 16)
            | ((addr[3] as u32) << 24));
        rar_high = (addr[4] as u32 | ((addr[5] as u32) << 8));

        /* Indicate to hardware the Address is Valid. */
        rar_high |= E1000_RAH_AV;

        self.write_array(E1000_RA, (index << 1), rar_low)?;
        self.write_flush()?;
        self.write_array(E1000_RA, ((index << 1) + 1), rar_high)?;
        self.write_flush()
    }

    fn setup_link(&mut self) -> Result<(), ()> {
        let mut eeprom_data: u16;

        /*
         * Read and store word 0x0F of the EEPROM. This word contains bits
         * that determine the hardware's default PAUSE (flow control) mode,
         * a bit that determines whether the HW defaults to enabling or
         * disabling auto-negotiation, and the direction of the
         * SW defined pins. If there is no SW over-ride of the flow
         * control setting, then the variable hw->fc will
         * be initialized based on a value in the EEPROM.
         */
        if (self.fc == FlowControlSettings::E1000_FC_DEFAULT) {
            eeprom_data = self.read_eeprom(EEPROM_INIT_CONTROL2_REG, 1)?;
            if ((eeprom_data & EEPROM_WORD0F_PAUSE_MASK) == 0) {
                self.fc = FlowControlSettings::E1000_FC_NONE;
            } else if ((eeprom_data & EEPROM_WORD0F_PAUSE_MASK) == EEPROM_WORD0F_ASM_DIR) {
                self.fc = FlowControlSettings::E1000_FC_TX_PAUSE;
            } else {
                self.fc = FlowControlSettings::E1000_FC_FULL;
            }
        }

        /*
         * We want to save off the original Flow Control configuration just
         * in case we get disconnected and then reconnected into a different
         * hub or switch with different Flow Control capabilities.
         */
        self.original_fc = self.fc;

        println!("After fix-ups FlowControl is now = {:x}", self.fc as u32);

        /* Call the necessary subroutine to configure the link. */
        match self.media_type {
            MediaType::e1000_media_type_copper => self.setup_copper_link()?,
            _ => return Err(()),
        };

        /* Initialize the flow control address, type, and PAUSE timer
         * registers to their default values.  This is done even if flow
         * control is disabled, because it does not hurt anything to
         * initialize these registers.
         *
        e_dbg("Initializing the Flow Control address, type and timer regs\n");

        ew32(FCT, FLOW_CONTROL_TYPE);
        ew32(FCAH, FLOW_CONTROL_ADDRESS_HIGH);
        ew32(FCAL, FLOW_CONTROL_ADDRESS_LOW);

        ew32(FCTTV, hw->fc_pause_time);

        * Set the flow control receive threshold registers.  Normally,
         * these registers will be set to a default threshold that may be
         * adjusted later by the driver's runtime code.  However, if the
         * ability to transmit pause frames in not enabled, then these
         * registers will be set to 0.
         *
        if (!(hw->fc & E1000_FC_TX_PAUSE)) {
            ew32(FCRTL, 0);
            ew32(FCRTH, 0);
        } else {
            * We need to set up the Receive Threshold high and low water
             * marks as well as (optionally) enabling the transmission of
             * XON frames.
             *
            if (hw->fc_send_xon) {
                ew32(FCRTL, (hw->fc_low_water | E1000_FCRTL_XONE));
                ew32(FCRTH, hw->fc_high_water);
            } else {
                ew32(FCRTL, hw->fc_low_water);
                ew32(FCRTH, hw->fc_high_water);
            }
        }*/
        Ok(())
    }

    /**
     * e1000_phy_hw_reset - reset the phy, hardware style
     *
     * Returns the PHY to the power-on reset state
     */
    fn phy_hw_reset(&mut self) -> Result<(), ()> {
        /*
         * Read the Extended Device Control Register, assert the
         * PHY_RESET_DIR bit to put the PHY into reset. Then, take it
         * out of reset.
         */
        let mut ctrl_ext = self.read(E1000_CTRL_EXT)?;
        ctrl_ext |= E1000_CTRL_EXT_SDP4_DIR;
        ctrl_ext &= !E1000_CTRL_EXT_SDP4_DATA;
        self.write(E1000_CTRL_EXT, ctrl_ext)?;
        self.write_flush()?;

        self.delay();
        //msleep(10);

        ctrl_ext |= E1000_CTRL_EXT_SDP4_DATA;
        self.write(E1000_CTRL_EXT, ctrl_ext)?;
        self.write_flush()?;

        self.delay();
        // udelay(150);

        /* Wait for FW to finish PHY configuration. */
        //msleep(10);
        self.delay();

        Ok(())
    }

    /**
     * e1000_read_phy_reg - read a phy register
     * @reg_addr: address of the PHY register to read
     * @phy_data: pointer to the value on the PHY register
     *
     * Reads the value from a PHY register, if the value is on a specific non zero
     * page, sets the page first.
     */
    fn read_phy_reg(&self, reg_addr: u32) -> Result<u16, ()> {
        // Linux does a lock here, but I can't be bothered
        // spin_lock_irqsave(&e1000_phy_lock, flags);

        let address = MAX_PHY_REG_ADDRESS & reg_addr;
        let phy_addr: u32 = 1;

        let mut mdic: u32 = ((reg_addr << E1000_MDIC_REG_SHIFT)
            | (phy_addr << E1000_MDIC_PHY_SHIFT)
            | (E1000_MDIC_OP_READ));

        self.write(E1000_MDIC, mdic)?;

        /* Poll the ready bit to see if the MDI read
         * completed
         */
        for i in 0..64 {
            //udelay(50);
            self.delay();
            mdic = self.read(E1000_MDIC)?;
            if (mdic & E1000_MDIC_READY != 0) {
                break;
            }
        }

        if (mdic & E1000_MDIC_READY == 0) {
            //e_dbg("MDI Read did not complete\n");
            return Err(());
        }
        if (mdic & E1000_MDIC_ERROR != 0) {
            //e_dbg("MDI Error\n");
            return Err(());
        }

        println!("Got mdic {:x}", mdic);

        // spin_unlock_irqrestore(&e1000_phy_lock, flags);

        Ok(mdic as u16)
    }

    /**
     * e1000_detect_gig_phy - check the phy type
     *
     * Probes the expected PHY address for known PHY IDs
     */
    fn detect_gig_phy(&mut self) -> Result<(), ()> {
        // Work is already done, so no-op this
        if (self.phy_id != 0) {
            return Ok(());
        }

        let mut matched: bool = false;

        /* Read the PHY ID Registers to identify which PHY is onboard. */
        self.phy_id = (self.read_phy_reg(PHY_ID1)? as u32) << 16;
        println!("phy_id is now {:x}", self.phy_id);

        self.delay();
        // udelay(20);

        self.phy_id |= (self.read_phy_reg(PHY_ID2)? as u32) & PHY_REVISION_MASK;
        println!("phy_id is now {:x}", self.phy_id);

        /*

        hw->phy_id |= (u32)(phy_id_low & PHY_REVISION_MASK);
        hw->phy_revision = (u32)phy_id_low & ~PHY_REVISION_MASK;

        switch (hw->mac_type) {
        case e1000_82543:
            if (hw->phy_id == M88E1000_E_PHY_ID)
                match = true;
            break;
        case e1000_82544:
            if (hw->phy_id == M88E1000_I_PHY_ID)
                match = true;
            break;
        case e1000_82540:
        case e1000_82545:
        case e1000_82545_rev_3:
        case e1000_82546:
        case e1000_82546_rev_3:
            if (hw->phy_id == M88E1011_I_PHY_ID)
                match = true;
            break;
        case e1000_ce4100:
            if ((hw->phy_id == RTL8211B_PHY_ID) ||
                (hw->phy_id == RTL8201N_PHY_ID) ||
                (hw->phy_id == M88E1118_E_PHY_ID))
                match = true;
            break;
        case e1000_82541:
        case e1000_82541_rev_2:
        case e1000_82547:
        case e1000_82547_rev_2:
            if (hw->phy_id == IGP01E1000_I_PHY_ID)
                match = true;
            break;
        default:
            e_dbg("Invalid MAC type %d\n", hw->mac_type);
            return -E1000_ERR_CONFIG;
        }
        phy_init_status = e1000_set_phy_type(hw);

        if ((match) && (phy_init_status == E1000_SUCCESS)) {
            e_dbg("PHY ID 0x%X detected\n", hw->phy_id);
            return E1000_SUCCESS;
        }
        e_dbg("Invalid PHY ID 0x%X\n", hw->phy_id);
        return -E1000_ERR_PHY;
        */
        Ok(())
    }

    /**
     * e1000_copper_link_preconfig - early configuration for copper
     *
     * Make sure we have a valid PHY and change PHY mode before link setup.
     */
    fn copper_link_preconfig(&mut self) -> Result<(), ()> {
        let mut ctrl = self.read(CTRL)?;
        ctrl |= (E1000_CTRL_FRCSPD | E1000_CTRL_FRCDPX | E1000_CTRL_SLU);
        self.write(CTRL, ctrl);

        self.phy_hw_reset()?;

        /* Make sure we have a valid PHY */
        self.detect_gig_phy()?;
        println!("Phy ID = {:x}", self.phy_id);

        /* Set PHY to class A mode (if necessary) */
        //self.set_phy_mode()?;

        Ok(())
    }

    /**
     * e1000_setup_copper_link - phy/speed/duplex setting
     *
     * Detects which PHY is present and sets up the speed and duplex
     */
    fn setup_copper_link(&mut self) -> Result<(), ()> {
        let mut phy_data: u16;

        /* Check if it is a valid PHY and set PHY mode if necessary. */
        self.copper_link_preconfig()?;

        /*
        if (hw->phy_type == e1000_phy_igp) {
            ret_val = e1000_copper_link_igp_setup(hw);
            if (ret_val)
                return ret_val;
        } else if (hw->phy_type == e1000_phy_m88) {
            ret_val = e1000_copper_link_mgp_setup(hw);
            if (ret_val)
                return ret_val;
        } else {
            ret_val = gbe_dhg_phy_setup(hw);
            if (ret_val) {
                e_dbg("gbe_dhg_phy_setup failed!\n");
                return ret_val;
            }
        }

        if (hw->autoneg) {
            /* Setup autoneg and flow control advertisement
            * and perform autonegotiation
            */
            ret_val = e1000_copper_link_autoneg(hw);
            if (ret_val)
                return ret_val;
        } else {
            /* PHY will be set to 10H, 10F, 100H,or 100F
            * depending on value from forced_speed_duplex.
            */
            e_dbg("Forcing speed and duplex\n");
            ret_val = e1000_phy_force_speed_duplex(hw);
            if (ret_val) {
                e_dbg("Error Forcing Speed and Duplex\n");
                return ret_val;
            }
        }

        * Check link status. Wait up to 100 microseconds for link to become
        * valid.
        *
        for (i = 0; i < 10; i++) {
            ret_val = e1000_read_phy_reg(hw, PHY_STATUS, &phy_data);
            if (ret_val)
                return ret_val;
            ret_val = e1000_read_phy_reg(hw, PHY_STATUS, &phy_data);
            if (ret_val)
                return ret_val;

            if (phy_data & MII_SR_LINK_STATUS) {
                /* Config the MAC and PHY after link is up */
                ret_val = e1000_copper_link_postconfig(hw);
                if (ret_val)
                    return ret_val;

                e_dbg("Valid link established!!!\n");
                return E1000_SUCCESS;
            }
            udelay(10);
        }

        e_dbg("Unable to establish link!!!\n");
        return E1000_SUCCESS;
        */
        Ok(())
    }
}
