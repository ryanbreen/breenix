use crate::println;

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
    phy_info: self::hardware::PhyInfo,
    en_mng_pt: bool,
    rx_buffer_len: u32,
    num_tx_queues: u32,
    num_rx_queues: u32,
}

#[allow(unused_mut, unused_assignments)]
impl E1000 {
    pub fn new(device: pci::Device) -> Result<E1000, ()> {
        let mut e1000: E1000 = E1000 {
            hardware: self::hardware::Hardware::new(device),
            mng_vlan_id: 0,
            phy_info: self::hardware::PhyInfo::defaults(),
            en_mng_pt: false,
            rx_buffer_len: 0,
            num_tx_queues: 0,
            num_rx_queues: 0,
        };

        e1000.probe()?;
        Ok(e1000)
    }

    fn sw_init(&mut self) -> Result<(), ()> {
        self.rx_buffer_len = crate::io::drivers::network::MAXIMUM_ETHERNET_VLAN_SIZE;

        self.num_tx_queues = 1;
        self.num_rx_queues = 1;

        //self.alloc_queues(adapter)?;

        /* Explicitly disable IRQ since the NIC can be in any state. */
        //self.irq_disable(adapter);

        // spin_lock_init(&adapter->stats_lock);

        //set_bit(__E1000_DOWN, &adapter->flags);

        Ok(())
    }

    /**
     * e1000_alloc_queues - Allocate memory for all rings
     *
     * We allocate one ring per queue at run-time since we don't know the
     * number of queues at compile-time.
     **/
    fn alloc_queues(&mut self) -> Result<(), ()> {
        /*
        adapter->tx_ring = kcalloc(adapter->num_tx_queues,
                    sizeof(struct e1000_tx_ring), GFP_KERNEL);
        if (!adapter->tx_ring)
            return -ENOMEM;

        adapter->rx_ring = kcalloc(adapter->num_rx_queues,
                    sizeof(struct e1000_rx_ring), GFP_KERNEL);
        if (!adapter->rx_ring) {
            kfree(adapter->tx_ring);
            return -ENOMEM;
        }
        */

        Ok(())
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

    fn release_manageability(&self) -> Result<(), ()> {
        if self.en_mng_pt {
            let mut manc = self.hardware.read(E1000_MANC)?;

            /* re-enable hardware interception of ARP */
            manc |= E1000_MANC_ARP_EN;

            self.hardware.write(E1000_MANC, manc)?;
        }
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

        self.phy_info = self.hardware.populate_phy_info()?;

        self.release_manageability()?;

        Ok(())
    }

    fn probe(&mut self) -> Result<(), ()> {
        self.hardware.init_data()?;

        // There's a whole bunch of stuff Linux does here that I don't yet understand
        /*

        /* there is a workaround being applied below that limits
        * 64-bit DMA addresses to 64-bit hardware.  There are some
        * 32-bit adapters that Tx hang when given 64-bit DMA addresses
        */
        pci_using_dac = 0;
        if ((hw->bus_type == e1000_bus_type_pcix) &&
            !dma_set_mask_and_coherent(&pdev->dev, DMA_BIT_MASK(64))) {
            pci_using_dac = 1;
        } else {
            pr_info("DMA setting?\n");
            err = dma_set_mask_and_coherent(&pdev->dev, DMA_BIT_MASK(32));
            if (err) {
                pr_err("No usable DMA config, aborting\n");
                goto err_dma;
            }
        }

        netdev->netdev_ops = &e1000_netdev_ops;
        e1000_set_ethtool_ops(netdev);
        netdev->watchdog_timeo = 5 * HZ;
        netif_napi_add(netdev, &adapter->napi, e1000_clean, 64);

        strncpy(netdev->name, pci_name(pdev), sizeof(netdev->name) - 1);

        adapter->bd_number = cards_found;
        */

        /* setup the private structure */
        self.sw_init()?;

        self.hardware.reset()?;

        self.hardware.checksum_eeprom()?;
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

        Ok(())
    }
}
