
use core::ptr;

use macaddr::MacAddr;

use crate::println;

use crate::io::pci;
use crate::io::pci::BAR;

use crate::io::drivers::network::e1000::constants::*;

mod eeprom;

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub(in crate::io::drivers::network::e1000) struct DHCPCookie {
    pub(in crate::io::drivers::network::e1000) signature: u32,
    pub(in crate::io::drivers::network::e1000) status: u8,
    pub(in crate::io::drivers::network::e1000) reserved0: u8,
    pub(in crate::io::drivers::network::e1000) vlan_id: u16,
    pub(in crate::io::drivers::network::e1000) reserved1: u32,
    pub(in crate::io::drivers::network::e1000) reserved2: u16,
    pub(in crate::io::drivers::network::e1000) reserved3: u8,
    pub(in crate::io::drivers::network::e1000) checksum: u8,
}

impl DHCPCookie {
    pub fn empty() -> DHCPCookie {
        DHCPCookie {
            signature: 0,
            status: 0,
            reserved0: 0,
            vlan_id: 0,
            reserved1: 0,
            reserved2: 0,
            reserved3: 0,
            checksum: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::io::drivers::network::e1000) struct PhyInfo {
    pub(in crate::io::drivers::network::e1000) cable_length: CableLength,
    pub(in crate::io::drivers::network::e1000) extended_10bt_distance: TenBTExtDistEnable,
    pub(in crate::io::drivers::network::e1000) cable_polarity: RevPolarity,
    pub(in crate::io::drivers::network::e1000) downshift: Downshift,
    pub(in crate::io::drivers::network::e1000) polarity_correction: PolarityReversal,
    pub(in crate::io::drivers::network::e1000) mdix_mode: AutoXMode,
    pub(in crate::io::drivers::network::e1000) local_rx: RXStatus,
    pub(in crate::io::drivers::network::e1000) remote_rx: RXStatus,
}

impl PhyInfo {
    pub fn defaults() -> PhyInfo {
        PhyInfo {
            cable_length: CableLength::Undefined,
            extended_10bt_distance: TenBTExtDistEnable::Undefined,
            cable_polarity: RevPolarity::Undefined,
            downshift: Downshift::Undefined,
            polarity_correction: PolarityReversal::Undefined,
            mdix_mode: AutoXMode::Undefined,
            local_rx: RXStatus::Undefined,
            remote_rx: RXStatus::Undefined,
        }
    }
}

pub(in crate::io::drivers::network::e1000) struct Hardware {
    io_base: BAR,
    hw_addr: BAR,
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
    pub(in crate::io::drivers::network::e1000) phy_type: PhyType,
    pub(in crate::io::drivers::network::e1000) phy_revision: u32,
    pub(in crate::io::drivers::network::e1000) mtu: u16,
    pub(in crate::io::drivers::network::e1000) max_frame_size: u32,
    pub(in crate::io::drivers::network::e1000) fc_high_water: u32,
    pub(in crate::io::drivers::network::e1000) fc_low_water: u32,
    pub(in crate::io::drivers::network::e1000) fc_pause_time: u32,
    pub(in crate::io::drivers::network::e1000) fc_send_xon: bool,
    pub(in crate::io::drivers::network::e1000) fc: FlowControlSettings,
    pub(in crate::io::drivers::network::e1000) original_fc: FlowControlSettings,
    current_ifs_val: u16,
    ifs_min_val: u16,
    ifs_max_val: u16,
    ifs_step_size: u16,
    ifs_ratio: u16,
    in_ifs_mode: bool,
    pub(in crate::io::drivers::network::e1000) autoneg_advertised: u16,
    pub(in crate::io::drivers::network::e1000) get_link_status: bool,
    pub(in crate::io::drivers::network::e1000) wait_autoneg_complete: bool,
    pub(in crate::io::drivers::network::e1000) tbi_compatibility_en: bool,
    pub(in crate::io::drivers::network::e1000) adaptive_ifs: bool,
    pub(in crate::io::drivers::network::e1000) mdix: u8,
    pub(in crate::io::drivers::network::e1000) disable_polarity_correction: bool,
    pub(in crate::io::drivers::network::e1000) master_slave: MasterSlaveType,
    pub(in crate::io::drivers::network::e1000) ledctl_default: u32,
    pub(in crate::io::drivers::network::e1000) ledctl_mode1: u32,
    pub(in crate::io::drivers::network::e1000) ledctl_mode2: u32,
    pub(in crate::io::drivers::network::e1000) mng_cookie: DHCPCookie,
    speed_downgraded: bool,
}

#[allow(unused_mut, unused_assignments)]
impl Hardware {
    pub fn new(device: pci::Device) -> Hardware {
        Hardware {
            io_base: device.bar(0x1),
            hw_addr: device.bar(0x0),
            /* below vendor fields are pulled from Linux running on Qemu */
            vendor_id: 0x8086, /* Intel */
            device_id: 0x100e, /* e1000 */
            subsystem_vendor_id: 0x1af4,
            subsystem_id: 0x1100,
            bus_type: BusType::E1000BusTypeUnknown,
            bus_speed: BusSpeed::E1000BusSpeedUnknown,
            bus_width: BusWidth::E1000BusWidthUnknown,
            phy_id: 0,
            phy_revision: 0,
            phy_type: PhyType::E1000PhyUndefined,
            mac_type: MacType::E100082540,
            mac: MacAddr::from([0, 0, 0, 0, 0, 0]),
            media_type: MediaType::E1000MediaTypeCopper,
            revision_id: 0x3,
            mtu: 0x5dc,
            max_frame_size: 0x5ee,
            fc_high_water: 0,
            fc_low_water: 0,
            fc_pause_time: 0,
            fc_send_xon: false,
            fc: FlowControlSettings::E1000FCDefault,
            original_fc: FlowControlSettings::E1000FCDefault,
            current_ifs_val: 0,
            ifs_min_val: IFS_MIN,
            ifs_max_val: IFS_MAX,
            ifs_step_size: IFS_STEP,
            ifs_ratio: IFS_RATIO,
            in_ifs_mode: false,
            autoneg_advertised: 0,
            get_link_status: false,
            wait_autoneg_complete: false,
            tbi_compatibility_en: true,
            adaptive_ifs: true,
            mdix: AUTO_ALL_MODES,
            disable_polarity_correction: false,
            master_slave: MasterSlaveType::E1000MSHWDefault,
            ledctl_default: 0,
            ledctl_mode1: 0,
            ledctl_mode2: 0,
            mng_cookie: DHCPCookie::empty(),
            speed_downgraded: true,
        }
    }

    pub(in crate::io::drivers::network::e1000) fn init_data(&mut self) -> Result<(), ()> {
        use x86_64::structures::paging::PageTableFlags;
        let res = crate::memory::identity_map_range(
            self.hw_addr.addr,
            self.hw_addr.size,
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
        );

        if !res.is_ok() {
            panic!("Failed to map memory");
        }

        // Make this more real because we're just hardcoding a lot of this

        // Init eeprom
        //self.eeprom.lock().init(&self);

        self.populate_bus_info()?;

        Ok(())
    }

    fn populate_bus_info(&mut self) -> Result<(), ()> {
        let status = self.read(STATUS)?;

        self.bus_type = match status & E1000_STATUS_PCIX_MODE {
            0 => BusType::E1000BusTypePCI,
            _ => BusType::E1000BusTypePCIX,
        };

        self.bus_speed = match status & E1000_STATUS_PCI66 {
            0 => BusSpeed::E1000BusSpeed33,
            _ => BusSpeed::E1000BusSpeed66,
        };

        self.bus_width = match status & E1000_STATUS_BUS64 {
            0 => BusWidth::E1000BusWidth32,
            _ => BusWidth::E1000BusWidth64,
        };

        Ok(())
    }

    pub fn write_command(&self, offset: u32, val: u32) {
        // TODO: Check for invalid ranges to make sure this is safe.
        unsafe {
            ptr::write_volatile(
                (self.io_base.addr + offset as u64) as *const u32 as *mut _,
                val,
            );
        }
    }

    pub fn read_command(&self, offset: u32) -> u32 {
        // TODO: Check for invalid ranges to make sure this is safe.
        unsafe { ptr::read_volatile((self.io_base.addr + offset as u64) as *const u32) }
    }

    pub fn write(&self, offset: u32, val: u32) -> Result<(), ()> {
        // TODO: Check for invalid ranges to make sure this is safe.
        unsafe {
            ptr::write_volatile(
                (self.hw_addr.addr + offset as u64) as *const u32 as *mut _,
                val,
            );
        }
        Ok(())
    }

    pub fn write_array(&self, offset: u32, idx: u32, val: u32) -> Result<(), ()> {
        self.write(offset + (idx << 2), val)
    }

    pub fn read(&self, offset: u32) -> Result<u32, ()> {
        // TODO: Check for invalid ranges to make sure this is safe.
        Ok(unsafe { ptr::read_volatile((self.hw_addr.addr + offset as u64) as *const u32) })
    }

    pub(in crate::io::drivers::network::e1000) fn write_flush(&self) -> Result<(), ()> {
        // write flush
        self.read(STATUS)?;
        Ok(())
    }

    pub(in crate::io::drivers::network::e1000) fn delay(&self) {
        //crate::delay!(EEPROM_DELAY_USEC);
        /*
        for i in 0..1 {
            //udelay(eeprom->delay_usec);
        }*/
    }

    /**
     * Verifies that the EEPROM has a valid checksum
     *
     * Reads the first 64 16 bit words of the EEPROM and sums the values read.
     * If the the sum of the 64 16 bit words is 0xBABA, the EEPROM's checksum is
     * valid.
     */
    pub fn checksum_eeprom(&self) -> Result<(), ()> {
        let mut checksum: u16 = 0;
        for i in 0..EEPROM_CHECKSUM_REG + 1 {
            let data = eeprom::read_eeprom(self, i, 1)?;
            checksum = checksum.wrapping_add(data);
        }

        crate::println!("eeprom checksum is {:x}", checksum);

        if checksum != EEPROM_SUM {
            return Err(());
        }

        Ok(())
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
            eeprom_data = eeprom::read_eeprom(self, offset, 1)?;
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
        self.write_command(CTRL, ctrl | E1000_CTRL_RST);

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

        /* Call a subroutine to configure the link and setup flow control. */
        self.setup_link()?;

        /* Clear all of the statistics registers (clear on read).  It is
         * important that we do this after we have tried to establish link
         * because the symbol error count will increment wildly if there
         * is no link.
         */
        self.clear_hw_cntrs()?;

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

        let mut eeprom_data: u16 = eeprom::read_eeprom(self, EEPROM_ID_LED_SETTINGS, 1)?;

        if eeprom_data == ID_LED_RESERVED_0000 || eeprom_data == ID_LED_RESERVED_FFFF {
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
     * e1000_clear_hw_cntrs - Clears all hardware statistics counters.
     */
    fn clear_hw_cntrs(&self) -> Result<(), ()> {
        let registers = [
            E1000_CRCERRS,
            E1000_SYMERRS,
            E1000_MPC,
            E1000_SCC,
            E1000_ECOL,
            E1000_MCC,
            E1000_LATECOL,
            E1000_COLC,
            E1000_DC,
            E1000_SEC,
            E1000_RLEC,
            E1000_XONRXC,
            E1000_XONTXC,
            E1000_XOFFRXC,
            E1000_XOFFTXC,
            E1000_FCRUC,
            E1000_PRC64,
            E1000_PRC127,
            E1000_PRC255,
            E1000_PRC511,
            E1000_PRC1023,
            E1000_PRC1522,
            E1000_GPRC,
            E1000_BPRC,
            E1000_MPRC,
            E1000_GPTC,
            E1000_GORCL,
            E1000_GORCH,
            E1000_GOTCL,
            E1000_GOTCH,
            E1000_RNBC,
            E1000_RUC,
            E1000_RFC,
            E1000_ROC,
            E1000_RJC,
            E1000_TORL,
            E1000_TORH,
            E1000_TOTL,
            E1000_TOTH,
            E1000_TPR,
            E1000_TPT,
            E1000_PTC64,
            E1000_PTC127,
            E1000_PTC255,
            E1000_PTC511,
            E1000_PTC1023,
            E1000_PTC1522,
            E1000_MPTC,
            E1000_BPTC,
            E1000_ALGNERRC,
            E1000_RXERRC,
            E1000_TNCRS,
            E1000_CEXTERR,
            E1000_TSCTC,
            E1000_TSCTFC,
            E1000_MGTPRC,
            E1000_MGTPDC,
            E1000_MGTPTC,
        ];

        for i in 0..registers.len() {
            self.read(registers[i])?;
        }

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
        self.rar_set(self.mac.as_bytes(), 0)?;

        /*
         * Zero out the following 14 receive addresses. RAR[15] is for
         * manageability
         */
        for i in 1..E1000_RAR_ENTRIES as u32 {
            self.write_array(E1000_RA, i << 1, 0)?;
            self.write_flush()?;
            self.write_array(E1000_RA, (i << 1) + 1, 0)?;
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
        rar_low = addr[0] as u32
            | (addr[1] as u32) << 8
            | (addr[2] as u32) << 16
            | (addr[3] as u32) << 24;
        rar_high = addr[4] as u32 | (addr[5] as u32) << 8;

        /* Indicate to hardware the Address is Valid. */
        rar_high |= E1000_RAH_AV;

        self.write_array(E1000_RA, index << 1, rar_low)?;
        self.write_flush()?;
        self.write_array(E1000_RA, (index << 1) + 1, rar_high)?;
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
        if self.fc == FlowControlSettings::E1000FCDefault {
            eeprom_data = eeprom::read_eeprom(self, EEPROM_INIT_CONTROL2_REG, 1)?;
            if eeprom_data & EEPROM_WORD0F_PAUSE_MASK == 0 {
                self.fc = FlowControlSettings::E1000FCNone;
            } else if eeprom_data & EEPROM_WORD0F_PAUSE_MASK == EEPROM_WORD0F_ASM_DIR {
                self.fc = FlowControlSettings::E1000FCTXPause;
            } else {
                self.fc = FlowControlSettings::E1000FCFull;
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
            MediaType::E1000MediaTypeCopper => self.setup_copper_link()?,
            _ => {
                println!("Unexpected media type");
                return Err(());
            }
        };

        /* Initialize the flow control address, type, and PAUSE timer
         * registers to their default values.  This is done even if flow
         * control is disabled, because it does not hurt anything to
         * initialize these registers.
         */
        println!("Initializing the Flow Control address, type and timer regs");

        self.write(E1000_FCT, FLOW_CONTROL_TYPE)?;
        self.write(E1000_FCAH, FLOW_CONTROL_ADDRESS_HIGH)?;
        self.write(E1000_FCAL, FLOW_CONTROL_ADDRESS_LOW)?;

        self.write(E1000_FCTTV, self.fc_pause_time as u32)?;

        /* Set the flow control receive threshold registers.  Normally,
         * these registers will be set to a default threshold that may be
         * adjusted later by the driver's runtime code.  However, if the
         * ability to transmit pause frames in not enabled, then these
         * registers will be set to 0.
         */
        if ((self.fc as u32) & (FlowControlSettings::E1000FCTXPause as u32)) == 0 {
            self.write(E1000_FCRTL, 0)?;
            self.write(E1000_FCRTH, 0)?;
        } else {
            /* We need to set up the Receive Threshold high and low water
             * marks as well as (optionally) enabling the transmission of
             * XON frames.
             */
            if self.fc_send_xon {
                self.write(E1000_FCRTL, self.fc_low_water | E1000_FCRTL_XONE)?;
                self.write(E1000_FCRTH, self.fc_high_water)?;
            } else {
                self.write(E1000_FCRTL, self.fc_low_water)?;
                self.write(E1000_FCRTH, self.fc_high_water)?;
            }
        }
        Ok(())
    }

    pub fn read_eeprom(&self, offset: u16, words: u16) -> Result<u16, ()> {
        eeprom::read_eeprom(self, offset, words)
    }

    /**
     * e1000_reset_adaptive - Resets Adaptive IFS to its default state.
     *
     * Call this after init_hw. You may override the IFS defaults by setting
     * ifs_params_forced to true. However, you must initialize current_ifs_val,
     * ifs_min_val, ifs_max_val, ifs_step_size, and ifs_ratio before calling
     * this function.
     */
    pub(in crate::io::drivers::network::e1000) fn reset_adaptive(&mut self) -> Result<(), ()> {
        if self.adaptive_ifs {
            self.current_ifs_val = 0;
            self.ifs_min_val = IFS_MIN;
            self.ifs_max_val = IFS_MAX;
            self.ifs_step_size = IFS_STEP;
            self.ifs_ratio = IFS_RATIO;
            self.in_ifs_mode = false;
            self.write(E1000_AIT, 0)?;
        } else {
            println!("Not in Adaptive IFS mode!");
        }

        Ok(())
    }

    pub(in crate::io::drivers::network::e1000) fn populate_phy_info(
        &mut self,
    ) -> Result<PhyInfo, ()> {
        let mut phy_info: PhyInfo = PhyInfo::defaults();

        let _ = self.read_phy_reg(PHY_STATUS)?;
        let mut phy_data = self.read_phy_reg(PHY_STATUS)?;

        if phy_data & MII_SR_LINK_STATUS != MII_SR_LINK_STATUS {
            println!("PHY info is only valid if link is up");
            return Ok(phy_info);
        }

        /*
         * The downshift status is checked only once, after link is established,
         * and it stored in the hw->speed_downgraded parameter.
         */
        phy_info.downshift = match self.speed_downgraded {
            true => Downshift::Activated,
            false => Downshift::Normal,
        };

        phy_data = self.read_phy_reg(M88E1000_PHY_SPEC_CTRL)?;

        phy_info.extended_10bt_distance = match (phy_data & M88E1000_PSCR_10BT_EXT_DIST_ENABLE)
            >> M88E1000_PSCR_10BT_EXT_DIST_ENABLE_SHIFT
        {
            0 => TenBTExtDistEnable::Normal,
            _ => TenBTExtDistEnable::Lower,
        };

        phy_info.polarity_correction = match (phy_data & M88E1000_PSCR_POLARITY_REVERSAL)
            >> M88E1000_PSCR_POLARITY_REVERSAL_SHIFT
        {
            0 => PolarityReversal::Enabled,
            _ => PolarityReversal::Disabled,
        };

        // FIXME once we have a working link and can test it.

        /* Check polarity status *
        ret_val = e1000_check_polarity(hw, &polarity);
        if (ret_val)
            return ret_val;
        phy_info->cable_polarity = polarity;

        ret_val = e1000_read_phy_reg(hw, M88E1000_PHY_SPEC_STATUS, &phy_data);
        if (ret_val)
            return ret_val;

        phy_info->mdix_mode =
            (e1000_auto_x_mode) ((phy_data & M88E1000_PSSR_MDIX) >>
                    M88E1000_PSSR_MDIX_SHIFT);

        if ((phy_data & M88E1000_PSSR_SPEED) == M88E1000_PSSR_1000MBS) {
            /* Cable Length Estimation and Local/Remote Receiver Information
            * are only valid at 1000 Mbps.
            */
            phy_info->cable_length =
                (e1000_cable_length) ((phy_data &
                        M88E1000_PSSR_CABLE_LENGTH) >>
                        M88E1000_PSSR_CABLE_LENGTH_SHIFT);

            ret_val = e1000_read_phy_reg(hw, PHY_1000T_STATUS, &phy_data);
            if (ret_val)
                return ret_val;

            phy_info->local_rx = ((phy_data & SR_1000T_LOCAL_RX_STATUS) >>
                        SR_1000T_LOCAL_RX_STATUS_SHIFT) ?
                e1000_1000t_rx_status_ok : e1000_1000t_rx_status_not_ok;
            phy_info->remote_rx = ((phy_data & SR_1000T_REMOTE_RX_STATUS) >>
                        SR_1000T_REMOTE_RX_STATUS_SHIFT) ?
                e1000_1000t_rx_status_ok : e1000_1000t_rx_status_not_ok;
        }
        */

        Ok(phy_info)
    }

    /**
     * e1000_phy_reset - reset the phy to commit settings
     *
     * Resets the PHY
     * Sets bit 15 of the MII Control register
     */
    fn phy_reset(&self) -> Result<(), ()> {
        let mut phy_data: u16 = self.read_phy_reg(PHY_CTRL)?;
        phy_data |= MII_CR_RESET;
        self.write_phy_reg(PHY_CTRL, phy_data)?;

        //udelay(1);
        self.delay();

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

        let phy_addr: u32 = 1;

        let mut mdic: u32 = reg_addr << E1000_MDIC_REG_SHIFT
            | phy_addr << E1000_MDIC_PHY_SHIFT
            | E1000_MDIC_OP_READ;

        self.write(E1000_MDIC, mdic)?;

        /*
         * Poll the ready bit to see if the MDI read
         * completed
         */
        for _ in 0..64 {
            //udelay(50);
            self.delay();

            mdic = self.read(E1000_MDIC)?;
            if mdic & E1000_MDIC_READY != 0 {
                break;
            }
        }

        if (mdic & E1000_MDIC_READY) == 0 {
            println!("MDI Read did not complete");
            return Err(());
        }
        if (mdic & E1000_MDIC_ERROR) != 0 {
            println!("MDI Read error");
            return Err(());
        }

        // spin_unlock_irqrestore(&e1000_phy_lock, flags);

        Ok(mdic as u16)
    }

    /**
     * e1000_write_phy_reg - write a phy register
     *
     * @reg_addr: address of the PHY register to write
     * @data: data to write to the PHY
     *
     * Writes a value to a PHY register
     */
    fn write_phy_reg(&self, reg_addr: u32, phy_data: u16) -> Result<(), ()> {
        // Linux does a lock here, but I can't be bothered
        // spin_lock_irqsave(&e1000_phy_lock, flags);

        let phy_addr: u32 = 1;

        let mut mdic = (phy_data as u32)
            | (reg_addr << E1000_MDIC_REG_SHIFT)
            | (phy_addr << E1000_MDIC_PHY_SHIFT)
            | (E1000_MDIC_OP_WRITE);

        self.write(E1000_MDIC, mdic)?;

        /*
         * Poll the ready bit to see if the MDI read
         * completed
         */

        for _ in 0..641 {
            //udelay(5);
            self.delay();
            mdic = self.read(E1000_MDIC)?;
            if mdic & E1000_MDIC_READY != 0 {
                break;
            }
        }

        if mdic & E1000_MDIC_READY == 0 {
            println!("MDI write did not complete");
            return Err(());
        }

        // spin_unlock_irqrestore(&e1000_phy_lock, flags);

        Ok(())
    }

    /**
     * e1000_detect_gig_phy - check the phy type
     *
     * Probes the expected PHY address for known PHY IDs
     */
    fn detect_gig_phy(&mut self) -> Result<(), ()> {
        // Work is already done, so no-op this
        if self.phy_id != 0 {
            return Ok(());
        }

        /* Read the PHY ID Registers to identify which PHY is onboard. */
        self.phy_id = (self.read_phy_reg(PHY_ID1)? as u32) << 16;

        self.delay();
        // udelay(20);

        let phy_low = self.read_phy_reg(PHY_ID2)? as u32;

        self.phy_id |= phy_low & PHY_REVISION_MASK;
        println!("phy_id is now {:x}", self.phy_id);

        self.phy_revision = phy_low & !PHY_REVISION_MASK;
        println!("phy_revision is now {:x}", self.phy_revision);

        let matched = self.phy_id == M88E1011_I_PHY_ID;

        match self.phy_id {
            M88E1000_E_PHY_ID | M88E1000_I_PHY_ID | M88E1011_I_PHY_ID | M88E1111_I_PHY_ID
            | M88E1118_E_PHY_ID => {
                self.phy_type = PhyType::E1000PhyM88;
            }
            IGP01E1000_I_PHY_ID => {
                if self.mac_type == MacType::E100082541
                    || self.mac_type == MacType::E100082541Rev2
                    || self.mac_type == MacType::E100082547
                    || self.mac_type == MacType::E100082547Rev2
                {
                    self.phy_type = PhyType::E1000PhyIGP;
                }
            }
            RTL8211B_PHY_ID => {
                self.phy_type = PhyType::E1000Phy8211;
            }
            RTL8201N_PHY_ID => {
                self.phy_type = PhyType::E1000Phy8201;
            }
            _ => {
                /* Should never have loaded on this device */
                self.phy_type = PhyType::E1000PhyUndefined;
                return Err(()); //-E1000_ERR_PHY_TYPE;
            }
        };

        if !matched {
            return Err(()); //-E1000_ERR_PHY;
        }

        println!(
            "PHY ID 0x{:x} detected, type 0x{:x}",
            self.phy_id, self.phy_type as u32
        );
        Ok(())
    }

    /**
     * e1000_copper_link_preconfig - early configuration for copper
     *
     * Make sure we have a valid PHY and change PHY mode before link setup.
     */
    fn copper_link_preconfig(&mut self) -> Result<(), ()> {
        let mut ctrl = self.read(CTRL)?;
        ctrl |= E1000_CTRL_FRCSPD | E1000_CTRL_FRCDPX | E1000_CTRL_SLU;
        self.write(CTRL, ctrl)?;

        self.phy_hw_reset()?;

        /* Make sure we have a valid PHY */
        self.detect_gig_phy()?;
        println!("Phy ID = {:x}", self.phy_id);

        Ok(())
    }

    /**
     * e1000_copper_link_mgp_setup - Copper link setup for e1000_phy_m88 series.
     */
    fn copper_link_mgp_setup(&self) -> Result<(), ()> {
        /* Enable CRS on TX. This must be set for half-duplex operation. */
        let mut phy_data: u16 = self.read_phy_reg(M88E1000_PHY_SPEC_CTRL)?;
        phy_data |= M88E1000_PSCR_ASSERT_CRS_ON_TX;

        /* Options:
         *   MDI/MDI-X = 0 (default)
         *   0 - Auto for all speeds
         *   1 - MDI mode
         *   2 - MDI-X mode
         *   3 - Auto for 1000Base-T only (MDI-X for 10/100Base-T modes)
         */
        phy_data &= !M88E1000_PSCR_AUTO_X_MODE;

        let shifter = match self.mdix {
            1 => M88E1000_PSCR_MDI_MANUAL_MODE,
            2 => M88E1000_PSCR_MDIX_MANUAL_MODE,
            3 => M88E1000_PSCR_AUTO_X_1000T,
            _ => M88E1000_PSCR_AUTO_X_MODE,
        };

        phy_data |= shifter;

        /*
         * Options:
         *   disable_polarity_correction = 0 (default)
         *       Automatic Correction for Reversed Cable Polarity
         *   0 - Disabled
         *   1 - Enabled
         */
        phy_data &= !M88E1000_PSCR_POLARITY_REVERSAL;
        if self.disable_polarity_correction {
            phy_data |= M88E1000_PSCR_POLARITY_REVERSAL;
        }

        self.write_phy_reg(M88E1000_PHY_SPEC_CTRL, phy_data)?;

        if self.phy_revision < M88E1011_I_REV_4 {
            /*
             * Force TX_CLK in the Extended PHY Specific Control Register
             * to 25MHz clock.
             */
            phy_data = self.read_phy_reg(M88E1000_EXT_PHY_SPEC_CTRL)?;
            phy_data |= M88E1000_EPSCR_TX_CLK_25;

            /* Configure Master and Slave downshift values */
            phy_data &=
                !(M88E1000_EPSCR_MASTER_DOWNSHIFT_MASK | M88E1000_EPSCR_SLAVE_DOWNSHIFT_MASK);
            phy_data |= M88E1000_EPSCR_MASTER_DOWNSHIFT_1X | M88E1000_EPSCR_SLAVE_DOWNSHIFT_1X;
            self.write_phy_reg(M88E1000_EXT_PHY_SPEC_CTRL, phy_data)?;
        }

        println!("before reset, phy_data is {:x}", phy_data);

        /* SW Reset the PHY so all changes take effect */
        self.phy_reset()?;

        Ok(())
    }

    /**
     * e1000_phy_setup_autoneg - phy settings
     *
     * Configures PHY autoneg and flow control advertisement settings
     */
    fn phy_setup_autoneg(&self) -> Result<(), ()> {
        /* Read the MII Auto-Neg Advertisement Register (Address 4). */
        let mut mii_autoneg_adv_reg = self.read_phy_reg(PHY_AUTONEG_ADV)?;

        /* Read the MII 1000Base-T Control Register (Address 9). */
        let mut mii_1000t_ctrl_reg = self.read_phy_reg(PHY_1000T_CTRL)?;

        if self.phy_type == PhyType::E1000Phy8201 {
            mii_1000t_ctrl_reg &= !REG9_SPEED_MASK;
        }

        /* Need to parse both autoneg_advertised and fc and set up
         * the appropriate PHY registers.  First we will parse for
         * autoneg_advertised software override.  Since we can advertise
         * a plethora of combinations, we need to check each bit
         * individually.
         */

        /* First we clear all the 10/100 mb speed bits in the Auto-Neg
         * Advertisement Register (Address 4) and the 1000 mb speed bits in
         * the  1000Base-T Control Register (Address 9).
         */
        mii_autoneg_adv_reg &= !REG4_SPEED_MASK;
        mii_1000t_ctrl_reg &= !REG9_SPEED_MASK;

        println!("autoneg_advertised {:x}", self.autoneg_advertised);

        /* Do we want to advertise 10 Mb Half Duplex? */
        if self.autoneg_advertised & ADVERTISE_10_HALF != 0 {
            println!("Advertise 10mb Half duplex");
            mii_autoneg_adv_reg |= NWAY_AR_10T_HD_CAPS;
        }

        /* Do we want to advertise 10 Mb Full Duplex? */
        if self.autoneg_advertised & ADVERTISE_10_FULL != 0 {
            println!("Advertise 10mb Full duplex");
            mii_autoneg_adv_reg |= NWAY_AR_10T_FD_CAPS;
        }

        /* Do we want to advertise 100 Mb Half Duplex? */
        if self.autoneg_advertised & ADVERTISE_100_HALF != 0 {
            println!("Advertise 100mb Half duplex");
            mii_autoneg_adv_reg |= NWAY_AR_100TX_HD_CAPS;
        }

        /* Do we want to advertise 100 Mb Full Duplex? */
        if self.autoneg_advertised & ADVERTISE_100_FULL != 0 {
            println!("Advertise 100mb Full duplex");
            mii_autoneg_adv_reg |= NWAY_AR_100TX_FD_CAPS;
        }

        /* We do not allow the Phy to advertise 1000 Mb Half Duplex */
        if self.autoneg_advertised & ADVERTISE_1000_HALF != 0 {
            println!("Advertise 1000mb Half duplex requested, request denied!");
        }

        /* Do we want to advertise 1000 Mb Full Duplex? */
        if self.autoneg_advertised & ADVERTISE_1000_FULL != 0 {
            println!("Advertise 1000mb Full duplex");
            mii_1000t_ctrl_reg |= CR_1000T_FD_CAPS;
        }

        /* Check for a software override of the flow control settings, and
         * setup the PHY advertisement registers accordingly.  If
         * auto-negotiation is enabled, then software will have to set the
         * "PAUSE" bits to the correct value in the Auto-Negotiation
         * Advertisement Register (PHY_AUTONEG_ADV) and re-start
         * auto-negotiation.
         *
         * The possible values of the "fc" parameter are:
         *      0:  Flow control is completely disabled
         *      1:  Rx flow control is enabled (we can receive pause frames
         *          but not send pause frames).
         *      2:  Tx flow control is enabled (we can send pause frames
         *          but we do not support receiving pause frames).
         *      3:  Both Rx and TX flow control (symmetric) are enabled.
         *  other:  No software override.  The flow control configuration
         *          in the EEPROM is used.
         */
        match self.fc {
            FlowControlSettings::E1000FCNone =>
            /* 0 */
            {
                /* Flow control (RX & TX) is completely disabled by a
                 * software over-ride.
                 */
                mii_autoneg_adv_reg &= !(NWAY_AR_ASM_DIR | NWAY_AR_PAUSE);
            }
            FlowControlSettings::E1000FCRXPause =>
            /* 1 */
            {
                /* RX Flow control is enabled, and TX Flow control is
                 * disabled, by a software over-ride.
                 */

                /* Since there really isn't a way to advertise that we are
                 * capable of RX Pause ONLY, we will advertise that we
                 * support both symmetric and asymmetric RX PAUSE.  Later
                 * (in e1000_config_fc_after_link_up) we will disable the
                 * hw's ability to send PAUSE frames.
                 */
                mii_autoneg_adv_reg |= NWAY_AR_ASM_DIR | NWAY_AR_PAUSE;
            }
            FlowControlSettings::E1000FCTXPause =>
            /* 2 */
            {
                /* TX Flow control is enabled, and RX Flow control is
                 * disabled, by a software over-ride.
                 */
                mii_autoneg_adv_reg |= NWAY_AR_ASM_DIR;
                mii_autoneg_adv_reg &= !NWAY_AR_PAUSE;
            }
            FlowControlSettings::E1000FCFull =>
            /* 3 */
            {
                /* Flow control (both RX and TX) is enabled by a software
                 * over-ride.
                 */
                mii_autoneg_adv_reg |= NWAY_AR_ASM_DIR | NWAY_AR_PAUSE;
            }
            _ => {
                println!("Flow control param set incorrectly");
                return Err(());
                //return -E1000_ERR_CONFIG;
            }
        };

        self.write_phy_reg(PHY_AUTONEG_ADV, mii_autoneg_adv_reg)?;
        println!("Auto-Neg Advertising {:x}", mii_autoneg_adv_reg);

        if self.phy_type == PhyType::E1000Phy8201 {
            mii_1000t_ctrl_reg = 0;
        } else {
            self.write_phy_reg(PHY_1000T_CTRL, mii_1000t_ctrl_reg)?;
        }

        Ok(())
    }

    /**
     * e1000_wait_autoneg
     *
     * Blocks until autoneg completes or times out (~4.5 seconds)
     */
    fn wait_autoneg(&self) -> Result<(), ()> {
        println!("Waiting for Auto-Neg to complete.\n");

        /* We will wait for autoneg to complete or 4.5 seconds to expire. */
        for _ in 0..PHY_AUTO_NEG_TIME {
            /* Read the MII Status Register and wait for Auto-Neg
             * Complete bit to be set.
             */
            let _ = self.read_phy_reg(PHY_STATUS)?;
            let phy_data = self.read_phy_reg(PHY_STATUS)?;
            if (phy_data & MII_SR_AUTONEG_COMPLETE) != 0 {
                return Ok(());
            }
        }
        self.delay();
        //msleep(100);

        Ok(())
    }

    /**
     * e1000_copper_link_autoneg - setup auto-neg
     *
     * Setup auto-negotiation and flow control advertisements,
     * and then perform auto-negotiation.
     */
    fn copper_link_autoneg(&mut self) -> Result<(), ()> {
        /* Perform some bounds checking on the hw->autoneg_advertised
         * parameter.  If this variable is zero, then set it to the default.
         */
        self.autoneg_advertised &= AUTONEG_ADVERTISE_SPEED_DEFAULT;

        /* If autoneg_advertised is zero, we assume it was not defaulted
         * by the calling code so we set to advertise full capability.
         */
        if self.autoneg_advertised == 0 {
            self.autoneg_advertised = AUTONEG_ADVERTISE_SPEED_DEFAULT;
        }

        /* IFE/RTL8201N PHY only supports 10/100 */
        if self.phy_type == PhyType::E1000Phy8201 {
            self.autoneg_advertised &= AUTONEG_ADVERTISE_10_100_ALL;
        }

        println!("Reconfiguring auto-neg advertisement params");
        self.phy_setup_autoneg()?;
        println!("Restarting Auto-Neg");

        /* Restart auto-negotiation by setting the Auto Neg Enable bit and
         * the Auto Neg Restart bit in the PHY control register.
         */
        let mut phy_data = self.read_phy_reg(PHY_CTRL)?;
        phy_data |= MII_CR_AUTO_NEG_EN | MII_CR_RESTART_AUTO_NEG;

        self.write_phy_reg(PHY_CTRL, phy_data)?;

        /* Does the user want to wait for Auto-Neg to complete here, or
         * check at a later time (for example, callback routine).
         */
        if self.wait_autoneg_complete {
            self.wait_autoneg()?;
        }

        self.get_link_status = true;

        Ok(())
    }

    /**
     * e1000_setup_copper_link - phy/speed/duplex setting
     *
     * Detects which PHY is present and sets up the speed and duplex
     */
    fn setup_copper_link(&mut self) -> Result<(), ()> {
        /* Check if it is a valid PHY and set PHY mode if necessary. */
        self.copper_link_preconfig()?;

        self.copper_link_mgp_setup()?;

        /*
         * Setup autoneg and flow control advertisement
         * and perform autonegotiation
         */
        self.copper_link_autoneg()?;

        /*
         * Check link status. Wait up to 100 microseconds for link to become
         * valid.
         */
        for _ in 0..10 {
            let _ = self.read_phy_reg(PHY_STATUS)?;
            let _ = self.read_phy_reg(PHY_STATUS)?;

            self.delay();
        }

        Ok(())
    }
}
