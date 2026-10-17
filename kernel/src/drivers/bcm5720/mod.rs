//! Broadcom BCM5720 Gigabit Ethernet, one driver instance per PCI function.
//!
//! Clean-room: written only from Broadcom's "BCM5718 Family Programmer's
//! Reference Guide" (5718-PG107-R, 2013-07-17, which covers the BCM5719 and
//! BCM5720), the PCI Local Bus 3.0 and PCI Express Base specifications, and
//! IEEE 802.3 clauses 22, 28 and 40. No other driver's source, and no trace of
//! one, was read. Section and page references are to the guide.
//!
//! Each of the controller's two ports is its own PCI function with its own
//! DMA engines, MAC and PHY (Section 2, "Overview of Features", p. 53), so a
//! `Port` owns exactly one function's registers, rings and PHY. A function
//! attaches only when its PHY reports link at handoff; a port with no cable
//! is left as the firmware left it.
//!
//! Attaching a function, with interrupts off at driver initialization:
//! the guide's "Device Reset Procedure" (Section 7, p. 147), including its
//! bootcode mailbox handshake, then the "Initialization Procedure" (Section 7,
//! pp. 140-147) with one send ring, one standard receive producer ring and one
//! receive return ring in host memory, MSI through the local APIC
//! (`arch_impl::x86_64::msi`), and the PHY's autonegotiation restarted.
//!
//! A bring-up probe thread then waits for the link, and, once the tick census
//! has reported on firmware that runs it, sends one frame (a DHCPDISCOVER) and
//! takes received frames by MSI until a DHCPOFFER answers it. It takes no
//! lease, and the network stack does not see these frames: the stack's DHCP client,
//! addressing and production rings arrive with the networking lane.
//!
//! The management firmware: the guide's driver/firmware handshake is the
//! bootcode mailbox at internal memory 0x0b50 around each reset (Section 7,
//! p. 147; Section 10, "Firmware Mailbox", p. 219). The guide does not specify
//! the management processor's own register block (Table 97 gives only its
//! range, 0x10000-0x18fff, p. 271), so the driver does not touch it, and it
//! keeps the EMAC and Mode Control bits that route management traffic as the
//! firmware set them.

mod phy;
mod regs;

use alloc::vec::Vec;
use core::fmt;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{fence, AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

use crate::arch_impl::x86_64::timer;
use crate::drivers::pci::{
    self, pci_read_config_dword, pci_read_config_word, pci_write_config_dword,
    pci_write_config_word, Device,
};
use crate::memory::frame_allocator;
use crate::net::{dhcp, ethernet, ipv4, udp};
use regs::*;

pub const BROADCOM_VENDOR_ID: u16 = 0x14e4;
pub const BCM5720_DEVICE_ID: u16 = 0x165f;

/// The BCM5720's ports, each a PCI function (Section 2, p. 53).
const MAX_PORTS: usize = 2;

/// Ring sizes: the smallest the rings take (Section 4, "Ring Control Blocks",
/// p. 72), except the return ring, which must hold at least the producer
/// rings' combined entries (Section 6, "Ring Control Blocks", p. 123) and
/// fills one page.
const SEND_RING_ENTRIES: u32 = 64;
const STD_RING_ENTRIES: u32 = 64;
const RETURN_RING_ENTRIES: u32 = 128;
/// A producer ring is full one entry short of its size (Section 4, "Producer
/// and Consumer Indices", p. 71).
const RX_BUFFERS: usize = STD_RING_ENTRIES as usize - 1;
/// Each receive buffer is its own page, so a buffer cannot cross a 4 GiB boundary
/// (Section 4, "Descriptor Rings", p. 70).
const RX_BUFFER_SIZE: u32 = 1536;
/// 1518 bytes plus an 802.1Q tag.
const RX_MTU_BYTES: u32 = 1522;
const SEND_BD_WORDS: usize = 4;
const RX_BD_WORDS: usize = 8;
const PAGE: usize = 4096;

const PCI_COMMAND: u8 = 0x04;
const PCI_COMMAND_MEMORY: u16 = 1 << 1;
const PCI_COMMAND_BUS_MASTER: u16 = 1 << 2;
const PCI_CAP_POWER_MANAGEMENT: u8 = 0x01;
const PCI_CAP_EXPRESS: u8 = 0x10;

/// The bootcode answers within 1 s from flash and 10 s from SEEPROM
/// (Section 7, "Device Reset Procedure", step 11, p. 147).
const BOOTCODE_TIMEOUT_MS: u64 = 10_000;
const LINK_TIMEOUT_MS: u64 = 15_000;
const CENSUS_TIMEOUT_MS: u64 = 60_000;
const SEND_TIMEOUT_MS: u64 = 1_000;
const REPLY_TIMEOUT_MS: u64 = 15_000;
/// The DISCOVER is sent again after 4 s and 8 s without an offer.
const DISCOVER_RETRIES: u64 = 2;
const DISCOVER_RETRY_MS: u64 = 4_000;
/// Received frames the probe describes, after the first.
const FRAMES_DESCRIBED: u64 = 16;

/// Each attached port's register window, for its interrupt handler, which
/// takes no lock; 0 until the port's rings are live.
static IRQ_REGS: [AtomicUsize; MAX_PORTS] = [const { AtomicUsize::new(0) }; MAX_PORTS];
/// MSI interrupts each attached port has taken.
static INTERRUPTS: [AtomicU64; MAX_PORTS] = [const { AtomicU64::new(0) }; MAX_PORTS];
/// MSI handlers take no argument, so each port slot has its own.
const HANDLERS: [fn(); MAX_PORTS] = [interrupt_slot_0, interrupt_slot_1];
/// Attached ports, from driver initialization until their probes start.
static ATTACHED: Mutex<Vec<Port>> = Mutex::new(Vec::new());

fn interrupt_slot_0() {
    interrupt(0);
}

fn interrupt_slot_1() {
    interrupt(1);
}

/// Acknowledge the port's interrupt (Section 11, "ISR Flow", step 1,
/// p. 231): a nonzero interrupt mailbox holds further interrupts until the
/// probe has serviced the status block and writes 0 to it.
fn interrupt(slot: usize) {
    let regs = IRQ_REGS[slot].load(Ordering::Acquire);
    if regs == 0 {
        return;
    }
    // SAFETY: the port's mapped register window, published once it was live.
    unsafe { write_volatile((regs + MBOX_INTERRUPT_0) as *mut u32, 1) };
    INTERRUPTS[slot].fetch_add(1, Ordering::Release);
}

fn now_ms() -> u64 {
    let hz = u128::from(timer::frequency_hz().max(1));
    (u128::from(timer::rdtsc()) * 1000 / hz) as u64
}

/// Spin for at least `us` microseconds of TSC time.
fn delay_us(us: u64) {
    let end = timer::rdtsc() + timer::frequency_hz() * us / 1_000_000;
    while timer::rdtsc() < end {
        core::hint::spin_loop();
    }
}

/// Wait `ms` milliseconds on a thread with interrupts enabled.
fn sleep_ms(ms: u64) {
    let end = now_ms() + ms;
    while now_ms() < end {
        crate::arch_halt_with_interrupts();
    }
}

struct Bdf(u8, u8, u8);

impl fmt::Display for Bdf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02x}:{:02x}.{}", self.0, self.1, self.2)
    }
}

