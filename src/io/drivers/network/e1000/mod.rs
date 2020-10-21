use crate::println;

use crate::io::pci;
use crate::io::pci::{DeviceError, DeviceErrorCause};
use crate::io::drivers::network::NetworkDriver;

#[allow(unused_variables)]
#[allow(dead_code)]
mod constants;
mod hardware;

#[allow(unused_variables)]
#[allow(dead_code)]
mod vlan;

use self::constants::*;
use super::constants::*;
use super::NetworkDeviceData;

/* Error cause */
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::io::drivers::network::e1000) enum ErrorCause {
    MacType,
    MediaType,
    Phy,
    PhyType,
    Register,
    Config,
    EEPROM,
    DMA,
    SoftwareInit,
    MDIORemap,
    IORemap,
    AllocNetdev,
    PCIReg,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::io::drivers::network::e1000) struct DriverError {
    pub(in crate::io::drivers::network::e1000) cause: ErrorCause,
}

impl From<DriverError> for DeviceError {
    fn from(_error: DriverError) -> Self {
        DeviceError { cause: DeviceErrorCause::InitializationFailure }
    }
}

#[allow(dead_code)]
pub(in crate::io) struct E1000 {
    //pci_device: pci::Device,
    hardware: self::hardware::Hardware,
    mng_vlan_id: u16,
    phy_info: self::hardware::PhyInfo,
    en_mng_pt: bool,
    rx_buffer_len: u32,
    num_tx_queues: u32,
    num_rx_queues: u32,
    wol: u32,
    eeprom_wol: u32,
    device_data: NetworkDeviceData,
}

impl E1000 {
    pub fn new(device: &pci::Device) -> E1000 {
        E1000 {
            //pci_device: device,
            hardware: self::hardware::Hardware::new(device),
            device_data: NetworkDeviceData::defaults(),
            mng_vlan_id: 0,
            phy_info: self::hardware::PhyInfo::defaults(),
            en_mng_pt: false,
            rx_buffer_len: 0,
            num_tx_queues: 0,
            num_rx_queues: 0,
            wol: 0,
            eeprom_wol: 0,
        }
    }

    fn sw_init(&mut self) -> Result<(), DriverError> {
        self.rx_buffer_len = crate::io::drivers::network::MAXIMUM_ETHERNET_VLAN_SIZE;

        self.num_tx_queues = 1;
        self.num_rx_queues = 1;

        self.alloc_queues()?;

        /* Explicitly disable IRQ since the NIC can be in any state. */
        self.irq_disable()?;

        // FIXME: sync
        // spin_lock_init(&adapter->stats_lock);

        // FIXME: NET DEVICE SETUP
        //set_bit(__DOWN, &adapter->flags);

        Ok(())
    }

    /**
     * e1000_irq_disable - Mask off interrupt generation on the NIC
     **/
    fn irq_disable(&self) -> Result<(), DriverError> {
        self.hardware.write(IMC, !0)?;
        self.hardware.write_flush()?;
        
        // FIXME: NET DEVICE SETUP
        // synchronize_irq(adapter->pdev->irq);

        Ok(())
    }

    /**
     * e1000_irq_enable - Enable default interrupt generation settings
     **/
    fn irq_enable(&self) -> Result<(), DriverError> {
        self.hardware.write(IMS, IMS_ENABLE_MASK)?;
        self.hardware.write_flush()?;
        Ok(())
    }

