//! Intel 82540EM (e1000) Gigabit Ethernet Driver
//!
//! This driver supports the Intel 82540EM NIC which is the default network
//! card emulated by QEMU. It implements basic packet transmission and reception
//! using memory-mapped I/O and descriptor rings.
//!
//! # References
//! - Intel PCI/PCI-X Family of Gigabit Ethernet Controllers Software Developer's Manual
//! - OSDev Wiki: https://wiki.osdev.org/Intel_8254x

mod regs;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

use crate::drivers::pci::{self, Device, INTEL_VENDOR_ID};
use crate::memory::PhysAddrWrapper as PhysAddr;

pub use regs::*;

/// Intel 82540EM device ID
#[allow(dead_code)] // Used in init() for device detection
pub const E1000_DEVICE_ID: u16 = 0x100E;

/// Number of receive descriptors (must be multiple of 8)
const RX_RING_SIZE: usize = 32;
/// Number of transmit descriptors (must be multiple of 8)
const TX_RING_SIZE: usize = 32;
/// Size of each receive buffer
const RX_BUFFER_SIZE: usize = 2048;

/// Maximum Ethernet frame size (including header and CRC)
#[allow(dead_code)] // Public API for network stack
pub const ETH_FRAME_MAX: usize = 1518;

/// Receive descriptor (16 bytes)
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug)]
pub struct RxDesc {
    /// Physical address of the receive buffer
    pub addr: u64,
    /// Length of received data
    pub length: u16,
    /// Checksum
    pub checksum: u16,
    /// Status bits
    pub status: u8,
    /// Error bits
    pub errors: u8,
    /// Special/VLAN tag
    pub special: u16,
}

impl RxDesc {
    const fn new() -> Self {
        RxDesc {
            addr: 0,
            length: 0,
            checksum: 0,
            status: 0,
            errors: 0,
            special: 0,
        }
    }

    /// Check if this descriptor has been filled by hardware
    pub fn is_done(&self) -> bool {
        self.status & RXD_STAT_DD != 0
    }

    /// Check if this is the end of a packet
    #[allow(dead_code)] // Part of RX descriptor API for multi-packet handling
    pub fn is_eop(&self) -> bool {
        self.status & RXD_STAT_EOP != 0
    }
}

/// Transmit descriptor (16 bytes)
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug)]
pub struct TxDesc {
    /// Physical address of the transmit buffer
    pub addr: u64,
    /// Length of data to transmit
    pub length: u16,
    /// Checksum offset
    pub cso: u8,
    /// Command bits
    pub cmd: u8,
    /// Status bits
    pub status: u8,
    /// Checksum start
    pub css: u8,
    /// Special/VLAN tag
    pub special: u16,
}

impl TxDesc {
    const fn new() -> Self {
        TxDesc {
            addr: 0,
            length: 0,
            cso: 0,
            cmd: 0,
            status: 0,
            css: 0,
            special: 0,
        }
    }

    /// Check if this descriptor has been processed by hardware
    pub fn is_done(&self) -> bool {
        self.status & TXD_STAT_DD != 0
    }
}

/// E1000 driver state
pub struct E1000 {
    /// PCI device information
    #[allow(dead_code)] // Stored for future use (interrupt routing, power management)
    pci_device: Device,
    /// Base address of MMIO registers
    mmio_base: usize,
    /// Receive descriptor ring
    rx_ring: Box<[RxDesc; RX_RING_SIZE]>,
    /// Receive buffers
    rx_buffers: Vec<Box<[u8; RX_BUFFER_SIZE]>>,
    /// Transmit descriptor ring
    tx_ring: Box<[TxDesc; TX_RING_SIZE]>,
    /// Current receive descriptor index
    rx_cur: usize,
    /// Current transmit descriptor index
    tx_cur: usize,
    /// MAC address
    mac_addr: [u8; 6],
}

impl E1000 {
    /// Read a 32-bit register
    fn read_reg(&self, reg: u32) -> u32 {
        unsafe { read_volatile((self.mmio_base + reg as usize) as *const u32) }
    }