/// One zeroed, physically contiguous page reachable through the physical
/// memory map. x86 DMA is cache-coherent, so the cacheable mapping is used.
struct DmaPage {
    phys: u64,
    virt: usize,
}

impl DmaPage {
    fn allocate() -> Result<Self, &'static str> {
        let frame = frame_allocator::allocate_frame().ok_or("out of DMA frames")?;
        let phys = frame.start_address().as_u64();
        let virt = (phys + crate::memory::physical_memory_offset().as_u64()) as usize;
        // SAFETY: a frame this driver now owns, mapped by the physical memory map.
        unsafe { core::ptr::write_bytes(virt as *mut u8, 0, PAGE) };
        Ok(Self { phys, virt })
    }

    fn word(&self, index: usize) -> u32 {
        // SAFETY: `index` stays within the page; the controller writes it by DMA.
        unsafe { read_volatile((self.virt as *const u32).add(index)) }
    }

    fn set_word(&self, index: usize, value: u32) {
        // SAFETY: `index` stays within the page, which this driver owns.
        unsafe { write_volatile((self.virt as *mut u32).add(index), value) }
    }

    fn high(&self) -> u32 {
        (self.phys >> 32) as u32
    }

    fn low(&self) -> u32 {
        self.phys as u32
    }
}

/// One PCI function's registers, internal memory window and PHY.
struct Function {
    device: Device,
    regs: usize,
    phy: u8,
}

impl Function {
    fn bdf(&self) -> Bdf {
        Bdf(self.device.bus, self.device.device, self.device.function)
    }

    fn read32(&self, offset: usize) -> u32 {
        // SAFETY: an offset within the mapped 64 KiB register window.
        unsafe { read_volatile((self.regs + offset) as *const u32) }
    }

    fn write32(&self, offset: usize, value: u32) {
        // SAFETY: an offset within the mapped 64 KiB register window.
        unsafe { write_volatile((self.regs + offset) as *mut u32, value) }
    }

    fn set_bits(&self, offset: usize, bits: u32) {
        self.write32(offset, self.read32(offset) | bits);
    }

    fn config_read(&self, offset: u8) -> u32 {
        pci_read_config_dword(
            self.device.bus,
            self.device.device,
            self.device.function,
            offset,
        )
    }

    fn config_write(&self, offset: u8, value: u32) {
        pci_write_config_dword(
            self.device.bus,
            self.device.device,
            self.device.function,
            offset,
            value,
        );
    }

    fn command(&self) -> u16 {
        pci_read_config_word(
            self.device.bus,
            self.device.device,
            self.device.function,
            PCI_COMMAND,
        )
    }

    fn set_command(&self, value: u16) {
        pci_write_config_word(
            self.device.bus,
            self.device.device,
            self.device.function,
            PCI_COMMAND,
            value,
        );
    }

    /// Point the memory window at `address`'s 32 KiB and return where it
    /// appears in the register window (Section 9, "Standard Mode", p. 180).
    fn window(&self, address: u32) -> usize {
        self.config_write(PCI_MEMORY_BASE, address & !(MEMORY_WINDOW_SIZE - 1));
        MEMORY_WINDOW + (address & (MEMORY_WINDOW_SIZE - 1)) as usize
    }

    fn mem_read(&self, address: u32) -> u32 {
        let at = self.window(address);
        self.read32(at)
    }

    fn mem_write(&self, address: u32, value: u32) {
        let at = self.window(address);
        self.write32(at, value);
    }

    /// Map BAR0 and select the register access a little-endian host uses:
    /// endian word swap, indirect access and PCI State writes, interrupts
    /// masked (Section 7, "Device Reset Procedure", step 4, p. 147).
    fn map(device: Device) -> Result<(Self, u32, u16), &'static str> {
        let bar = device.get_mmio_bar().ok_or("no memory BAR")?;
        let regs = crate::memory::map_mmio(bar.address, bar.size as usize)?;
        let phy = PHY_ADDRESS_PORT_0 + device.function;
        let function = Self { device, regs, phy };
        let host_control = function.config_read(PCI_MISC_HOST_CONTROL);
        let command = function.command();
        function.set_host_control();
        function.set_command(command | PCI_COMMAND_MEMORY);
        Ok((function, host_control, command))
    }

    /// Put back what `map` changed, for a function this driver does not attach.
    fn release(&self, host_control: u32, command: u16) {
        self.config_write(PCI_MISC_HOST_CONTROL, host_control & 0xffff);
        self.set_command(command);
    }

    fn set_host_control(&self) {
        self.config_write(
            PCI_MISC_HOST_CONTROL,
            MHC_ENDIAN_WORD_SWAP | MHC_INDIRECT_ACCESS | MHC_PCI_STATE_RW | MHC_MASK_INTERRUPT,
        );
    }

    /// The guide's Device Reset Procedure (Section 7, p. 147). Returns how long
    /// the bootcode took to answer the mailbox.
    fn reset(&self) -> Result<u64, &'static str> {
        // 1. Tell the bootcode this is a driver-initiated warm reset.
        self.mem_write(NIC_FIRMWARE_MAILBOX, T3_MAGIC_NUMBER);
        // 2. The core-clock reset clears the command register's memory enable.
        let command = self.command();
        // 3. Clear the fast boot program counter; enable the memory arbiter.
        self.write32(FAST_BOOT_PC, 0);
        self.set_bits(MEMORY_ARBITER_MODE, MODE_ENABLE);
        // 4. Host control, as before.
        self.set_host_control();
        // 5. Reset the core clocks, sparing the PCI Express block.
        self.set_bits(MISC_CONFIG, MISC_CONFIG_KEEP_PCIE | MISC_CONFIG_GRC_RESET);
        // 6. The reset cannot be polled: wait at least 1 ms.
        delay_us(2_000);
        // 7. Memory space decode and bus mastering.
        self.set_command(command | PCI_COMMAND_MEMORY | PCI_COMMAND_BUS_MASTER);
        // 8. The memory arbiter again.
        self.set_bits(MEMORY_ARBITER_MODE, MODE_ENABLE);
        // 9. Host control again.
        self.set_host_control();
        // 10. Swap frame data and descriptors for a little-endian host
        //     (Section 9, "Endian Control", pp. 194-203): frames arrive in wire
        //     order, and descriptors and the status block in the 32-bit word
        //     order of the guide's tables (Table 67, p. 202). The management
        //     pass-through data swaps a little-endian host needs as well
        //     (Mode Control, bits 7:6, p. 473).
        let mode = self.read32(MODE_CONTROL) & !MODE_BYTE_SWAP_BD;
        self.write32(
            MODE_CONTROL,
            mode | MODE_WORD_SWAP_DATA
                | MODE_BYTE_SWAP_DATA
                | MODE_WORD_SWAP_BD
                | MODE_WORD_SWAP_B2HRX
                | MODE_BYTE_SWAP_B2HRX,
        );
        // 11. Wait for the bootcode's answer: the one's complement of the magic.
        let start = now_ms();
        loop {
            if self.mem_read(NIC_FIRMWARE_MAILBOX) == !T3_MAGIC_NUMBER {
                return Ok(now_ms() - start);
            }
            if now_ms() - start > BOOTCODE_TIMEOUT_MS {
                return Err("the bootcode did not answer the reset mailbox within 10 s");
            }
            delay_us(100);
        }
    }

    /// MAC address 0 as the bootcode loaded it from NVRAM (Section 6, "MAC
    /// Address Setup/Configuration", Table 36, p. 136).
    fn mac_address(&self) -> Result<[u8; 6], &'static str> {
        let high = self.read32(MAC_ADDRESS_0_HIGH);
        let low = self.read32(MAC_ADDRESS_0_HIGH + 4);
        let mac = [
            (high >> 8) as u8,
            high as u8,
            (low >> 24) as u8,
            (low >> 16) as u8,
            (low >> 8) as u8,
            low as u8,
        ];
        if mac == [0; 6] || mac[0] & 1 != 0 {
            return Err("MAC address 0 holds no unicast address");
        }
        Ok(mac)
    }
}

