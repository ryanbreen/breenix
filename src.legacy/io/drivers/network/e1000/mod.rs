use crate::println;

use crate::io::drivers::network::{NetworkDriver, NetworkFlags};
use crate::io::pci;
use crate::io::pci::{Device, DeviceError, DeviceErrorCause};

#[allow(unused_variables)]
#[allow(dead_code)]
mod constants;
mod hardware;

#[allow(unused_variables)]
#[allow(dead_code)]
mod vlan;

use alloc::boxed::Box;
use core::marker::PhantomPinned;
use core::mem::size_of;
use core::pin::Pin;

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
        DeviceError {
            cause: DeviceErrorCause::InitializationFailure,
        }
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
struct TransmitDescriptorFlags {
    length: u16,	/* Data buffer length */
    cso: u8,	/* Checksum offset */
    cmd: u8,	/* Descriptor control */
}

#[repr(C)]
#[derive(Clone, Copy)]
union TransmitDescriptorLower {
    data: u32,
    flags: TransmitDescriptorFlags,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
struct TransmitDescriptorFields {
    status: u8,	/* Descriptor status */
    css: u8,	/* Checksum start */
    special: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
union TransmitDescriptorUpper {
    data: u32,
    fields: TransmitDescriptorFields,
}

/* Transmit Descriptor */
#[repr(C)]
#[derive(Copy, Clone)]
struct TransmitDescriptor {
	buffer_addr: u64,	/* Address of the descriptor's data buffer */
	lower: TransmitDescriptorLower,
    upper : TransmitDescriptorUpper,
    _pin: PhantomPinned,
}

/* wrapper around a pointer to a socket buffer,
 * so a DMA handle can be stored along with the buffer
 *
#[repr(C)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct TransmitBuffer {
	// struct sk_buff *skb;
	dma: u64,
	time_stamp: u64,
	length: u16,
	next_to_watch: u16,
	mapped_as_page, bool,
	segs: u8,
	bytecount: u32,
}

/* Transmit Ring */
#[repr(C)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct TransmitRing {
	/* pointer to the descriptor ring memory */
	void *desc,
	/* physical address of the descriptor ring */
	dma: u64,
	/* length of descriptor ring in bytes */
	size: u32,
	/* number of descriptors in the ring */
	count: u32,
	/* next descriptor to associate a buffer with */
	next_to_use: u32,
	/* next descriptor to check for DD status bit */
	next_to_clean: u32,
	/* array of buffer information structs */
	struct e1000_tx_buffer *buffer_info,

	tdh: u16,
	tdt: u16,
	last_tx_tso: bool,
}
*/

#[allow(dead_code)]
pub(in crate::io) struct E1000 {
    pci_device: Device,
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
    tx_queue_0_ptr: *const [TransmitDescriptor; 0x1000], /* We have 4096 tx records per queue */
    rx_queue_0_ptr: *const [TransmitDescriptor; 0x1000], // FIXME: THIS IS FAKE /* We have 4096 tx records per queue */
    txd_cmd: u32,
	tx_int_delay: u32,
	tx_abs_int_delay: u32,
	rx_int_delay: u32,
	rx_abs_int_delay: u32,
	itr_setting: u32,
	itr: u32,
	rx_csum: u32,
}

impl E1000 {
    pub fn new(device: &pci::Device) -> Box<E1000> {

        let txd_archetype = TransmitDescriptor {
            buffer_addr: 0,
            lower: TransmitDescriptorLower {
                data: 0,
            },
            upper: TransmitDescriptorUpper {
                data: 0,
            },
            _pin: PhantomPinned,
        };

        let tx_queue = Box::pin([txd_archetype; 0x1000]);
        let rx_queue = Box::pin([txd_archetype; 0x1000]); // FIXME: THIS IS FAKE

        let dev = Box::new(E1000 {
            pci_device: *device,
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
            tx_queue_0_ptr: &*tx_queue as *const [TransmitDescriptor; 4096],
            rx_queue_0_ptr: &*rx_queue as *const [TransmitDescriptor; 4096], // FIXME: THIS IS FAKE
            txd_cmd: 0,
        	tx_int_delay: DEFAULT_TIDV,
	        tx_abs_int_delay: DEFAULT_TADV,
        	rx_int_delay: DEFAULT_RDTR,
	        rx_abs_int_delay: DEFAULT_RADV,
	        itr: DEFAULT_ITR,
	        itr_setting: DEFAULT_ITR,
	        rx_csum: DEFAULT_RXCSUM,
        });

        dev
    }