    /// Write a 32-bit register
    fn write_reg(&self, reg: u32, value: u32) {
        unsafe { write_volatile((self.mmio_base + reg as usize) as *mut u32, value) }
    }

    /// Read MAC address from EEPROM
    fn read_eeprom(&self, addr: u8) -> u16 {
        // Write the address and start bit
        self.write_reg(REG_EERD, ((addr as u32) << EERD_ADDR_SHIFT) | EERD_START);

        // Wait for completion
        loop {
            let val = self.read_reg(REG_EERD);
            if val & EERD_DONE != 0 {
                return ((val >> EERD_DATA_SHIFT) & 0xFFFF) as u16;
            }
        }
    }

    /// Read MAC address from EEPROM or RAL/RAH registers
    fn read_mac_address(&self) -> [u8; 6] {
        // Try to read from EEPROM first
        let word0 = self.read_eeprom(0);
        let word1 = self.read_eeprom(1);
        let word2 = self.read_eeprom(2);

        [
            (word0 & 0xFF) as u8,
            ((word0 >> 8) & 0xFF) as u8,
            (word1 & 0xFF) as u8,
            ((word1 >> 8) & 0xFF) as u8,
            (word2 & 0xFF) as u8,
            ((word2 >> 8) & 0xFF) as u8,
        ]
    }

    /// Get virtual address to physical address (identity mapped for now)
    fn virt_to_phys(virt: usize) -> u64 {
        // In Breenix, we use identity mapping for kernel addresses
        // The physical address is the virtual address minus the kernel offset
        // For now, assume identity mapping in the physical memory region
        PhysAddr::from_kernel_virt(virt)
    }

    /// Reset the device
    fn reset(&self) {
        // Disable interrupts
        self.write_reg(REG_IMC, 0xFFFF_FFFF);

        // Reset the device
        let ctrl = self.read_reg(REG_CTRL);
        self.write_reg(REG_CTRL, ctrl | CTRL_RST);

        // Wait for reset to complete (typically < 1ms)
        for _ in 0..1000 {
            if self.read_reg(REG_CTRL) & CTRL_RST == 0 {
                break;
            }
            // Small delay
            for _ in 0..1000 {
                core::hint::spin_loop();
            }
        }

        // Disable interrupts again after reset
        self.write_reg(REG_IMC, 0xFFFF_FFFF);
    }

    /// Initialize receive functionality
    fn init_rx(&mut self) {
        // Set up receive descriptor ring physical address
        let rx_ring_phys = Self::virt_to_phys(self.rx_ring.as_ptr() as usize);
        self.write_reg(REG_RDBAL, rx_ring_phys as u32);
        self.write_reg(REG_RDBAH, (rx_ring_phys >> 32) as u32);

        // Set receive descriptor ring length
        self.write_reg(
            REG_RDLEN,
            (RX_RING_SIZE * core::mem::size_of::<RxDesc>()) as u32,
        );

        // Set head and tail pointers
        self.write_reg(REG_RDH, 0);
        self.write_reg(REG_RDT, (RX_RING_SIZE - 1) as u32);

        // Configure receive control register
        // Enable receiver, accept broadcast, 2KB buffers, strip CRC
        self.write_reg(
            REG_RCTL,
            RCTL_EN | RCTL_BAM | RCTL_SZ_2048 | RCTL_SECRC | RCTL_BSEX,
        );

        log::info!("E1000: RX initialized with {} descriptors", RX_RING_SIZE);
    }