/// An attached port: its function, its rings and buffers in host memory, and
/// the driver's ring indices.
struct Port {
    f: Function,
    slot: usize,
    vector: u8,
    mac: [u8; 6],
    status: DmaPage,
    send_ring: DmaPage,
    std_ring: DmaPage,
    return_ring: DmaPage,
    rx_buffers: Vec<DmaPage>,
    tx_buffer: DmaPage,
    send_producer: u32,
    std_producer: u32,
    return_consumer: u32,
    autoneg_started_ms: u64,
    /// The Receive BD Initiator Mode register before initialization set it.
    rbdi_at_init: u32,
    /// Return ring entries flagged in error, and the last one's error flags.
    rx_errored: u32,
    rx_last_error_flags: u32,
}

impl Port {
    fn attach(f: Function, slot: usize) -> Result<Self, &'static str> {
        let asic = f.config_read(PCI_MISC_HOST_CONTROL) >> 16;
        let phy_id = f.phy_id()?;
        log::info!(
            "[bcm5720] found {} rev {} asic {:#06x} phy {:#010x} at address {}",
            f.bdf(),
            f.device.revision_id,
            asic,
            phy_id,
            f.phy
        );
        let bootcode_ms = f.reset()?;
        log::info!("[bcm5720] reset complete: the bootcode answered in {bootcode_ms} ms");
        let mac = f.mac_address()?;