    /**
     * alloc_queues - Allocate memory for all rings
     *
     * We allocate one ring per queue at run-time since we don't know the
     * number of queues at compile-time.
     **/
    fn alloc_queues(&mut self) -> Result<(), DriverError> {
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

    fn release_manageability(&self) -> Result<(), DriverError> {
        if self.en_mng_pt {
            let mut manc = self.hardware.read(MANC)?;

            /* re-enable hardware interception of ARP */
            manc |= MANC_ARP_EN;

            self.hardware.write(MANC, manc)?;
        }
        Ok(())
    }

    fn reset(&mut self) -> Result<(), DriverError> {
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

        vlan::update_mng_vlan(self)?;

        /* Enable h/w to recognize an 802.1Q VLAN Ethernet packet */
        self.hardware.write(VET, ETHERNET_IEEE_VLAN_TYPE)?;

        self.hardware.reset_adaptive()?;

        self.phy_info = self.hardware.populate_phy_info()?;

        self.release_manageability()?;

        Ok(())
    }

    /**
     * e1000_configure - configure the hardware for RX and TX
     **/
    fn configure(&mut self) -> Result<(), DriverError> {
        /*
    	struct net_device *netdev = adapter->netdev;
	    int i;

        e1000_set_rx_mode(netdev);

        e1000_restore_vlan(adapter);
        e1000_init_manageability(adapter);

        e1000_configure_tx(adapter);
        e1000_setup_rctl(adapter);
        e1000_configure_rx(adapter);

        /* call E1000_DESC_UNUSED which always leaves
        * at least 1 descriptor unused to make sure
        * next_to_use != next_to_clean
        */
        for (i = 0; i < adapter->num_rx_queues; i++) {
            struct e1000_rx_ring *ring = &adapter->rx_ring[i];
            adapter->alloc_rx_buf(adapter, ring,
                        E1000_DESC_UNUSED(ring));
        }
        */
        Ok(())
    }

    /**
     * e1000_open - Called when a network interface is made active
     *
     * Returns 0 on success, negative value on failure
     *
     * The open entry point is called when a network interface is made
     * active by the system (IFF_UP).  At this point all resources needed
     * for transmit and receive operations are allocated, the interrupt
     * handler is registered with the OS, the watchdog task is started,
     * and the stack is notified that the interface is ready.
     **/
    fn open(&mut self) -> Result<(), DriverError> {

        /*
    	netif_carrier_off(netdev);

    	/* allocate transmit descriptors */
	    err = e1000_setup_all_tx_resources(adapter);
	    if (err)
		    goto err_setup_tx;

    	/* allocate receive descriptors */
	    err = e1000_setup_all_rx_resources(adapter);
	    if (err)
		    goto err_setup_rx;

    	e1000_power_up_phy(adapter);

    	self.mng_vlan_id = MNG_VLAN_NONE;
	    if self.hardware.mng_cookie.status & MNG_DHCP_COOKIE_STATUS_VLAN_SUPPORT != 0 {
		    vlan::update_mng_vlan(self);
	    }

    	/* before we allocate an interrupt, we must be ready to handle it.
     	 * Setting DEBUG_SHIRQ in the kernel makes it fire an interrupt
	     * as soon as we call pci_request_irq, so we have to setup our
	     * clean_rx handler before we do so.
	     */
    	self.configure(adapter)?;

    	err = e1000_request_irq(adapter);
	    if (err)
		    goto err_req_irq;

    	/* From here on the code is the same as e1000_up() */
	    clear_bit(__E1000_DOWN, &adapter->flags);

    	napi_enable(&adapter->napi);

    	e1000_irq_enable(adapter);

    	netif_start_queue(netdev);

    	/* fire a link status change interrupt to start the watchdog */
	    ew32(ICS, E1000_ICS_LSC);

    	return E1000_SUCCESS;

        /*
    err_req_irq:
	    e1000_power_down_phy(adapter);
	    e1000_free_all_rx_resources(adapter);
    err_setup_rx:
	    e1000_free_all_tx_resources(adapter);
    err_setup_tx:
        e1000_reset(adapter);
        */
        */

    	return Ok(())
    }
}

impl NetworkDriver for E1000 {