    fn sw_init(&mut self) -> Result<(), DriverError> {
        self.rx_buffer_len = MAXIMUM_ETHERNET_VLAN_SIZE;

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
        println!("Enabling IRQ with {:x} sent to {:x}", IMS_ENABLE_MASK, IMS);
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
        self.tx_ring = kcalloc(adapter->num_tx_queues,
                    sizeof(struct tx_ring), GFP_KERNEL);
        if (!adapter->tx_ring)
            return -ENOMEM;

        */
        /*
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

        println!("before hardware reset\n{:?}", self.hardware);

        self.hardware.reset()?;

        println!("mancystank {:?}", self.hardware);
        panic!();

        self.hardware.init()?;

        vlan::update_mng_vlan(self)?;

        /* Enable h/w to recognize an 802.1Q VLAN Ethernet packet */
        self.hardware.write(VET, ETHERNET_IEEE_VLAN_TYPE)?;

        self.hardware.reset_adaptive()?;

        self.phy_info = self.hardware.populate_phy_info()?;

        self.release_manageability()?;

        Ok(())
    }

    fn init_manageability(&self) -> Result<(), DriverError> {
        if self.en_mng_pt {
            let mut manc = self.hardware.read(MANC)?;

            /* disable hardware interception of ARP */
            manc &= !MANC_ARP_EN;

            self.hardware.write(MANC, manc)?;
        }

        Ok(())
    }

    /**
     * e1000_set_rx_mode - Secondary Unicast, Multicast and Promiscuous mode set
     * @netdev: network interface device structure
     *
     * The set_rx_mode entry point is called whenever the unicast or multicast
     * address lists or the network interface flags are updated. This routine is
     * responsible for configuring the hardware for proper unicast, multicast,
     * promiscuous mode, and all-multi behavior.
     **/
    fn set_rx_mode(&self) -> Result<(), DriverError> {
        /*
        struct e1000_adapter *adapter = netdev_priv(netdev);
        struct e1000_hw *hw = &adapter->hw;
        struct netdev_hw_addr *ha;
        bool use_uc = false;
        u32 rctl;
        u32 hash_value;
        int i, rar_entries = E1000_RAR_ENTRIES;
        int mta_reg_count = E1000_NUM_MTA_REGISTERS;
        u32 *mcarray = kcalloc(mta_reg_count, sizeof(u32), GFP_ATOMIC);

        if (!mcarray)
            return;
        */

        /* Check for Promiscuous and All Multicast modes */

        let mut rctl = self.hardware.read(RCTL)?;

        if (self.device_data.flags & NetworkFlags::Promiscuous as u32) != 0 {
            rctl |= RCTL_UPE | RCTL_MPE;
            rctl &= !RCTL_VFE;
        } else {
            if (self.device_data.flags & NetworkFlags::AllMulti as u32) != 0 {
                rctl |= RCTL_MPE;
            } else {
                rctl &= !RCTL_MPE;
            }
            /* Enable VLAN filter if there is a VLAN */
            if false {// self.vlan_used() {
                rctl |= RCTL_VFE;
            }
        }

        /*
        if (netdev_uc_count(netdev) > rar_entries - 1) {
            rctl |= E1000_RCTL_UPE;
        } else if (!(netdev->flags & IFF_PROMISC)) {
            */
        rctl &= !RCTL_UPE;
        let use_uc:bool = true;
        //}

        self.hardware.write(RCTL, rctl)?;

        /* 82542 2.0 needs to be in reset to write receive address registers *

        if (hw->mac_type == e1000_82542_rev2_0)
            e1000_enter_82542_rst(adapter);

        /* load the first 14 addresses into the exact filters 1-14. Unicast
        * addresses take precedence to avoid disabling unicast filtering
        * when possible.
        *
        * RAR 0 is used for the station MAC address
        * if there are not 14 addresses, go ahead and clear the filters
        */
        i = 1;
        if (use_uc)
            netdev_for_each_uc_addr(ha, netdev) {
                if (i == rar_entries)
                    break;
                e1000_rar_set(hw, ha->addr, i++);
            }

        netdev_for_each_mc_addr(ha, netdev) {
            if (i == rar_entries) {
                /* load any remaining addresses into the hash table */
                u32 hash_reg, hash_bit, mta;
                hash_value = e1000_hash_mc_addr(hw, ha->addr);
                hash_reg = (hash_value >> 5) & 0x7F;
                hash_bit = hash_value & 0x1F;
                mta = (1 << hash_bit);
                mcarray[hash_reg] |= mta;
            } else {
                e1000_rar_set(hw, ha->addr, i++);
            }
        }

        for (; i < rar_entries; i++) {
            E1000_WRITE_REG_ARRAY(hw, RA, i << 1, 0);
            E1000_WRITE_FLUSH();
            E1000_WRITE_REG_ARRAY(hw, RA, (i << 1) + 1, 0);
            E1000_WRITE_FLUSH();
        }

        /* write the hash table completely, write from bottom to avoid
        * both stupid write combining chipsets, and flushing each write
        */
        for (i = mta_reg_count - 1; i >= 0 ; i--) {
            /* If we are on an 82544 has an errata where writing odd
            * offsets overwrites the previous even offset, but writing
            * backwards over the range solves the issue by always
            * writing the odd offset first
            */
            E1000_WRITE_REG_ARRAY(hw, MTA, i, mcarray[i]);
        }
        E1000_WRITE_FLUSH();

        if (hw->mac_type == e1000_82542_rev2_0)
            e1000_leave_82542_rst(adapter);

        kfree(mcarray);
        */

        Ok(())
    }