        let mut rx_buffers = Vec::new();
        rx_buffers
            .try_reserve_exact(RX_BUFFERS)
            .map_err(|_| "out of memory")?;
        for _ in 0..RX_BUFFERS {
            rx_buffers.push(DmaPage::allocate()?);
        }
        let vector = crate::arch_impl::x86_64::msi::enable(&f.device, HANDLERS[slot])?;
        let mut port = Self {
            f,
            slot,
            vector,
            mac,
            status: DmaPage::allocate()?,
            send_ring: DmaPage::allocate()?,
            std_ring: DmaPage::allocate()?,
            return_ring: DmaPage::allocate()?,
            rx_buffers,
            tx_buffer: DmaPage::allocate()?,
            send_producer: 0,
            std_producer: 0,
            return_consumer: 0,
            autoneg_started_ms: 0,
            rbdi_at_init: 0,
            rx_errored: 0,
            rx_last_error_flags: 0,
        };
        port.initialize();
        port.f.phy_start_autonegotiation()?;
        port.autoneg_started_ms = now_ms();
        port.post_receive_buffers();
        IRQ_REGS[slot].store(port.f.regs, Ordering::Release);
        port.enable_interrupts();
        port.report_receive_state("after initialization");
        log::info!(
            "[bcm5720] {}: MSI vector {:#x}; send ring {}, standard receive ring {}, return ring {}; autonegotiation started",
            port.f.bdf(),
            vector,
            SEND_RING_ENTRIES,
            STD_RING_ENTRIES,
            RETURN_RING_ENTRIES
        );
        Ok(port)
    }

    /// The guide's Initialization Procedure (Section 7, pp. 140-147), step by
    /// step, for one host send ring, one standard producer ring and return
    /// ring 0. Steps 1 (the reset) and 72 (the PHY) run around it.
    fn initialize(&mut self) {
        let f = &self.f;
        // 2. Errata bug-fix bits: the guide defers to the family errata
        //    documents (5720-ES10X-R), which are not public; this driver sets
        //    no errata bits.
        // 3. Tagged status mode stays off.
        // 4. The status block page was zeroed when it was allocated.
        // 5-6. The DMA write watermark from the PCI Express maximum payload
        //      size, and 64-byte cache alignment for DMA writes.
        let mut dma = f.config_read(PCI_DMA_RW_CONTROL)
            & !(DMA_RW_WRITE_WATERMARK_MASK | DMA_RW_CACHE_ALIGNMENT_MASK);
        let watermark = if self.max_payload_bytes() <= 128 {
            DMA_RW_WRITE_WATERMARK_128
        } else {
            DMA_RW_WRITE_WATERMARK_256
        };
        dma |= watermark << DMA_RW_WRITE_WATERMARK_SHIFT;
        f.config_write(PCI_DMA_RW_CONTROL, dma);
        // 7. Byte swapping for big-endian hosts only. 8-9. Host-based send
        //    BDs; the host stack is ready.
        f.set_bits(MODE_CONTROL, MODE_HOST_SEND_BDS | MODE_HOST_STACK_UP);
        // 10. No checksum offload is used.
        // 11-12. Mbuf watermarks, and frames received past the low watermark.
        f.write32(MAC_RX_MBUF_LOW_WATERMARK, 0x2a);
        f.write32(MBUF_HIGH_WATERMARK, 0xa0);
        f.write32(
            LOW_WATERMARK_MAX_RX_FRAMES,
            (f.read32(LOW_WATERMARK_MAX_RX_FRAMES) & !0xffff) | 1,
        );
        // 13. The buffer manager.
        f.set_bits(BUFFER_MANAGER_MODE, MODE_ENABLE | MODE_ATTN_ENABLE);
        // 14. Standard ring replenish threshold.
        f.write32(STD_RING_REPLENISH_THRESHOLD, 0x19);
        // 15-16. The standard producer ring's control block; its NIC address
        //        keeps its default on this family. The jumbo ring is not used.
        f.write32(STD_RING_RCB + RCB_HOST_HIGH as usize, self.std_ring.high());
        f.write32(STD_RING_RCB + RCB_HOST_LOW as usize, self.std_ring.low());
        f.write32(
            STD_RING_RCB + RCB_LENGTH_FLAGS as usize,
            (STD_RING_ENTRIES << RCB_MAX_LEN_SHIFT) | (RX_BUFFER_SIZE << RCB_MAX_FRAME_SHIFT),
        );
        f.write32(
            JUMBO_RING_RCB + RCB_LENGTH_FLAGS as usize,
            RCB_FLAG_RING_DISABLED,
        );
        // 17. The standard producer index starts at 0.
        f.write32(MBOX_STD_PRODUCER, 0);
        // 18. Standard ring replenish watermark.
        f.write32(STD_RING_REPLENISH_WATERMARK, 0x20);
        // 19. The send producer index starts at 0, and the host send ring's
        //     control block in internal memory (Section 6, "Ring Control
        //     Block" and "Host-Based Send Ring", pp. 108-109); the other send rings
        //     are disabled.
        f.write32(MBOX_SEND_PRODUCER, 0);
        f.mem_write(NIC_SEND_RCB + RCB_HOST_HIGH, self.send_ring.high());
        f.mem_write(NIC_SEND_RCB + RCB_HOST_LOW, self.send_ring.low());
        f.mem_write(
            NIC_SEND_RCB + RCB_LENGTH_FLAGS,
            SEND_RING_ENTRIES << RCB_MAX_LEN_SHIFT,
        );
        f.mem_write(NIC_SEND_RCB + RCB_NIC_ADDRESS, NIC_SEND_BD_STAGING);
        for ring in 1..SEND_RINGS {
            f.mem_write(
                NIC_SEND_RCB + ring * 0x10 + RCB_LENGTH_FLAGS,
                RCB_FLAG_RING_DISABLED,
            );
        }
        // 20. Disable the unused return rings. 21. Return ring 0.
        for ring in 1..RETURN_RINGS {
            f.mem_write(
                NIC_RETURN_RCB + ring * 0x10 + RCB_LENGTH_FLAGS,
                RCB_FLAG_RING_DISABLED,
            );
        }
        f.mem_write(NIC_RETURN_RCB + RCB_HOST_HIGH, self.return_ring.high());
        f.mem_write(NIC_RETURN_RCB + RCB_HOST_LOW, self.return_ring.low());
        f.mem_write(
            NIC_RETURN_RCB + RCB_LENGTH_FLAGS,
            RETURN_RING_ENTRIES << RCB_MAX_LEN_SHIFT,
        );
        f.mem_write(NIC_RETURN_RCB + RCB_NIC_ADDRESS, 0);
        // 22. The standard producer mailbox, cleared.
        f.write32(MBOX_STD_PRODUCER, 0);
        // 23. MAC address registers 0-3 hold the port's address (p. 136).
        let mac = self.mac;
        for index in 0..MAC_ADDRESSES {
            let at = MAC_ADDRESS_0_HIGH + index * 8;
            f.write32(at, (u32::from(mac[0]) << 8) | u32::from(mac[1]));
            f.write32(at + 4, u32::from_be_bytes([mac[2], mac[3], mac[4], mac[5]]));
        }
        // 24. The transmit backoff seed.
        let seed = mac.iter().map(|&octet| u32::from(octet)).sum::<u32>() & 0x3ff;
        f.write32(TX_RANDOM_BACKOFF, seed);
        // 25. The largest frame accepted.
        f.write32(RX_MTU, RX_MTU_BYTES);
        // 26. Inter-packet gap and slot time.
        f.write32(
            TX_MAC_LENGTHS,
            (f.read32(TX_MAC_LENGTHS) & !0xffff) | TX_MAC_LENGTHS_INIT,
        );
        // 27. Frames no rule matches go to class 1.
        f.write32(RX_RULES_CONFIG, RX_RULES_DEFAULT_CLASS_1);
        // 28-30. Receive lists and their statistics.
        f.write32(RX_LIST_PLACEMENT_CONFIG, RX_LIST_PLACEMENT_CONFIG_INIT);
        f.write32(RX_LIST_PLACEMENT_STATS_MASK, 0x7b_ffff);
        f.write32(RX_LIST_PLACEMENT_STATS_CONTROL, 1);
        // 31-32. Send statistics.
        f.write32(SEND_DATA_INITIATOR_STATS_MASK, 0xff_ffff);
        f.write32(SEND_DATA_INITIATOR_STATS_CONTROL, SDI_STATS_ENABLE_FASTER);
        // 33-34. Stop host coalescing, then wait up to 20 ms for it to stop.
        f.write32(HC_MODE, 0);
        for _ in 0..200 {
            if f.read32(HC_MODE) == 0 {
                break;
            }
            delay_us(100);
        }
        // 35-37. Coalescing ticks and frame counts. The guide's step 37 names
        //        0x3c20 and 0x3c24; the register definitions place the
        //        during-interrupt counts at 0x3c18 and 0x3c1c (pp. 421-422).
        f.write32(HC_RX_TICKS, 0x48);
        f.write32(HC_TX_TICKS, 0x14);
        f.write32(HC_RX_MAX_FRAMES, 0x05);
        f.write32(HC_TX_MAX_FRAMES, 0x35);
        f.write32(HC_RX_MAX_FRAMES_DURING_INT, 0x05);
        f.write32(HC_TX_MAX_FRAMES_DURING_INT, 0x05);
        // 38. Where the status block goes.
        f.write32(HC_STATUS_BLOCK_HIGH, self.status.high());
        f.write32(HC_STATUS_BLOCK_LOW, self.status.low());
        // 39. Host coalescing on. The low three bits of the MSI data come from
        //     Host Coalescing Mode bits 6:4 (Section 11, "MSI", p. 240), so
        //     they carry the vector's own.
        f.write32(
            HC_MODE,
            MODE_ENABLE | (u32::from(self.vector & 0b111) << HC_MODE_MSI_BITS_SHIFT),
        );
        // 40-41. Receive BD completion and receive list placement.
        f.write32(RX_BD_COMPLETION_MODE, MODE_ENABLE | MODE_ATTN_ENABLE);
        f.write32(RX_LIST_PLACEMENT_MODE, MODE_ENABLE);
        // 42-43. DMA engines and statistics, GMII until the link resolves,
        //        keeping the management firmware's EMAC paths.
        let emac = (f.read32(EMAC_MODE) & !(EMAC_MODE_PORT_MODE_MASK | EMAC_MODE_HALF_DUPLEX))
            | EMAC_MODE_PORT_MODE_GMII;
        f.write32(
            EMAC_MODE,
            emac | EMAC_MODE_ENABLE_FHDE
                | EMAC_MODE_ENABLE_RDE
                | EMAC_MODE_ENABLE_TDE
                | EMAC_MODE_CLEAR_TX_STATS
                | EMAC_MODE_ENABLE_TX_STATS
                | EMAC_MODE_CLEAR_RX_STATS
                | EMAC_MODE_ENABLE_RX_STATS,
        );
        // 44-46. Interrupt on attention.
        delay_us(40);
        f.set_bits(MISC_LOCAL_CONTROL, MLC_INTERRUPT_ON_ATTENTION);
        delay_us(100);
        // 47-49. Write DMA, with the status block sent before the interrupt.
        f.write32(
            WRITE_DMA_MODE,
            MODE_ENABLE | DMA_MODE_ATTENTIONS | WRITE_DMA_STATUS_BEFORE_INTERRUPT,
        );
        delay_us(40);
        // 50. Limit BD fetches to 256 bytes.
        f.set_bits(READ_DMA_RESERVED_CONTROL, READ_DMA_FIFO_OVERRUN_FIX);
        // 51-54. Read DMA, multiple outstanding reads, 4 KiB LSO bursts.
        let read_dma = f.read32(READ_DMA_MODE) & !READ_DMA_DISABLE_MULTIPLE_OUTSTANDING;
        f.write32(READ_DMA_MODE, read_dma | MODE_ENABLE | DMA_MODE_ATTENTIONS);
        delay_us(40);
        let read_dma = f.read32(READ_DMA_MODE) & !READ_DMA_DISABLE_MULTIPLE_OUTSTANDING;
        f.write32(READ_DMA_MODE, read_dma | READ_DMA_LSO_BURST_4K);
        // 55. 4 KiB bursts for standard frames.
        f.set_bits(READ_DMA_CORRUPTION_CONTROL, READ_DMA_NON_LSO_BURST_4K);
        // 56-62. The remaining receive and send blocks. The receive BD
        //        initiator, which the procedure leaves at its reset value, is
        //        enabled explicitly, its value as found recorded first.
        self.rbdi_at_init = f.read32(RX_BD_INITIATOR_MODE);
        f.set_bits(RX_BD_INITIATOR_MODE, MODE_ENABLE);
        f.write32(RX_DATA_COMPLETION_MODE, MODE_ENABLE | MODE_ATTN_ENABLE);
        f.write32(SEND_DATA_COMPLETION_MODE, MODE_ENABLE);
        f.write32(SEND_BD_COMPLETION_MODE, MODE_ENABLE | MODE_ATTN_ENABLE);
        f.write32(
            RX_DATA_BD_INITIATOR_MODE,
            MODE_ENABLE | RDI_MODE_ILLEGAL_RETURN_RING_SIZE,
        );
        f.write32(SEND_DATA_INITIATOR_MODE, MODE_ENABLE);
        f.write32(SEND_BD_INITIATOR_MODE, MODE_ENABLE | MODE_ATTN_ENABLE);
        f.write32(SEND_BD_SELECTOR_MODE, MODE_ENABLE | MODE_ATTN_ENABLE);
        // 63-64. The transmit MAC, keeping its management arbitration.
        f.set_bits(
            TX_MAC_MODE,
            TX_MAC_MODE_ENABLE | TX_MAC_MODE_MBUF_LOCKUP_FIX,
        );
        delay_us(100);
        // 65-66. The receive MAC.
        f.set_bits(RX_MAC_MODE, RX_MAC_MODE_ENABLE);
        delay_us(10);
        // 67. LEDs.
        f.write32(LED_CONTROL, LED_CONTROL_INIT);
        // 68-69. A link attention, and link state from the PHY's link signal
        //        rather than auto-polling (Section 10, "Configuring How MAC
        //        Detects Link Up/Down", p. 204).
        f.write32(MI_STATUS, MI_STATUS_LINK);
        f.write32(MI_MODE, f.read32(MI_MODE) & !MI_MODE_PORT_POLLING);
        // 70. Done at step 12. 71. Power state D0.
        self.set_d0();
        // 73. No multicast filter: the hash registers keep their reset zeros.
        // MSI with untagged status (the MSI Mode register's note, p. 470).
        f.set_bits(MSI_MODE, MODE_ENABLE | MSI_MODE_SINGLE_SHOT_DISABLE);
    }

    /// The PCI Express maximum payload size, from Device Control bits 7:5.
    fn max_payload_bytes(&self) -> u32 {
        let Some(express) = self.f.device.find_capability(PCI_CAP_EXPRESS) else {
            return 128;
        };
        let control = self.f.config_read(express + 8);
        128 << ((control >> 5) & 0b111)
    }

    /// Step 71: the power management control/status register's state field
    /// to D0 (PCI Bus Power Management Interface).
    fn set_d0(&self) {
        if let Some(pm) = self.f.device.find_capability(PCI_CAP_POWER_MANAGEMENT) {
            let control = self.f.config_read(pm + 4);
            if control & 0b11 != 0 {
                self.f.config_write(pm + 4, control & !0b11);
            }
        }
    }

    /// Step 74: clear and unmask the host interrupt, and write 0 to interrupt
    /// mailbox 0.
    fn enable_interrupts(&self) {
        let control = self.f.config_read(PCI_MISC_HOST_CONTROL) & 0xffff;
        self.f.config_write(
            PCI_MISC_HOST_CONTROL,
            (control & !MHC_MASK_INTERRUPT) | MHC_CLEAR_INTERRUPT,
        );
        self.f.write32(MBOX_INTERRUPT_0, 0);
    }

    /// Write standard ring entry `slot` to describe receive buffer `buffer`
    /// (Section 4, Table 11, p. 79, in the word order of Table 67).
    fn describe_receive_buffer(&self, slot: u32, buffer: usize) {
        let page = &self.rx_buffers[buffer];
        let base = slot as usize * RX_BD_WORDS;
        self.std_ring.set_word(base, page.high());
        self.std_ring.set_word(base + 1, page.low());
        self.std_ring
            .set_word(base + 2, ((buffer as u32) << 16) | RX_BUFFER_SIZE);
        self.std_ring.set_word(base + 3, 0);
        self.std_ring.set_word(base + 4, 0);
        self.std_ring.set_word(base + 5, 0);
        self.std_ring.set_word(base + 6, 0);
        self.std_ring.set_word(base + 7, buffer as u32);
    }

    fn post_receive_buffers(&mut self) {
        for buffer in 0..RX_BUFFERS {
            self.describe_receive_buffer(buffer as u32, buffer);
        }
        self.std_producer = RX_BUFFERS as u32;
        fence(Ordering::SeqCst);
        self.f.write32(MBOX_STD_PRODUCER, self.std_producer);
    }

    /// The configuration the link resolved to: GMII at 1000 Mb/s, MII below,
    /// and half duplex where negotiated (Section 10, "Configuring the Ethernet
    /// Controller for GMII and MII Modes", p. 204).
    fn set_link(&self, link: phy::Resolved) {
        let mut emac =
            self.f.read32(EMAC_MODE) & !(EMAC_MODE_PORT_MODE_MASK | EMAC_MODE_HALF_DUPLEX);
        emac |= if link.mbps == 1000 {
            EMAC_MODE_PORT_MODE_GMII
        } else {
            EMAC_MODE_PORT_MODE_MII
        };
        if !link.full_duplex {
            emac |= EMAC_MODE_HALF_DUPLEX;
        }
        self.f.write32(EMAC_MODE, emac);
    }

    /// Queue one frame on the send ring (Section 4, Table 8, p. 76, in the word
    /// order of Table 67) and ring the producer mailbox. The descriptor asks
    /// for an immediate status block update.
    fn send(&mut self, frame: &[u8]) -> Result<(), &'static str> {
        if frame.is_empty() || frame.len() > PAGE {
            return Err("frame length out of range");
        }
        // SAFETY: the page this driver owns; the length was checked above.
        unsafe {
            core::ptr::copy_nonoverlapping(
                frame.as_ptr(),
                self.tx_buffer.virt as *mut u8,
                frame.len(),
            );
        }
        let base = self.send_producer as usize * SEND_BD_WORDS;
        self.send_ring.set_word(base, self.tx_buffer.high());
        self.send_ring.set_word(base + 1, self.tx_buffer.low());
        self.send_ring.set_word(
            base + 2,
            ((frame.len() as u32) << 16) | SEND_BD_PACKET_END | SEND_BD_COAL_NOW,
        );
        self.send_ring.set_word(base + 3, 0);
        self.send_producer = (self.send_producer + 1) % SEND_RING_ENTRIES;
        fence(Ordering::SeqCst);
        self.f.write32(MBOX_SEND_PRODUCER, self.send_producer);
        Ok(())
    }

    /// The send consumer index from the status block (Section 4, Table 14,
    /// p. 84: word 4, bits 31:16).
    fn send_consumer(&self) -> u32 {
        self.status.word(4) >> 16
    }

    /// Service the status block (Section 11, "ISR Flow", p. 231): acknowledge,
    /// clear the updated bit, take each new return ring entry, return its
    /// buffer to the standard ring, write the consumer and producer
    /// mailboxes, and re-enable the interrupt; repeat while the controller
    /// updated the block meanwhile. `each` sees each good frame.
    fn service(&mut self, mut each: impl FnMut(&[u8])) {
        loop {
            self.f.write32(MBOX_INTERRUPT_0, 1);
            self.status
                .set_word(0, self.status.word(0) & !STATUS_WORD_UPDATED);
            fence(Ordering::SeqCst);
            let produced = self.status.word(4) & 0xffff;
            let mut returned = false;
            while self.return_consumer != produced % RETURN_RING_ENTRIES {
                let base = self.return_consumer as usize * RX_BD_WORDS;
                let buffer = (self.return_ring.word(base + 2) >> 16) as usize;
                let length = (self.return_ring.word(base + 2) & 0xffff) as usize;
                let flags = self.return_ring.word(base + 3) & 0xffff;
                self.return_consumer = (self.return_consumer + 1) % RETURN_RING_ENTRIES;
                if buffer >= RX_BUFFERS {
                    continue;
                }
                let good = flags & RX_BD_FRAME_HAS_ERROR == 0
                    && flags & RX_BD_PACKET_END != 0
                    && length <= RX_BUFFER_SIZE as usize;
                if good {
                    // SAFETY: the buffer's page, `length` bytes of it, written by
                    // the controller's DMA before it posted this entry.
                    let frame = unsafe {
                        core::slice::from_raw_parts(
                            self.rx_buffers[buffer].virt as *const u8,
                            length,
                        )
                    };
                    each(frame);
                } else {
                    self.rx_errored += 1;
                    self.rx_last_error_flags =
                        (self.return_ring.word(base + 5) >> 16) | (flags << 16);
                }
                self.describe_receive_buffer(self.std_producer, buffer);
                self.std_producer = (self.std_producer + 1) % STD_RING_ENTRIES;
                returned = true;
            }
            fence(Ordering::SeqCst);
            if returned {
                self.f.write32(MBOX_RETURN_0_CONSUMER, self.return_consumer);
                self.f.write32(MBOX_STD_PRODUCER, self.std_producer);
            }
            self.f.write32(MBOX_INTERRUPT_0, 0);
            // A read flushes the posted mailbox write (Section 11, Figure 53).
            let _ = self.f.read32(MBOX_INTERRUPT_0);
            if self.status.word(0) & STATUS_WORD_UPDATED == 0 {
                return;
            }
        }
    }

    /// The receive path's registers: the standard ring's control block and
    /// producer mailbox as read back, the controller's own indices, the receive
    /// engines' status and the receive list placement counters.
    fn report_receive_state(&self, when: &str) {
        let f = &self.f;
        log::info!(
            "[bcm5720] {} receive state {when}: std rcb {:#x} {:#x} {:#x} {:#x}, std producer mailbox {:#x}, rbdi mode {:#x} (found {:#x}), rbdi status {:#x}, rdi mode {:#x}, rdi status {:#x}, rlp status {:#x}, rx mac mode {:#x}, rx mac status {:#x}",
            f.bdf(),
            f.read32(STD_RING_RCB + RCB_HOST_HIGH as usize),
            f.read32(STD_RING_RCB + RCB_HOST_LOW as usize),
            f.read32(STD_RING_RCB + RCB_LENGTH_FLAGS as usize),
            f.read32(STD_RING_RCB + RCB_NIC_ADDRESS as usize),
            f.read32(MBOX_STD_PRODUCER),
            f.read32(RX_BD_INITIATOR_MODE),
            self.rbdi_at_init,
            f.read32(RX_BD_INITIATOR_STATUS),
            f.read32(RX_DATA_BD_INITIATOR_MODE),
            f.read32(RX_DATA_BD_INITIATOR_STATUS),
            f.read32(RX_LIST_PLACEMENT_STATUS),
            f.read32(RX_MAC_MODE),
            f.read32(RX_MAC_STATUS)
        );
        log::info!(
            "[bcm5720] {} receive indices {when}: nic std consumer {}, rdi local std consumer {:#x}, nic return 0 producer {}, no more rx bds {}, discards {}, errors {}",
            f.bdf(),
            f.read32(NIC_STD_CONSUMER),
            f.read32(RDI_LOCAL_STD_CONSUMER),
            f.read32(NIC_RETURN_0_PRODUCER),
            f.read32(STAT_NO_MORE_RX_BDS),
            f.read32(STAT_IN_DISCARDS),
            f.read32(STAT_IN_ERRORS)
        );
    }

    /// Controller state worth recording when the probe stops short.
    fn report_state(&self, what: &str) {
        self.report_receive_state("when the probe stopped");
        log::warn!(
            "[bcm5720] {}: {what}; status word {:#x}, send consumer {}, return producer {}, standard consumer {}, MSI interrupts {}, emac status {:#x}, tx mac status {:#x}, read dma status {:#x}, write dma status {:#x}, flow attention {:#x}",
            self.f.bdf(),
            self.status.word(0),
            self.send_consumer(),
            self.status.word(4) & 0xffff,
            self.status.word(2) >> 16,
            INTERRUPTS[self.slot].load(Ordering::Acquire),
            self.f.read32(EMAC_STATUS),
            self.f.read32(TX_MAC_STATUS),
            self.f.read32(READ_DMA_STATUS),
            self.f.read32(WRITE_DMA_STATUS),
            self.f.read32(HC_FLOW_ATTENTION)
        );
        log::warn!(
            "[bcm5720] {}: the MAC counted {} octets out, {} broadcasts out, {} octets in, {} broadcasts in, {} unicasts in",
            self.f.bdf(),
            self.f.read32(STAT_OUT_OCTETS),
            self.f.read32(STAT_OUT_BROADCAST),
            self.f.read32(STAT_IN_OCTETS),
            self.f.read32(STAT_IN_BROADCAST),
            self.f.read32(STAT_IN_UNICAST)
        );
    }

    /// The bring-up probe: link, one frame sent, frames received by MSI, and a
    /// reply to the frame sent.
    fn probe(mut self) {
        let bdf = self.f.bdf();
        let link = loop {
            match self.f.phy_resolved() {
                Ok(Some(link)) => break link,
                Ok(None) => {}
                Err(error) => {
                    log::warn!("[bcm5720] {bdf}: PHY unreadable while waiting for link: {error}");
                    return;
                }
            }
            if now_ms() - self.autoneg_started_ms > LINK_TIMEOUT_MS {
                log::warn!(
                    "[bcm5720] {bdf}: no link {LINK_TIMEOUT_MS} ms after autonegotiation started"
                );
                return;
            }
            sleep_ms(20);
        };
        self.set_link(link);
        log::info!(
            "[bcm5720] link up {} Mb/s {} duplex, {} ms after autonegotiation started",
            link.mbps,
            if link.full_duplex { "full" } else { "half" },
            now_ms() - self.autoneg_started_ms
        );

        // Where the tick census runs (no fw_cfg), it reports the interrupt and
        // SMP milestones' last stages once boot is idle; this probe's lines
        // follow them.
        if !crate::drivers::fw_cfg::present() {
            let start = now_ms();
            while !crate::arch_impl::x86_64::tick_census::reported()
                && now_ms() - start < CENSUS_TIMEOUT_MS
            {
                sleep_ms(20);
            }
        }

        let xid = (timer::rdtsc() as u32)
            ^ u32::from_be_bytes([self.mac[2], self.mac[3], self.mac[4], self.mac[5]]);
        let frame = match discover_frame(xid, self.mac) {
            Ok(frame) => frame,
            Err(error) => {
                log::warn!("[bcm5720] {bdf}: no frame to send: {error}");
                return;
            }
        };
        // The MAC's counters clear on read: read them before the frame.
        let _ = self.f.read32(STAT_OUT_OCTETS);
        let _ = self.f.read32(STAT_OUT_BROADCAST);
        if let Err(error) = self.send(&frame) {
            log::warn!("[bcm5720] {bdf}: send failed: {error}");
            return;
        }
        let start = now_ms();
        while self.send_consumer() != self.send_producer {
            if now_ms() - start > SEND_TIMEOUT_MS {
                self.report_state("the controller did not consume the send descriptor within 1 s");
                return;
            }
            sleep_ms(1);
        }
        let octets = self.f.read32(STAT_OUT_OCTETS);
        let broadcasts = self.f.read32(STAT_OUT_BROADCAST);
        if broadcasts == 0 {
            self.report_state(
                "the send descriptor was consumed but the MAC counted no broadcast frame",
            );
            return;
        }
        log::info!(
            "[bcm5720] frame sent: a {}-byte DHCP discover; the MAC counted {broadcasts} broadcast frame(s), {octets} octets",
            frame.len()
        );

        let mut frames = 0u64;
        let mut answered = false;
        let mac = self.mac;
        let slot = self.slot;
        let mut seen_interrupts = 0;
        let mut resent = 0;
        let start = now_ms();
        while !answered && now_ms() - start < REPLY_TIMEOUT_MS {
            // RFC 2131 section 4.1: a client retransmits an unanswered DISCOVER.
            if resent < DISCOVER_RETRIES && now_ms() - start > (resent + 1) * DISCOVER_RETRY_MS {
                resent += 1;
                if let Err(error) = self.send(&frame) {
                    log::warn!("[bcm5720] {bdf}: retransmission failed: {error}");
                }
            }
            let interrupts = INTERRUPTS[slot].load(Ordering::Acquire);
            if interrupts == seen_interrupts && self.status.word(0) & STATUS_WORD_UPDATED == 0 {
                sleep_ms(1);
                continue;
            }
            seen_interrupts = interrupts;
            self.service(|data| {
                frames += 1;
                let summary = FrameSummary::of(data, mac);
                if frames == 1 {
                    log::info!(
                        "[bcm5720] first frame received: {}, after {} MSI interrupt(s)",
                        summary,
                        INTERRUPTS[slot].load(Ordering::Acquire)
                    );
                } else if frames <= FRAMES_DESCRIBED {
                    log::info!("[bcm5720] frame {frames} received: {summary}");
                }
                if summary.from_dhcp_server() {
                    let offer = is_offer_frame(data, xid, mac);
                    log::info!(
                        "[bcm5720] frame {frames} is from the DHCP server port; an offer for this transaction: {}",
                        if offer { "yes" } else { "no" }
                    );
                    answered |= offer;
                }
            });
        }
        if self.rx_errored != 0 {
            log::info!(
                "[bcm5720] {bdf}: {} return ring entries flagged in error, the last with flags {:#x}",
                self.rx_errored,
                self.rx_last_error_flags
            );
        }
        if answered {
            log::info!(
                "[bcm5720] reply received: a DHCP offer answering the discover, frame {frames} of those received, {} ms after it was sent",
                now_ms() - start
            );
        } else {
            self.report_state(if frames == 0 {
                "no frame received within 15 s"
            } else {
                "no DHCP offer answered the discover within 15 s"
            });
        }
        crate::arch_without_interrupts(|| ATTACHED.lock().push(self));
    }
}