    /// Initialize transmit functionality
    fn init_tx(&mut self) {
        // Set up transmit descriptor ring physical address
        let tx_ring_phys = Self::virt_to_phys(self.tx_ring.as_ptr() as usize);
        self.write_reg(REG_TDBAL, tx_ring_phys as u32);
        self.write_reg(REG_TDBAH, (tx_ring_phys >> 32) as u32);

        // Set transmit descriptor ring length
        self.write_reg(
            REG_TDLEN,
            (TX_RING_SIZE * core::mem::size_of::<TxDesc>()) as u32,
        );

        // Set head and tail pointers
        self.write_reg(REG_TDH, 0);
        self.write_reg(REG_TDT, 0);

        // Configure transmit control register
        // Enable transmitter, pad short packets, collision threshold, collision distance
        self.write_reg(
            REG_TCTL,
            TCTL_EN | TCTL_PSP | (0x10 << TCTL_CT_SHIFT) | (0x40 << TCTL_COLD_SHIFT),
        );

        // Set inter-packet gap
        // IPG transmit time: 10 + 8 + 6 (for IEEE 802.3 standard)
        self.write_reg(REG_TIPG, 10 | (8 << 10) | (6 << 20));

        log::info!("E1000: TX initialized with {} descriptors", TX_RING_SIZE);
    }

    /// Set up the MAC address filter
    fn init_mac_filter(&self) {
        let mac = &self.mac_addr;

        // Write to Receive Address Low (RAL)
        let ral = (mac[0] as u32)
            | ((mac[1] as u32) << 8)
            | ((mac[2] as u32) << 16)
            | ((mac[3] as u32) << 24);
        self.write_reg(REG_RAL, ral);

        // Write to Receive Address High (RAH) with Address Valid bit
        let rah = (mac[4] as u32) | ((mac[5] as u32) << 8) | RAH_AV;
        self.write_reg(REG_RAH, rah);

        // Clear Multicast Table Array
        for i in 0..128 {
            self.write_reg(REG_MTA + (i * 4), 0);
        }
    }

    /// Enable link
    fn enable_link(&self) {
        let ctrl = self.read_reg(REG_CTRL);
        // Set Link Up, Auto-Speed Detection Enable
        self.write_reg(REG_CTRL, ctrl | CTRL_SLU | CTRL_ASDE);
    }

    /// Check if link is up
    pub fn link_up(&self) -> bool {
        self.read_reg(REG_STATUS) & STATUS_LU != 0
    }

    /// Get link speed in Mbps
    pub fn link_speed(&self) -> u32 {
        let status = self.read_reg(REG_STATUS);
        match (status >> 6) & 0x3 {
            0b00 => 10,
            0b01 => 100,
            _ => 1000,
        }
    }

    /// Get MAC address
    pub fn mac_address(&self) -> &[u8; 6] {
        &self.mac_addr
    }

    /// Transmit a packet
    pub fn transmit(&mut self, data: &[u8]) -> Result<(), &'static str> {
        if data.len() > ETH_FRAME_MAX {
            return Err("Packet too large");
        }

        let idx = self.tx_cur;

        // Wait for the descriptor to be available
        // In a real driver, we'd use interrupts, but for now poll
        let desc = &self.tx_ring[idx];
        if desc.cmd & TXD_CMD_RS != 0 && !desc.is_done() {
            return Err("TX ring full");
        }

        // Copy data to a buffer (we need a stable physical address)
        // For now, allocate a buffer each time (inefficient but simple)
        let mut tx_buffer = Box::new([0u8; ETH_FRAME_MAX]);
        tx_buffer[..data.len()].copy_from_slice(data);
        let phys_addr = Self::virt_to_phys(tx_buffer.as_ptr() as usize);

        // Set up the descriptor
        self.tx_ring[idx].addr = phys_addr;
        self.tx_ring[idx].length = data.len() as u16;
        self.tx_ring[idx].cmd = TXD_CMD_EOP | TXD_CMD_IFCS | TXD_CMD_RS;
        self.tx_ring[idx].status = 0;

        // Advance tail pointer
        self.tx_cur = (self.tx_cur + 1) % TX_RING_SIZE;
        self.write_reg(REG_TDT, self.tx_cur as u32);

        // Wait for transmit to complete (polling for now)
        for _ in 0..10000 {
            if self.tx_ring[idx].is_done() {
                return Ok(());
            }
            core::hint::spin_loop();
        }

        // Don't leak the buffer if we timeout
        core::mem::forget(tx_buffer);