    /**
     * e1000_configure_tx - Configure 8254x Transmit Unit after Reset
     *
     * Configure the Tx unit of the MAC after a reset.
     **/
    fn configure_tx(&mut self) -> Result<(), DriverError> {

        /* Setup the HW Tx Head and Tail descriptor pointers */
        println!("configuring TX!");

        /*
        let queue:[TransmitDescriptor;0x1000] = self.tx_queues[0];
        let queue_ptr:* const u64 = (&queue) as *const u64;
        */
        let tdba:u64 = self.tx_queue_0_ptr as u64;
        let tdlen:u32 = (100 * size_of::<TransmitDescriptor>()) as u32;
        self.hardware.write(TDLEN, tdlen)?;
        self.hardware.write(TDBAH, tdba.wrapping_shr(32) as u32)?;
        self.hardware.write(TDBAL, (tdba & 0x00000000ffffffff) as u32)?;
        self.hardware.write(TDT, 0)?;
        self.hardware.write(TDH, 0)?;
        //self.adapter->tx_ring[0].tdh = ((hw->mac_type >= e1000_82543) ?
        //                E1000_TDH : E1000_82542_TDH);
        //    adapter->tx_ring[0].tdt = ((hw->mac_type >= e1000_82543) ?
        //                E1000_TDT : E1000_82542_TDT);

        /* Set the default values for the Tx Inter Packet Gap timer */
        let mut tipg:u32 = match self.hardware.media_type {
            MediaType::Fiber | MediaType::InternalSerdes => DEFAULT_82543_TIPG_IPGT_FIBER,
            _ => DEFAULT_82543_TIPG_IPGT_COPPER,
        };

        let ipgr1:u32;
        let ipgr2:u32;
        match self.hardware.mac_type {
            MacType::E100082542Rev2Point0 | MacType::E100082542Rev2Point1 => {
                tipg = DEFAULT_82542_TIPG_IPGT;
                ipgr1 = DEFAULT_82542_TIPG_IPGR1;
                ipgr2 = DEFAULT_82542_TIPG_IPGR2;
            },
            _ => {
                ipgr1 = DEFAULT_82543_TIPG_IPGR1;
                ipgr2 = DEFAULT_82543_TIPG_IPGR2;
            },
        };

        tipg |= ipgr1 << TIPG_IPGR1_SHIFT;
        tipg |= ipgr2 << TIPG_IPGR2_SHIFT;
        self.hardware.write(TIPG, tipg)?;

        /* Set the Tx Interrupt Delay register */
        self.hardware.write(TIDV, self.tx_int_delay)?;
        if (self.hardware.mac_type as u32) >= (MacType::E100082540 as u32) {
            self.hardware.write(TADV, self.tx_abs_int_delay)?;
        }

        /* Program the Transmit Control Register */
        let mut tctl = self.hardware.read(TCTL)?;
        println!("Read tctl of {:x}", tctl);
        tctl &= !TCTL_CT;
        tctl |= TCTL_PSP | TCTL_RTLC |
            (COLLISION_THRESHOLD << CT_SHIFT);

        self.hardware.config_collision_dist()?;

        /* Setup Transmit Descriptor Settings for eop descriptor */
        self.txd_cmd = TXD_CMD_EOP | TXD_CMD_IFCS;

        /* only set IDE if we are delaying interrupts using the timers */
        if self.tx_int_delay != 0 {
            self.txd_cmd |= TXD_CMD_IDE;
        }

        if (self.hardware.mac_type as u32) < (MacType::E100082543 as u32) {
            self.txd_cmd |= TXD_CMD_RPS;
        } else {
            self.txd_cmd |= TXD_CMD_RS;
        }

        /* Cache if we're 82544 running in PCI-X because we'll
        * need this to apply a workaround later in the send path.
        */
        if self.hardware.mac_type == MacType::E100082544 &&
           self.hardware.bus_type == BusType::PCIX {
            self.hardware.pcix_82544 = true;
        }

        self.hardware.write(TCTL, tctl)?;

        Ok(())
    }