/// What a received frame is, without its addresses: length, EtherType,
/// whether it was broadcast, multicast or sent to this port, and for IPv4
/// its protocol and UDP ports.
struct FrameSummary {
    length: usize,
    ethertype: u16,
    destination: &'static str,
    protocol: Option<u8>,
    ports: Option<(u16, u16)>,
}

impl FrameSummary {
    fn of(data: &[u8], mac: [u8; 6]) -> Self {
        let mut summary = Self {
            length: data.len(),
            ethertype: 0,
            destination: "short",
            protocol: None,
            ports: None,
        };
        let Some(frame) = ethernet::EthernetFrame::parse(data) else {
            return summary;
        };
        summary.ethertype = frame.ethertype;
        summary.destination = if frame.dst_mac == ethernet::BROADCAST_MAC {
            "broadcast"
        } else if frame.dst_mac == mac {
            "to this port"
        } else if frame.dst_mac[0] & 1 != 0 {
            "multicast"
        } else {
            "to another station"
        };
        if frame.ethertype != ethernet::ETHERTYPE_IPV4 {
            return summary;
        }
        let Some(ip) = ipv4::Ipv4Packet::parse(frame.payload) else {
            return summary;
        };
        summary.protocol = Some(ip.protocol);
        if ip.protocol == ipv4::PROTOCOL_UDP {
            summary.ports = udp::UdpHeader::parse(ip.payload)
                .map(|(header, _)| (header.src_port, header.dst_port));
        }
        summary
    }