        Err("TX timeout")
    }

    /// Check if a packet is available to receive
    pub fn can_receive(&self) -> bool {
        self.rx_ring[self.rx_cur].is_done()
    }

    /// Receive a packet
    /// Returns the number of bytes received and copies data to the provided buffer
    pub fn receive(&mut self, buffer: &mut [u8]) -> Result<usize, &'static str> {
        let idx = self.rx_cur;

        if !self.rx_ring[idx].is_done() {
            return Err("No packet available");
        }

        let desc = &self.rx_ring[idx];
        let len = desc.length as usize;

        if len > buffer.len() {
            return Err("Buffer too small");
        }

        // Check for errors
        if desc.errors != 0 {
            // Reset the descriptor for reuse
            self.rx_ring[idx].status = 0;
            self.rx_ring[idx].errors = 0;
            let old_tail = self.read_reg(REG_RDT);
            self.write_reg(REG_RDT, (old_tail + 1) % RX_RING_SIZE as u32);
            self.rx_cur = (self.rx_cur + 1) % RX_RING_SIZE;
            return Err("Receive error");
        }

        // Copy data from the receive buffer
        let rx_buf = &self.rx_buffers[idx];
        buffer[..len].copy_from_slice(&rx_buf[..len]);

        // Reset the descriptor for reuse
        self.rx_ring[idx].status = 0;
        self.rx_ring[idx].length = 0;

        // Advance tail pointer to give buffer back to hardware
        let old_tail = self.read_reg(REG_RDT);
        self.write_reg(REG_RDT, (old_tail + 1) % RX_RING_SIZE as u32);

        // Advance our current index
        self.rx_cur = (self.rx_cur + 1) % RX_RING_SIZE;

        Ok(len)
    }

    /// Handle interrupt
    pub fn handle_interrupt(&mut self) {
        let icr = self.read_reg(REG_ICR);

        if icr & ICR_RXT0 != 0 {
            // Receive timer expired - packets available
            log::debug!("E1000: RX interrupt");
        }

        if icr & ICR_TXDW != 0 {
            // Transmit descriptor written back
            log::debug!("E1000: TX interrupt");
        }

        if icr & ICR_LSC != 0 {
            // Link status change
            if self.link_up() {
                log::info!("E1000: Link up at {} Mbps", self.link_speed());
            } else {
                log::info!("E1000: Link down");
            }
        }
    }

    /// Enable interrupts
    #[allow(dead_code)] // Will be used when interrupt handler is wired up
    pub fn enable_interrupts(&self) {
        // Enable RX timer, TX descriptor written back, and link status change
        self.write_reg(REG_IMS, IMS_RXT0 | IMS_TXDW | IMS_LSC);

        // Set receive delay timer to 0 for immediate interrupts
        self.write_reg(REG_RDTR, 0);
        self.write_reg(REG_RADV, 0);
    }

    /// Disable interrupts
    #[allow(dead_code)] // Will be used when interrupt handler is wired up
    pub fn disable_interrupts(&self) {
        self.write_reg(REG_IMC, 0xFFFF_FFFF);
    }
}