    /**
     * e1000_alloc_rx_buffers - Replace used receive buffers; legacy & extended
     **/
    fn alloc_rx_buf(&self) -> Result<(), DriverError> {
        
        /*
        let bufsz: u32 = self.rx_buffer_len;

        i = rx_ring->next_to_use;
        buffer_info = &rx_ring->buffer_info[i];

        pr_info("Our buffers are not jumbo\n");

        while (cleaned_count--) {
            void *data;

            if (buffer_info->rxbuf.data)
                goto skip;

            data = e1000_alloc_frag(adapter);
            if (!data) {
                /* Better luck next round */
                adapter->alloc_rx_buff_failed++;
                break;
            }

            /* Fix for errata 23, can't cross 64kB boundary */
            if (!e1000_check_64k_bound(adapter, data, bufsz)) {
                void *olddata = data;
                e_err(rx_err, "skb align check failed: %u bytes at "
                    "%p\n", bufsz, data);
                /* Try again, without freeing the previous */
                data = e1000_alloc_frag(adapter);
                /* Failed allocation, critical failure */
                if (!data) {
                    skb_free_frag(olddata);
                    adapter->alloc_rx_buff_failed++;
                    break;
                }

                if (!e1000_check_64k_bound(adapter, data, bufsz)) {
                    /* give up */
                    skb_free_frag(data);
                    skb_free_frag(olddata);
                    adapter->alloc_rx_buff_failed++;
                    break;
                }

                /* Use new allocation */
                skb_free_frag(olddata);
            }
            buffer_info->dma = dma_map_single(&pdev->dev,
                            data,
                            adapter->rx_buffer_len,
                            DMA_FROM_DEVICE);
            if (dma_mapping_error(&pdev->dev, buffer_info->dma)) {
                skb_free_frag(data);
                buffer_info->dma = 0;
                adapter->alloc_rx_buff_failed++;
                break;
            }

            /* XXX if it was allocated cleanly it will never map to a
            * boundary crossing
            */

            /* Fix for errata 23, can't cross 64kB boundary */
            if (!e1000_check_64k_bound(adapter,
                        (void *)(unsigned long)buffer_info->dma,
                        adapter->rx_buffer_len)) {
                e_err(rx_err, "dma align check failed: %u bytes at "
                    "%p\n", adapter->rx_buffer_len,
                    (void *)(unsigned long)buffer_info->dma);

                dma_unmap_single(&pdev->dev, buffer_info->dma,
                        adapter->rx_buffer_len,
                        DMA_FROM_DEVICE);

                skb_free_frag(data);
                buffer_info->rxbuf.data = NULL;
                buffer_info->dma = 0;

                adapter->alloc_rx_buff_failed++;
                break;
            }
            buffer_info->rxbuf.data = data;
    skip:
            rx_desc = E1000_RX_DESC(*rx_ring, i);
            rx_desc->buffer_addr = cpu_to_le64(buffer_info->dma);