    fn probe(&mut self) -> Result<(), DeviceError> {

        // FIXME: NET DEVICE SETUP

        /* do not allocate ioport bars when not needed *
        need_ioport = e1000_is_need_ioport(pdev);
        if (need_ioport) {
            bars = pci_select_bars(pdev, IORESOURCE_MEM | IORESOURCE_IO);
            err = pci_enable_device(pdev);
            pr_info("bars? %x\n", bars);
        } else {
            bars = pci_select_bars(pdev, IORESOURCE_MEM);
            err = pci_enable_device_mem(pdev);
        }
        if (err)
            return err;

        pci_set_master(pdev);
        err = pci_save_state(pdev);
        if (err)
            goto err_alloc_etherdev;

        err = -ENOMEM;
        netdev = alloc_etherdev(sizeof(struct e1000_adapter));
        if (!netdev)
            goto err_alloc_etherdev;

        SET_NETDEV_DEV(netdev, &pdev->dev);

        pci_set_drvdata(pdev, netdev);
        adapter = netdev_priv(netdev);
        adapter->netdev = netdev;
        adapter->pdev = pdev;
        adapter->msg_enable = netif_msg_init(debug, DEFAULT_MSG_ENABLE);
        adapter->bars = bars;
        pr_info("bars? %x\n", bars);
        adapter->need_ioport = need_ioport;
        pr_info("Need ioport? %d\n", need_ioport);

        hw = &adapter->hw;
        hw->back = adapter;

        err = -EIO;
        hw->hw_addr = pci_ioremap_bar(pdev, BAR_0);
        pr_info("hw_addr is %pa\n", &(hw->hw_addr));
        */    

        // FIXME: Can't run this until hardware is setup, but I'm not doing this until init_data.
        // let _ = self.hardware.read(EECD)?;

        self.hardware.init_data()?;

        // There's a whole bunch of stuff Linux does here that I don't yet understand
        /*

        // FIXME: NET DEVICE SETUP
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

        // FIXME: NET DEVICE SETUP
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
        self.device_data.min_mtu = ETH_ZLEN - ETH_HLEN;
        self.device_data.max_mtu = MAX_JUMBO_FRAME_SIZE - (ETH_HLEN + ETH_FCS_LEN);

        self.en_mng_pt = self.hardware.enable_mng_pass_thru()?;

        /* initialize eeprom parameters */
        self.hardware.init_eeprom()?;

        let _ = self.hardware.read(EECD)?;

        self.hardware.reset()?;

        let _ = self.hardware.read(EECD)?;

        /* make sure the EEPROM is good */
        self.hardware.checksum_eeprom()?;
        self.hardware.load_mac_addr()?;

        println!("MAC is {}", self.hardware.mac);

        let _ = self.hardware.read(EECD)?;

        // FIXME: NET DEVICE SETUP
        /* don't block initialization here due to bad MAC address */
        // memcpy(netdev->dev_addr, hw->mac_addr, netdev->addr_len);

        // FIXME: NET DEVICE SETUP
        /*
        if (!is_valid_ether_addr(netdev->dev_addr)) {
            e_err(probe, "Invalid MAC Address\n");
        }
        */

        /*
        INIT_DELAYED_WORK(&adapter->watchdog_task, e1000_watchdog);
        INIT_DELAYED_WORK(&adapter->fifo_stall_task,
                e1000_82547_tx_fifo_stall_task);
        INIT_DELAYED_WORK(&adapter->phy_info_task, e1000_update_phy_info_task);
        INIT_WORK(&adapter->reset_task, e1000_reset_task);
        */

        /*
         * Initial Wake on LAN setting
         * If APM wake is enabled in the EEPROM,
         * enable the ACPI Magic Packet filter
         */

        let mut eeprom_apme_mask: u16 = EEPROM_APME;
        let mut eeprom_data: u16 = 0;

        match self.hardware.mac_type {
            MacType::E100082542Rev2Point0 | MacType::E100082542Rev2Point1 | MacType::E100082543 => {
            }
            MacType::E100082544 => {
                eeprom_data = self.hardware.read_eeprom(EEPROM_INIT_CONTROL2_REG, 1)?;
                eeprom_apme_mask = EEPROM_82544_APM;
            }
            MacType::E100082546 | MacType::E100082546Rev3 => {
                if self.hardware.read(STATUS)? & STATUS_FUNC_1 != 0 {
                    eeprom_data = self.hardware.read_eeprom(EEPROM_INIT_CONTROL3_PORT_B, 1)?;
                } else {
                    eeprom_data = self.hardware.read_eeprom(EEPROM_INIT_CONTROL3_PORT_A, 1)?;
                }
            }
            _ => {
                eeprom_data = self.hardware.read_eeprom(EEPROM_INIT_CONTROL3_PORT_A, 1)?;
            }
        };

        if eeprom_data & eeprom_apme_mask != 0 {
            self.eeprom_wol |= WUFC_MAG;
        }

        self.wol = self.eeprom_wol;
        println!("set wol to {:x}", self.wol);

        // FIXME: NET DEVICE SETUP
        //device_set_wakeup_enable(&adapter->pdev->dev, adapter->wol);

        /* Auto detect PHY address */
        if self.hardware.mac_type == MacType::E1000CE4100 {
            let mut i: u32 = 0;
            for _ in 0..32 {
                i += 1;
                self.hardware.phy_addr = i;
                let tmp = self.hardware.read_phy_reg(PHY_ID2)?;

                if tmp != 0 && tmp != 0xFF {
                    break;
                }
            }

            if i >= 32 {
                return Err(
                    DeviceError::from(
                        DriverError { cause: ErrorCause::EEPROM, }
                    )
                );
            }
        }

        /* reset the hardware with the new settings */
        self.reset()?;

        // FIXME: NET DEVICE SETUP
        /*
        strcpy(netdev->name, "eth%d");
        err = register_netdev(netdev);
        if (err)
            goto err_register;
            */

        vlan::toggle_vlan_filter(self, false)?;

        /* print bus type/speed/width info */
	    println!("(PCI{}:{}MHz:{}-bit)",
            match self.hardware.bus_type == BusType::PCIX { true => "-X", false => "" },
            self.hardware.bus_speed as u32,
            self.hardware.bus_width as u32
        );

        // FIXME: NET DEVICE SETUP
        /* carrier off reporting is important to ethtool even BEFORE open */
        // netif_carrier_off(netdev);

        println!("Intel(R) PRO/1000 Network Connection");

        let _ = self.hardware.read(EECD)?;

        Ok(())
    }
}