/// Global E1000 driver instance
static E1000_DRIVER: Mutex<Option<E1000>> = Mutex::new(None);
static E1000_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Initialize the E1000 driver
pub fn init() -> Result<(), &'static str> {
    // Find the E1000 device on the PCI bus
    let device = pci::find_device(INTEL_VENDOR_ID, E1000_DEVICE_ID)
        .ok_or("E1000 device not found on PCI bus")?;

    log::info!(
        "E1000: Found device at {:02x}:{:02x}.{} IRQ={}",
        device.bus,
        device.device,
        device.function,
        device.interrupt_line
    );

    // Get the MMIO BAR
    let mmio_bar = device.get_mmio_bar().ok_or("E1000: No MMIO BAR found")?;

    log::info!(
        "E1000: MMIO at {:#x} size {:#x}",
        mmio_bar.address,
        mmio_bar.size
    );

    // Map the MMIO region
    let mmio_base = crate::memory::map_mmio(mmio_bar.address, mmio_bar.size as usize)?;

    log::info!("E1000: Mapped MMIO to {:#x}", mmio_base);

    // Enable bus mastering and memory space
    device.enable_bus_master();
    device.enable_memory_space();

    // Allocate descriptor rings
    let rx_ring = Box::new([RxDesc::new(); RX_RING_SIZE]);
    let tx_ring = Box::new([TxDesc::new(); TX_RING_SIZE]);

    // Allocate receive buffers
    let mut rx_buffers = Vec::with_capacity(RX_RING_SIZE);
    for _ in 0..RX_RING_SIZE {
        rx_buffers.push(Box::new([0u8; RX_BUFFER_SIZE]));
    }

    // Create driver instance
    let mut driver = E1000 {
        pci_device: device,
        mmio_base,
        rx_ring,
        rx_buffers,
        tx_ring,
        rx_cur: 0,
        tx_cur: 0,
        mac_addr: [0; 6],
    };

    // Initialize the device
    driver.reset();

    // Read MAC address
    driver.mac_addr = driver.read_mac_address();
    log::info!(
        "E1000: MAC address {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        driver.mac_addr[0],
        driver.mac_addr[1],
        driver.mac_addr[2],
        driver.mac_addr[3],
        driver.mac_addr[4],
        driver.mac_addr[5]
    );

    // Set up MAC address filter
    driver.init_mac_filter();

    // Set up receive buffer addresses in descriptors
    for (i, buf) in driver.rx_buffers.iter().enumerate() {
        driver.rx_ring[i].addr = E1000::virt_to_phys(buf.as_ptr() as usize);
    }

    // Initialize RX and TX
    driver.init_rx();
    driver.init_tx();

    // Enable link
    driver.enable_link();

    // Check link status
    if driver.link_up() {
        log::info!("E1000: Link up at {} Mbps", driver.link_speed());
    } else {
        log::info!("E1000: Link down (waiting for link...)");
    }

    // Store driver instance
    *E1000_DRIVER.lock() = Some(driver);
    E1000_INITIALIZED.store(true, Ordering::Release);

    log::info!("E1000: Driver initialized successfully");
    Ok(())
}

/// Check if the E1000 driver is initialized
#[allow(dead_code)] // Public API for network stack
pub fn is_initialized() -> bool {
    E1000_INITIALIZED.load(Ordering::Acquire)
}

/// Get the MAC address
#[allow(dead_code)] // Public API for network stack
pub fn mac_address() -> Option<[u8; 6]> {
    E1000_DRIVER.lock().as_ref().map(|d| *d.mac_address())
}

/// Check if link is up
#[allow(dead_code)] // Public API for network stack
pub fn link_up() -> bool {
    E1000_DRIVER
        .lock()
        .as_ref()
        .map(|d| d.link_up())
        .unwrap_or(false)
}

/// Transmit a packet
#[allow(dead_code)] // Public API for network stack
pub fn transmit(data: &[u8]) -> Result<(), &'static str> {
    E1000_DRIVER
        .lock()
        .as_mut()
        .ok_or("E1000 not initialized")?
        .transmit(data)
}

/// Receive a packet
#[allow(dead_code)] // Public API for network stack
pub fn receive(buffer: &mut [u8]) -> Result<usize, &'static str> {
    E1000_DRIVER
        .lock()
        .as_mut()
        .ok_or("E1000 not initialized")?
        .receive(buffer)
}

/// Check if a packet is available to receive
#[allow(dead_code)] // Public API for network stack
pub fn can_receive() -> bool {
    E1000_DRIVER
        .lock()
        .as_ref()
        .map(|d| d.can_receive())
        .unwrap_or(false)
}

/// Handle E1000 interrupt
#[allow(dead_code)] // Will be called from interrupt handler
pub fn handle_interrupt() {
    if let Some(driver) = E1000_DRIVER.lock().as_mut() {
        driver.handle_interrupt();
    }
}