            if (unlikely(++i == rx_ring->count))
                i = 0;
            buffer_info = &rx_ring->buffer_info[i];
        }

        if (likely(rx_ring->next_to_use != i)) {
            rx_ring->next_to_use = i;
            if (unlikely(i-- == 0))
                i = (rx_ring->count - 1);

            /* Force memory writes to complete before letting h/w
            * know there are new descriptors to fetch.  (Only
            * applicable for weak-ordered memory model archs,
            * such as IA-64).
            */
            dma_wmb(); */
            //writel(i, hw->hw_addr + rx_ring->rdt);
            println!("Writing fake rx thingy");
            println!("Writing to {} to {:x} at {:x}", 254, 0x2818, self.hardware.hw_addr.addr);
            self.hardware.write(0x2818, 254)?;
            println!("Wrote fake rx thingy");
        //}
        Ok(())
    }

    /**
     * e1000_configure_rx - Configure 8254x Receive Unit after Reset
     *
     * Configure the Rx unit of the MAC after a reset.
     **/
     fn configure_rx(&mut self, ) -> Result<(), DriverError> {

        /*
        u64 rdba;
        u32 rdlen, rctl, rxcsum;
        */

        println!("configuring RX!");

        let rdlen:u32 = (100 * size_of::<TransmitDescriptor>()) as u32; // FIXME: This is fake

        // FIXME: We will need this before we can do real work
        /*
        let rdlen:u32 = 0;
        if self.device_data.mtu > ETH_DATA_LEN {
            rdlen = adapter->rx_ring[0].count *
                sizeof(struct e1000_rx_desc);
            adapter->clean_rx = e1000_clean_jumbo_rx_irq;
        } else {
            rdlen = adapter->rx_ring[0].count *
                sizeof(struct e1000_rx_desc);
            adapter->clean_rx = e1000_clean_rx_irq;
        }
        */

        /* disable receives while setting up the descriptors */
        let rctl = self.hardware.read(RCTL)?;
        self.hardware.write(RCTL, rctl & !RCTL_EN);

        /* set the Receive Delay Timer Register */
        self.hardware.write(RDTR, self.rx_int_delay)?;

        if (self.hardware.mac_type as u32) >= (MacType::E100082540 as u32) {
            self.hardware.write(RADV, self.rx_abs_int_delay)?;
            if self.itr_setting != 0 {
                self.hardware.write(ITR, 1000000000 / (self.itr * 256))?;
            }
        }

        /* Setup the HW Rx Head and Tail Descriptor Pointers and
        * the Base and Length of the Rx Descriptor Ring
        */
        let rdba:u64 = self.rx_queue_0_ptr as u64; // adapter->rx_ring[0].dma;
        self.hardware.write(RDLEN, rdlen)?;
        self.hardware.write(RDBAH, rdba.wrapping_shr(32) as u32)?;
        self.hardware.write(RDBAL, rdba as u32 & 0x00000000ffffffff);
        self.hardware.write(RDT, 0);
        self.hardware.write(RDH, 0);
            
        /*
        adapter->rx_ring[0].rdh = ((hw->mac_type >= e1000_82543) ?
                        E1000_RDH : E1000_82542_RDH);
            adapter->rx_ring[0].rdt = ((hw->mac_type >= e1000_82543) ?
                        E1000_RDT : E1000_82542_RDT);
        */

        /* Enable 82543 Receive Checksum Offload for TCP and UDP */
        if (self.hardware.mac_type as u32) >= (MacType::E100082543 as u32) {
            let mut rxcsum = self.hardware.read(RXCSUM)?;
            println!("register rxcusm is {:x}", rxcsum);
            if self.rx_csum != 0 {
                println!("adapter rx_csum is not 0");
                rxcsum |= RXCSUM_TUOFL;
            } else {
                /* don't need to clear IPPCSE as it defaults to 0 */
                rxcsum &= !RXCSUM_TUOFL;
            }
            self.hardware.write(RXCSUM, rxcsum)?;
        }

        /* Enable Receives */
        self.hardware.write(RCTL, rctl | RCTL_EN)?;

        Ok(())
    }