    fn from_dhcp_server(&self) -> bool {
        self.ports == Some((dhcp::SERVER_PORT, dhcp::CLIENT_PORT))
    }
}

impl fmt::Display for FrameSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} bytes, EtherType {:#06x}, {}",
            self.length, self.ethertype, self.destination
        )?;
        if let Some(protocol) = self.protocol {
            write!(f, ", IPv4 protocol {protocol}")?;
        }
        if let Some((source, destination)) = self.ports {
            write!(f, ", UDP {source} to {destination}")?;
        }
        Ok(())
    }
}

/// A broadcast DHCPDISCOVER from `mac`: Ethernet, IPv4 from 0.0.0.0 to the
/// limited broadcast address, and UDP from the client port to the server port
/// (RFC 2131 section 4.1).
fn discover_frame(xid: u32, mac: [u8; 6]) -> Result<Vec<u8>, &'static str> {
    let message = dhcp::discover_message(xid, mac);
    let datagram = udp::try_build_udp_packet(
        [0; 4],
        [255; 4],
        dhcp::CLIENT_PORT,
        dhcp::SERVER_PORT,
        &message,
    )
    .map_err(|_| "UDP datagram")?;
    let packet = ipv4::Ipv4Packet::try_build(
        [0; 4],
        [255; 4],
        ipv4::PROTOCOL_UDP,
        ipv4::DEFAULT_TTL,
        &datagram,
    )
    .map_err(|_| "IPv4 packet")?;
    ethernet::EthernetFrame::build(
        &mac,
        &ethernet::BROADCAST_MAC,
        ethernet::ETHERTYPE_IPV4,
        &packet,
    )
}

