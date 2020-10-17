use crate::println;

use crate::io::drivers::DeviceDriver;
use crate::io::pci;

mod constants;
mod hardware;
mod params;

use self::constants::*;
use crate::io::drivers::network::vlan::*;

pub struct E1000 {
    //pci_device: pci::Device,
    hardware: self::hardware::Hardware,
    mng_vlan_id: u16,
    //phy_info:
}

#[allow(unused_mut, unused_assignments)]
impl E1000 {
    pub fn new(device: pci::Device) -> Result<E1000, ()> {
        let mut e1000: E1000 = E1000 {
            hardware: self::hardware::Hardware::new(device)?,
            mng_vlan_id: 0,
            //phy_info:
        };

        e1000.initialize()?;
        Ok(e1000)
    }

    fn vlan_used(&self) -> bool {
        /*
        // FIXME: I will eventually need to support this.
        u16 vid;

        for_each_set_bit(vid, adapter->active_vlans, VLAN_N_VID)
            return true;
            */
        false
    }

    fn update_mng_vlan(&self) -> Result<(), ()> {
        let vid = self.hardware.mng_cookie.vlan_id;
        let old_vid = self.mng_vlan_id;

        if !self.vlan_used() {
            return Ok(());
        }

        // FIXME: I will eventually need to support this.
        /*
        if (!test_bit(vid, adapter->active_vlans)) {
            if (hw->mng_cookie.status &
                E1000_MNG_DHCP_COOKIE_STATUS_VLAN_SUPPORT) {
                e1000_vlan_rx_add_vid(netdev, htons(ETH_P_8021Q), vid);
                adapter->mng_vlan_id = vid;
            } else {
                adapter->mng_vlan_id = E1000_MNG_VLAN_NONE;
            }
            if ((old_vid != (u16)E1000_MNG_VLAN_NONE) &&
                (vid != old_vid) &&
                !test_bit(old_vid, adapter->active_vlans))
                e1000_vlan_rx_kill_vid(netdev, htons(ETH_P_8021Q),
                            old_vid);
        } else {
            adapter->mng_vlan_id = vid;
        }*/

        Ok(())
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
            (pba << 10) * 9 / 10,
            (pba << 10) - self.hardware.max_frame_size,
        );
        self.hardware.fc_high_water = hwm & 0xFFF8;
        self.hardware.fc_low_water = self.hardware.fc_high_water - 8;
        self.hardware.fc_pause_time = FC_PAUSE_TIME;
        self.hardware.fc_send_xon = true;
        self.hardware.fc = FlowControlSettings::E1000FCDefault;

        self.hardware.reset()?;

        self.hardware.init()?;

        self.update_mng_vlan()?;

        /* Enable h/w to recognize an 802.1Q VLAN Ethernet packet */
        self.hardware.write(E1000_VET, ETHERNET_IEEE_VLAN_TYPE)?;

        self.hardware.reset_adaptive()?;

        //self.phy_info = self.hardware.phy_get_info()?;

        /*

        e1000_release_manageability(adapter);
        */

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
            println!("MAC is {}", self.hardware.mac);

            let control_port = self
                .hardware
                .read_eeprom(self::constants::EEPROM_INIT_CONTROL3_PORT_A, 1)?;

            let mut wol = 0;

            println!("Control port is {:x}", control_port);

            if control_port & EEPROM_APME != 0 {
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