    /**
     * e1000_setup_rctl - configure the receive control registers
     **/
    fn setup_rctl(& self) -> Result<(), DriverError> {
        
        let mut rctl = self.hardware.read(RCTL)?;

        rctl &= !(3 << RCTL_MO_SHIFT);

        rctl |= RCTL_BAM | RCTL_LBM_NO |
            RCTL_RDMTS_HALF |
            (self.hardware.mc_filter_type << RCTL_MO_SHIFT);

        if self.hardware.tbi_compatibility_on {
            rctl |= RCTL_SBP;
        } else {
            rctl &= !RCTL_SBP;
        }

        if self.device_data.mtu <= ETH_DATA_LEN {
            rctl &= !RCTL_LPE;
        } else {
            rctl |= RCTL_LPE;
        }

        /* Setup buffer sizes */
        rctl &= !RCTL_SZ_4096;
        rctl |= RCTL_BSEX;
        match self.rx_buffer_len {
            _ => {
                rctl |= RCTL_SZ_2048;
                rctl &= !RCTL_BSEX;
            }
        };

        // FIXME: NETDEV
        /* This is useful for sniffing bad packets. *
        if adapter->netdev->features & NETIF_F_RXALL) {
            /* UPE and MPE will be handled by normal PROMISC logic
            * in e1000e_set_rx_mode
            */
            rctl |= (E1000_RCTL_SBP | /* Receive bad packets */
                E1000_RCTL_BAM | /* RX All Bcast Pkts */
                E1000_RCTL_PMCF); /* RX All MAC Ctrl Pkts */

            rctl &= ~(E1000_RCTL_VFE | /* Disable VLAN filter */
                E1000_RCTL_DPF | /* Allow filtered pause */
                E1000_RCTL_CFIEN); /* Dis VLAN CFIEN Filter */
            /* Do not mess with E1000_CTRL_VME, it affects transmit as well,
            * and that breaks VLANs.
            */
        }
        */

