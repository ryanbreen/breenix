use crate::println;

use crate::io::pci;

mod constants;
mod hardware;
//mod params;

use self::constants::*;
use crate::io::drivers::network::vlan::*;

pub struct E1000 {
    pci_device: pci::Device,
    hardware: self::hardware::Hardware,
    mng_vlan_id: u16,
    phy_info: self::hardware::PhyInfo,
    en_mng_pt: bool,
    rx_buffer_len: u32,
    num_tx_queues: u32,
    num_rx_queues: u32,
}

impl E1000 {
    pub fn new(device: pci::Device) -> Result<E1000, ()> {
        let mut e1000: E1000 = E1000 {
            pci_device: device,
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

        //set_bit(__DOWN, &adapter->flags);

        Ok(())
    }

    /**
     * alloc_queues - Allocate memory for all rings
     *
     * We allocate one ring per queue at run-time since we don't know the
     * number of queues at compile-time.
     **/
    fn alloc_queues(&mut self) -> Result<(), ()> {
        /*
        adapter->tx_ring = kcalloc(adapter->num_tx_queues,
                    sizeof(struct tx_ring), GFP_KERNEL);
        if (!adapter->tx_ring)
            return -ENOMEM;

        adapter->rx_ring = kcalloc(adapter->num_rx_queues,
                    sizeof(struct rx_ring), GFP_KERNEL);
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
                MNG_DHCP_COOKIE_STATUS_VLAN_SUPPORT) {
                vlan_rx_add_vid(netdev, htons(ETH_P_8021Q), vid);
                adapter->mng_vlan_id = vid;
            } else {
                adapter->mng_vlan_id = MNG_VLAN_NONE;
            }
            if ((old_vid != (u16)MNG_VLAN_NONE) &&
                (vid != old_vid) &&
                !test_bit(old_vid, adapter->active_vlans))
                vlan_rx_kill_vid(netdev, htons(ETH_P_8021Q),
                            old_vid);
        } else {
            adapter->mng_vlan_id = vid;
        }*/

        Ok(())
    }

    fn release_manageability(&self) -> Result<(), ()> {
        if self.en_mng_pt {
            let mut manc = self.hardware.read(MANC)?;

            /* re-enable hardware interception of ARP */
            manc |= MANC_ARP_EN;

            self.hardware.write(MANC, manc)?;
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
         *   with ERT support assuming ERT set to ERT_2048), or
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
        self.hardware.fc = FlowControlSettings::Default;

        self.hardware.reset()?;

        self.hardware.init()?;

        self.update_mng_vlan()?;

        /* Enable h/w to recognize an 802.1Q VLAN Ethernet packet */
        self.hardware.write(VET, ETHERNET_IEEE_VLAN_TYPE)?;

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
        if ((hw->bus_type == bus_type_pcix) &&
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

        netdev->netdev_ops = &netdev_ops;
        set_ethtool_ops(netdev);
        netdev->watchdog_timeo = 5 * HZ;
        netif_napi_add(netdev, &adapter->napi, clean, 64);

        strncpy(netdev->name, pci_name(pdev), sizeof(netdev->name) - 1);

        adapter->bd_number = cards_found;
        */

        /* setup the private structure */
        self.sw_init()?;

        /*
        netdev->priv_flags |= IFF_SUPP_NOFCS;

        netdev->features |= netdev->hw_features;
        netdev->hw_features |= (NETIF_F_RXCSUM |
                    NETIF_F_RXALL |
                    NETIF_F_RXFCS);

        if (pci_using_dac) {
            pr_info("Using dac\n");
            netdev->features |= NETIF_F_HIGHDMA;
            netdev->vlan_features |= NETIF_F_HIGHDMA;
        }

        netdev->vlan_features |= (NETIF_F_TSO |
                    NETIF_F_HW_CSUM |
                    NETIF_F_SG);

        /* Do not set IFF_UNICAST_FLT for VMWare's 82545EM */
        if (hw->device_id != E1000_DEV_ID_82545EM_COPPER ||
            hw->subsystem_vendor_id != PCI_VENDOR_ID_VMWARE)
            netdev->priv_flags |= IFF_UNICAST_FLT;

        */

        /* MTU range: 46 - 16110 */
        //netdev->min_mtu = ETH_ZLEN - ETH_HLEN;
        //netdev->max_mtu = MAX_JUMBO_FRAME_SIZE - (ETH_HLEN + ETH_FCS_LEN);

        self.en_mng_pt = self.hardware.enable_mng_pass_thru()?;

        self.hardware.checksum_eeprom()?;
        self.hardware.load_mac_addr()?;

        self.hardware.reset()?;

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