/// Whether `data` is a DHCPOFFER answering transaction `xid` for `mac`.
fn is_offer_frame(data: &[u8], xid: u32, mac: [u8; 6]) -> bool {
    let Some(frame) = ethernet::EthernetFrame::parse(data) else {
        return false;
    };
    if frame.ethertype != ethernet::ETHERTYPE_IPV4 {
        return false;
    }
    let Some(ip) = ipv4::Ipv4Packet::parse(frame.payload) else {
        return false;
    };
    if ip.protocol != ipv4::PROTOCOL_UDP {
        return false;
    }
    let Some((header, message)) = udp::UdpHeader::parse(ip.payload) else {
        return false;
    };
    header.src_port == dhcp::SERVER_PORT
        && header.dst_port == dhcp::CLIENT_PORT
        && dhcp::is_offer(message, xid, mac)
}

/// Attach the BCM5720 functions whose PHY has link at handoff, up to
/// `MAX_PORTS`. Called at driver initialization, interrupts off. Returns how
/// many attached.
pub fn init() -> usize {
    let Some(devices) = pci::get_devices() else {
        return 0;
    };
    let mut attached = 0;
    for device in devices.into_iter().filter(|device| {
        device.vendor_id == BROADCOM_VENDOR_ID && device.device_id == BCM5720_DEVICE_ID
    }) {
        let bdf = Bdf(device.bus, device.device, device.function);
        if attached == MAX_PORTS {
            log::info!("[bcm5720] {bdf}: not attached: every port slot is in use");
            continue;
        }
        let (function, host_control, command) = match Function::map(device) {
            Ok(mapped) => mapped,
            Err(error) => {
                log::warn!("[bcm5720] {bdf}: not attached: {error}");
                continue;
            }
        };
        match function.phy_link() {
            Ok(true) => {}
            Ok(false) => {
                function.release(host_control, command);
                log::info!("[bcm5720] {bdf}: no link at handoff; left as the firmware left it");
                continue;
            }
            Err(error) => {
                function.release(host_control, command);
                log::warn!("[bcm5720] {bdf}: not attached: {error}");
                continue;
            }
        }
        match Port::attach(function, attached) {
            Ok(port) => {
                crate::arch_without_interrupts(|| ATTACHED.lock().push(port));
                attached += 1;
            }
            Err(error) => log::warn!("[bcm5720] {bdf}: attach failed: {error}"),
        }
    }
    attached
}

/// Start each attached port's bring-up probe. Called once the scheduler runs
/// and interrupts are enabled.
pub fn start_probes() {
    let ports = crate::arch_without_interrupts(|| core::mem::take(&mut *ATTACHED.lock()));
    for port in ports {
        let bdf = port.f.bdf();
        if let Err(error) = crate::task::kthread::kthread_run(move || port.probe(), "kbcm5720") {
            log::warn!("[bcm5720] {bdf}: probe not started: {error:?}");
        }
    }
}