        self.hardware.write(RCTL, rctl)?;
        Ok(())
    }

    /**
     * e1000_configure - configure the hardware for RX and TX
     **/
    fn configure(&mut self) -> Result<(), DriverError> {

        self.set_rx_mode()?;

        /*
        e1000_restore_vlan(adapter);
        e1000_init_manageability(adapter);
        */

        self.configure_tx()?;
        self.setup_rctl()?;
        self.configure_rx()?;

        /* call E1000_DESC_UNUSED which always leaves
        * at least 1 descriptor unused to make sure
        * next_to_use != next_to_clean
        */
        //for i = 0; i < adapter->num_rx_queues; i++) {
        //    struct e1000_rx_ring *ring = &adapter->rx_ring[i];
        self.alloc_rx_buf()?; // ring, E1000_DESC_UNUSED(ring));
        //}

        Ok(())
    }

    /**
     * e1000_power_up_phy - restore link in case the phy was powered down
     * @adapter: address of board private structure
     *
     * The phy may be powered down to save power and turn off link when the
     * driver is unloaded and wake on lan is not enabled (among others)
     * *** this routine MUST be followed by a call to e1000_reset ***
     **/
    fn power_up_phy(&self) -> Result<(), DriverError> {
        /* Just clear the power down bit to wake the phy back up */
        if self.hardware.media_type == MediaType::Copper {
            /* according to the manual, the phy will retain its
             * settings across a power-down/up cycle
             */
            let mut mii_reg = self.hardware.read_phy_reg(PHY_CTRL)?;
            mii_reg &= !MII_CR_POWER_DOWN;
            self.hardware.write_phy_reg(PHY_CTRL, mii_reg)?;
        }

        Ok(())
    }

    fn request_irq(&self) -> Result<(), DriverError> {
        println!(
            "This is where we should set up a handler for {}",
            self.pci_device.irq
        );
        /*
        struct net_device *netdev = adapter->netdev;
        irq_handler_t handler = e1000_intr;
        int irq_flags = IRQF_SHARED;
        int err;

        err = request_irq(adapter->pdev->irq, handler, irq_flags, netdev->name,
                netdev);
        if (err) {
            e_err(probe, "Unable to allocate interrupt Error: %d\n", err);
        }

        return err;
        */

        Ok(())
    }

    /**
     * e1000_watchdog - work function
     * @work: work struct contained inside adapter struct
     **/
    fn watchdog(&self) -> Result<(), DriverError> {

        /*
        struct e1000_adapter *adapter = container_of(work,
                                struct e1000_adapter,
                                watchdog_task.work);
        struct e1000_hw *hw = &adapter->hw;
        struct net_device *netdev = adapter->netdev;
        struct e1000_tx_ring *txdr = adapter->tx_ring;
        u32 link, tctl;

        pr_info("Inside watchdog\n");

        link = e1000_has_link(adapter);
        if ((netif_carrier_ok(netdev)) && link)
            goto link_up;

        if (link) {
            if (!netif_carrier_ok(netdev)) {
                u32 ctrl;
                /* update snapshot of PHY registers on LSC */
                e1000_get_speed_and_duplex(hw,
                            &adapter->link_speed,
                            &adapter->link_duplex);

                ctrl = er32(CTRL);
                pr_info("%s NIC Link is Up %d Mbps %s, "
                    "Flow Control: %s\n",
                    netdev->name,
                    adapter->link_speed,
                    adapter->link_duplex == FULL_DUPLEX ?
                    "Full Duplex" : "Half Duplex",
                    ((ctrl & E1000_CTRL_TFCE) && (ctrl &
                    E1000_CTRL_RFCE)) ? "RX/TX" : ((ctrl &
                    E1000_CTRL_RFCE) ? "RX" : ((ctrl &
                    E1000_CTRL_TFCE) ? "TX" : "None")));

                /* adjust timeout factor according to speed/duplex */
                adapter->tx_timeout_factor = 1;
                switch (adapter->link_speed) {
                case SPEED_10:
                    adapter->tx_timeout_factor = 16;
                    break;
                case SPEED_100:
                    /* maybe add some timeout factor ? */
                    break;
                }

                /* enable transmits in the hardware */
                tctl = er32(TCTL);
                tctl |= E1000_TCTL_EN;
                ew32(TCTL, tctl);

                netif_carrier_on(netdev);
                if (!test_bit(__E1000_DOWN, &adapter->flags))
                    schedule_delayed_work(&adapter->phy_info_task,
                                2 * HZ);
                adapter->smartspeed = 0;
            }
        } else {
            if (netif_carrier_ok(netdev)) {
                adapter->link_speed = 0;
                adapter->link_duplex = 0;
                pr_info("%s NIC Link is Down\n",
                    netdev->name);
                netif_carrier_off(netdev);

                if (!test_bit(__E1000_DOWN, &adapter->flags))
                    schedule_delayed_work(&adapter->phy_info_task,
                                2 * HZ);
            }

            e1000_smartspeed(adapter);
        }

        //link_up:
        e1000_update_stats(adapter);

        hw->tx_packet_delta = adapter->stats.tpt - adapter->tpt_old;
        adapter->tpt_old = adapter->stats.tpt;
        hw->collision_delta = adapter->stats.colc - adapter->colc_old;
        adapter->colc_old = adapter->stats.colc;

        adapter->gorcl = adapter->stats.gorcl - adapter->gorcl_old;
        adapter->gorcl_old = adapter->stats.gorcl;
        adapter->gotcl = adapter->stats.gotcl - adapter->gotcl_old;
        adapter->gotcl_old = adapter->stats.gotcl;

        e1000_update_adaptive(hw);

        if (!netif_carrier_ok(netdev)) {
            if (E1000_DESC_UNUSED(txdr) + 1 < txdr->count) {
                /* We've lost link, so the controller stops DMA,
                * but we've got queued Tx work that's never going
                * to get done, so reset controller to flush Tx.
                * (Do the reset outside of interrupt context).
                */
                adapter->tx_timeout_count++;
                schedule_work(&adapter->reset_task);
                /* exit immediately since reset is imminent */
                return;
            }
        }

        /* Simple mode for Interrupt Throttle Rate (ITR) */
        if (hw->mac_type >= e1000_82540 && adapter->itr_setting == 4) {
            /* Symmetric Tx/Rx gets a reduced ITR=2000;
            * Total asymmetrical Tx or Rx gets ITR=8000;
            * everyone else is between 2000-8000.
            */
            u32 goc = (adapter->gotcl + adapter->gorcl) / 10000;
            u32 dif = (adapter->gotcl > adapter->gorcl ?
                    adapter->gotcl - adapter->gorcl :
                    adapter->gorcl - adapter->gotcl) / 10000;
            u32 itr = goc > 0 ? (dif * 6000 / goc + 2000) : 8000;

            ew32(ITR, 1000000000 / (itr * 256));
        }

        /* Cause software interrupt to ensure rx ring is cleaned */
        ew32(ICS, E1000_ICS_RXDMT0);

        /* Force detection of hung controller every watchdog period */
        adapter->detect_tx_hung = true;

        /* Reschedule the task */
        if (!test_bit(__E1000_DOWN, &adapter->flags))
            schedule_delayed_work(&adapter->watchdog_task, 2 * HZ);
        */
        Ok(())
    }
}

impl NetworkDriver for E1000 {
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
    fn open(&mut self) -> Result<(), DeviceError> {
        // On Linux, which of course does much more, the below equates to:
        //     netif_carrier_off(netdev);
        self.device_data.carrier_down_count += 1;
        self.device_data.carrier_online = false;
        /*

        /* allocate transmit descriptors */
        err = e1000_setup_all_tx_resources(adapter);
        if (err)
            goto err_setup_tx;

        /* allocate receive descriptors */
        err = e1000_setup_all_rx_resources(adapter);
        if (err)
            goto err_setup_rx;
        */

        // self.hardware.coming_up = true;

        self.power_up_phy()?;

        self.mng_vlan_id = MNG_VLAN_NONE;
        if self.hardware.mng_cookie.status & MNG_DHCP_COOKIE_STATUS_VLAN_SUPPORT != 0 {
            vlan::update_mng_vlan(self)?;
        }

        /* before we allocate an interrupt, we must be ready to handle it.
         * Setting DEBUG_SHIRQ in the kernel makes it fire an interrupt
         * as soon as we call pci_request_irq, so we have to setup our
         * clean_rx handler before we do so.
         */
        self.configure()?;

        println!("{:?}", self.hardware);

        self.request_irq()?;
        //if (err)
        //    goto err_req_irq;

        /*


        /* From here on the code is the same as e1000_up() */
        clear_bit(__E1000_DOWN, &adapter->flags);

        napi_enable(&adapter->napi);

        */

        self.irq_enable()?;

        /*
            netif_start_queue(netdev);
            */

        /* fire a link status change interrupt to start the watchdog */
        self.hardware.write(ICS, ICS_LSC)?;

            /*
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

        return Ok(());
    }

    fn probe(&mut self) -> Result<(), DeviceError> {
        // FIXME: NET DEVICE SETUP
        let mut bars = 0;

        /* do not allocate ioport bars when not needed */
        if self.hardware.need_ioport() {
            println!("NEED IOPORT!");
            // bars = self.pci_device.select_bars(IORESOURCE_MEM | IORESOURCE_IO);
            bars = self.pci_device.select_bars(0x200 | 0x100);
            println!("Bars selected are {:x}", bars);
            /*
            bars = pci_select_bars(pdev, IORESOURCE_MEM | IORESOURCE_IO);
            err = pci_enable_device(pdev);
            pr_info("bars? %x\n", bars);
            */
        }

        self.hardware.io_base.addr = 0xc080;
        println!("IO base is {:x}", self.hardware.io_base.addr);

        self.pci_device.set_master()?;
        self.pci_device.save_state()?;
        /*
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

        // FIXME RUN AS TASKS
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
                return Err(DeviceError::from(DriverError {
                    cause: ErrorCause::EEPROM,
                }));
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
        println!(
            "(PCI{}:{}MHz:{}-bit)",
            match self.hardware.bus_type == BusType::PCIX {
                true => "-X",
                false => "",
            },
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
