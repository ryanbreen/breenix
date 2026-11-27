//! The MAC programming of the Intel 82540EM and 82574, independent of how the
//! kernel reaches the device. It touches the hardware only through
//! [`Registers`] and [`Rings`], so the host runs it against a model of the
//! manuals (`tests/e1000_model.rs`).
//!
//! Sources (see `regs.rs`): "SDM" is the 8254x Software Developer's Manual,
//! "DS" the 82574 datasheet. The bring-up is the order DS 4.6 gives, which the
//! SDM's section 14 matches for the 82540EM:
//!
//! 1. mask every interrupt (DS 4.6.1); stop receive and transmit, on the
//!    82574 by draining bus-master requests (DS 10.2.2.1, 3.1.3.10);
//! 2. global reset through CTRL.RST, waiting before the next access (SDM
//!    13.4.1; DS 10.2.2.1), then mask interrupts again (DS 4.6.1) and, on the
//!    82574, wait for the NVM auto-read (DS 10.2.2.3);
//! 3. general configuration: set link up and let the MAC follow the speed and
//!    duplex the PHY resolves, no flow control (SDM 14.3; DS 4.6.2, 4.6.3.2),
//!    and the 82574's GCR and GCR2 bits (DS 4.6.2, 10.2.3.12);
//! 4. the station address in RAL0/RAH0, the other receive addresses the
//!    driver owns and the multicast table cleared (SDM 14.4; DS 4.6.5);
//! 5. the receive ring, then RCTL with the receiver enabled last (SDM 14.4;
//!    DS 4.6.5.1);
//! 6. the transmit ring, TIPG, and TCTL enabled last (SDM 14.5; DS 4.6.6).
//!
//! The PHY is left as reset left it: it auto-negotiates by default (SDM 14.6)
//! and the MAC takes the resolved link from it. No PHY register is written, so
//! no MDIO ownership protocol is needed (DS 4.5.2).

use super::regs::*;

/// Receive descriptors; RDLEN must be a multiple of 128 bytes (SDM 14.4).
pub const RX_RING: usize = 32;
/// Transmit descriptors; TDLEN must be a multiple of 128 bytes (SDM 14.5).
pub const TX_RING: usize = 32;
/// Bytes per descriptor buffer: RCTL.BSIZE = 00b with BSEX = 0 (SDM 13.4.22).
pub const BUFFER_SIZE: usize = 2048;
/// Bytes per legacy descriptor (SDM 3.2.3, 3.3.3).
pub const DESCRIPTOR_BYTES: usize = 16;
/// Largest frame handed to the MAC: an untagged Ethernet frame without the FCS,
/// which the MAC appends (TDESC.CMD.IFCS, SDM 3.3.3).
pub const MAX_FRAME: usize = 1514;

/// Polls of CTRL.RST, each after `POLL_US` (SDM 13.4.1: wait about 1 us before
/// the first check).
const RESET_POLLS: u32 = 1000;
/// Polls of STATUS.GIO_MASTER_ENABLE after the master disable (DS 3.1.3.10:
/// the driver may time out).
const MASTER_DISABLE_POLLS: u32 = 1000;
/// Polls of EERD.DONE for one NVM word, and of the ownership grant.
const NVM_POLLS: u32 = 1000;
const POLL_US: u32 = 10;

/// The 82540EM's device ID (SDM 5.2, Table 5-1).
pub const I82540EM_DEVICE_ID: u16 = 0x100E;
/// The 82574's default device ID (DS 9.1.2.2).
pub const I82574_DEVICE_ID: u16 = 0x10D3;

/// Which programming interface the function presents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    /// 82540EM and its PCI family (SDM).
    Pci8254x,
    /// 82574 on PCIe (DS).
    Pcie82574,
}

impl Model {
    pub fn from_device_id(device_id: u16) -> Option<Self> {
        match device_id {
            I82540EM_DEVICE_ID => Some(Self::Pci8254x),
            I82574_DEVICE_ID => Some(Self::Pcie82574),
            _ => None,
        }
    }

    /// TIPG for copper at full duplex: SDM 13.4.34 (IPGT 10, IPGR1 8, IPGR2 6);
    /// DS 10.2.6.2's suggested 0x00602006 (IPGT 6, IPGR1 8, IPGR2 6).
    pub fn tipg(self) -> u32 {
        let (ipgt, ipgr1, ipgr2) = match self {
            Self::Pci8254x => (10, 8, 6),
            Self::Pcie82574 => (6, 8, 6),
        };
        ipgt | (ipgr1 << TIPG_IPGR1_SHIFT) | (ipgr2 << TIPG_IPGR2_SHIFT)
    }

    /// TCTL collision threshold and full-duplex collision distance: SDM 14.5
    /// (CT 10h, COLD 40h); DS 4.6.6 (CT 0x0F, COLD 0x03F).
    fn collision_fields(self) -> u32 {
        let (ct, cold) = match self {
            Self::Pci8254x => (0x10, 0x40),
            Self::Pcie82574 => (0x0F, 0x3F),
        };
        (ct << TCTL_CT_SHIFT) | (cold << TCTL_COLD_SHIFT)
    }

    /// The IMC value that masks every cause (regs.rs: SDM 13.4.21; DS
    /// 10.2.4.6).
    fn all_causes(self) -> u32 {
        match self {
            Self::Pci8254x => INT_ALL_8254X,
            Self::Pcie82574 => INT_ALL_82574,
        }
    }

    /// Receive-address entries the driver owns: all 16 on the 8254x (SDM
    /// 13.5.2); 0-14 on the 82574, whose entry 15 belongs to manageability
    /// firmware (DS 10.2.5.23).
    fn receive_addresses(self) -> u32 {
        match self {
            Self::Pci8254x => 16,
            Self::Pcie82574 => 15,
        }
    }
}

/// The interrupt causes worth a wakeup: the receive causes SDM 14.4 and DS
/// 4.6.5 suggest, without RXSEQ, which both parts reserve (SDM Table 13-63;
/// DS 10.2.4.1), and link status change. Only x86-64 takes interrupts; ARM64
/// polls.
#[cfg(any(target_arch = "x86_64", test))]
pub const INTERRUPT_MASK: u32 = INT_RXT0 | INT_RXO | INT_RXDMT0 | INT_LSC;

/// Register access and a delay, all the reset, configuration and interrupt
/// paths need.
pub trait Registers {
    fn read(&mut self, offset: u32) -> u32;
    fn write(&mut self, offset: u32, value: u32);
    fn delay_us(&mut self, us: u32);
}

/// The descriptor rings and their buffers. Implementations make a descriptor
/// write visible to the device before returning, and a read observe what the
/// device last wrote.
pub trait Rings: Registers {
    fn rx_ring_address(&self) -> u64;
    fn tx_ring_address(&self) -> u64;
    fn rx_buffer_address(&self, index: usize) -> u64;
    fn tx_buffer_address(&self, index: usize) -> u64;
    fn rx_descriptor(&mut self, index: usize) -> [u64; 2];
    fn set_rx_descriptor(&mut self, index: usize, words: [u64; 2]);
    fn tx_descriptor(&mut self, index: usize) -> [u64; 2];
    fn set_tx_descriptor(&mut self, index: usize, words: [u64; 2]);
    fn read_rx_buffer(&mut self, index: usize, out: &mut [u8]);
    fn write_tx_buffer(&mut self, index: usize, data: &[u8]);
}

/// Legacy receive descriptor (SDM 3.2.3 Table 3-1; DS 7.1.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RxDescriptor {
    pub buffer: u64,
    pub length: u16,
    pub checksum: u16,
    pub status: u8,
    pub errors: u8,
    pub special: u16,
}

impl RxDescriptor {
    pub fn from_words(words: [u64; 2]) -> Self {
        let high = words[1];
        Self {
            buffer: words[0],
            length: high as u16,
            checksum: (high >> 16) as u16,
            status: (high >> 32) as u8,
            errors: (high >> 40) as u8,
            special: (high >> 48) as u16,
        }
    }

    pub fn to_words(self) -> [u64; 2] {
        [
            self.buffer,
            u64::from(self.length)
                | (u64::from(self.checksum) << 16)
                | (u64::from(self.status) << 32)
                | (u64::from(self.errors) << 40)
                | (u64::from(self.special) << 48),
        ]
    }
}

/// Legacy transmit descriptor (SDM 3.3.3 Table 3-8; DS 7.2.10.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TxDescriptor {
    pub buffer: u64,
    pub length: u16,
    pub cso: u8,
    pub cmd: u8,
    pub status: u8,
    pub css: u8,
    pub special: u16,
}

impl TxDescriptor {
    pub fn from_words(words: [u64; 2]) -> Self {
        let high = words[1];
        Self {
            buffer: words[0],
            length: high as u16,
            cso: (high >> 16) as u8,
            cmd: (high >> 24) as u8,
            status: (high >> 32) as u8,
            css: (high >> 40) as u8,
            special: (high >> 48) as u16,
        }
    }

    pub fn to_words(self) -> [u64; 2] {
        [
            self.buffer,
            u64::from(self.length)
                | (u64::from(self.cso) << 16)
                | (u64::from(self.cmd) << 24)
                | (u64::from(self.status) << 32)
                | (u64::from(self.css) << 40)
                | (u64::from(self.special) << 48),
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitError {
    MasterDrainTimeout,
    ResetTimeout,
    NvmLoadTimeout,
    NvmOwnershipTimeout,
    NvmReadTimeout,
    NoStationAddress,
}

impl InitError {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MasterDrainTimeout => "e1000: bus-master requests did not drain",
            Self::ResetTimeout => "e1000: CTRL.RST did not clear",
            Self::NvmLoadTimeout => "e1000: the NVM auto-read did not finish",
            Self::NvmOwnershipTimeout => "e1000: the NVM semaphore was not granted",
            Self::NvmReadTimeout => "e1000: EERD read did not complete",
            Self::NoStationAddress => "e1000: no valid station address",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxError {
    Empty,
    TooLong,
    RingFull,
}

impl TxError {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "e1000: empty frame",
            Self::TooLong => "e1000: frame longer than the MAC accepts",
            Self::RingFull => "e1000: transmit ring full",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RxError {
    NoFrame,
    BufferTooSmall,
}

impl RxError {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoFrame => "e1000: no received frame",
            Self::BufferTooSmall => "e1000: receive buffer smaller than the frame",
        }
    }
}

/// Whether the MAC reports link (STATUS.LU, SDM 13.4.2; DS 10.2.2.2).
pub fn link_up(regs: &mut impl Registers) -> bool {
    regs.read(STATUS) & STATUS_LU != 0
}

/// Read and clear the pending interrupt causes. The 8254x clears ICR on read
/// (SDM 13.4.17). The 82574 clears on read only in some states and always on
/// a write of 1b (DS 10.2.4.1), so the causes read are written back.
pub fn acknowledge_interrupt(regs: &mut impl Registers, model: Model) -> u32 {
    let causes = regs.read(ICR);
    if model == Model::Pcie82574 && causes != 0 {
        regs.write(ICR, causes);
    }
    causes
}

/// Unmask the causes in [`INTERRUPT_MASK`] (SDM 13.4.20; DS 4.6.1).
#[cfg(any(target_arch = "x86_64", test))]
pub fn enable_interrupts(regs: &mut impl Registers) {
    regs.write(IMS, INTERRUPT_MASK);
}

/// Whether the causes call for the receive bottom half.
#[cfg(any(target_arch = "x86_64", test))]
pub fn needs_receive_work(causes: u32) -> bool {
    causes & INTERRUPT_MASK != 0
}

/// The software side of both rings once the device is running.
pub struct Nic {
    mac: [u8; 6],
    /// Next receive descriptor the device fills.
    rx_next: usize,
    /// Dropping the rest of a frame that spanned descriptors.
    rx_discarding: bool,
    /// Next transmit descriptor software fills (the value written to TDT).
    tx_tail: usize,
    /// Oldest transmit descriptor not yet seen done.
    tx_clean: usize,
}

impl Nic {
    /// Reset and configure the function, with every interrupt masked. Call
    /// [`enable_interrupts`] afterwards to take interrupts.
    pub fn bring_up(hw: &mut impl Rings, model: Model) -> Result<Self, InitError> {
        reset(hw, model)?;
        configure_general(hw, model);
        let mac = station_address(hw, model)?;
        program_receive_addresses(hw, model, mac);
        initialize_receive(hw);
        initialize_transmit(hw, model);
        Ok(Self {
            mac,
            rx_next: 0,
            rx_discarding: false,
            tx_tail: 0,
            tx_clean: 0,
        })
    }

    pub fn mac(&self) -> [u8; 6] {
        self.mac
    }

    /// Whether the next receive descriptor holds a completed buffer.
    pub fn can_receive(&self, hw: &mut impl Rings) -> bool {
        RxDescriptor::from_words(hw.rx_descriptor(self.rx_next)).status & RXD_STATUS_DD != 0
    }

    /// Call once the interrupt route (MSI, or the INTx line) is live and
    /// unmasked. A cause raised before then sent no message, and an MSI is
    /// sent only as an unmasked cause first appears, so the pending causes are
    /// acknowledged here. Returns whether the receive bottom half has frames
    /// to drain: the causes asked for it, or the ring already holds one.
    #[cfg(any(target_arch = "x86_64", test))]
    pub fn interrupts_routed(&self, hw: &mut impl Rings, model: Model) -> bool {
        let causes = acknowledge_interrupt(hw, model);
        needs_receive_work(causes) || self.can_receive(hw)
    }

    /// Copy the next good frame into `out` and return its length. Frames the
    /// device marked in error, and frames that spanned more than one buffer,
    /// are dropped (SDM 3.2.3.1: only the EOP descriptor carries status and
    /// errors). Every descriptor examined goes back to the device.
    pub fn receive(&mut self, hw: &mut impl Rings, out: &mut [u8]) -> Result<usize, RxError> {
        for _ in 0..RX_RING {
            let index = self.rx_next;
            let descriptor = RxDescriptor::from_words(hw.rx_descriptor(index));
            if descriptor.status & RXD_STATUS_DD == 0 {
                return Err(RxError::NoFrame);
            }
            let end_of_packet = descriptor.status & RXD_STATUS_EOP != 0;
            let whole = !self.rx_discarding && end_of_packet && descriptor.errors == 0;
            let length = usize::from(descriptor.length);
            let result = if !whole {
                None
            } else if length > out.len() {
                Some(Err(RxError::BufferTooSmall))
            } else {
                hw.read_rx_buffer(index, &mut out[..length]);
                Some(Ok(length))
            };
            self.rx_discarding = !end_of_packet;
            self.return_rx_descriptor(hw, index);
            if let Some(result) = result {
                return result;
            }
        }
        Err(RxError::NoFrame)
    }

    /// Re-arm descriptor `index` and hand it back. RDT names one past the last
    /// descriptor the device may fill (SDM 3.2.6), so writing the index just
    /// consumed gives the device every descriptor up to the one before it.
    fn return_rx_descriptor(&mut self, hw: &mut impl Rings, index: usize) {
        let fresh = RxDescriptor {
            buffer: hw.rx_buffer_address(index),
            ..RxDescriptor::default()
        };
        hw.set_rx_descriptor(index, fresh.to_words());
        hw.write(RDT, index as u32);
        self.rx_next = (index + 1) % RX_RING;
    }

    /// Queue one frame. Each descriptor asks for status (RS), so its DD bit
    /// tells when the slot's buffer is free again (SDM 3.3.3, Table 3-10 note 2).
    pub fn transmit(&mut self, hw: &mut impl Rings, frame: &[u8]) -> Result<(), TxError> {
        if frame.is_empty() {
            return Err(TxError::Empty);
        }
        if frame.len() > MAX_FRAME {
            return Err(TxError::TooLong);
        }
        self.reclaim_transmitted(hw);
        let next = (self.tx_tail + 1) % TX_RING;
        if next == self.tx_clean {
            return Err(TxError::RingFull);
        }
        let index = self.tx_tail;
        hw.write_tx_buffer(index, frame);
        let descriptor = TxDescriptor {
            buffer: hw.tx_buffer_address(index),
            length: frame.len() as u16,
            cmd: TXD_CMD_EOP | TXD_CMD_IFCS | TXD_CMD_RS,
            ..TxDescriptor::default()
        };
        hw.set_tx_descriptor(index, descriptor.to_words());
        self.tx_tail = next;
        hw.write(TDT, next as u32);
        Ok(())
    }

    /// Advance past every descriptor the device has finished.
    fn reclaim_transmitted(&mut self, hw: &mut impl Rings) {
        while self.tx_clean != self.tx_tail {
            let descriptor = TxDescriptor::from_words(hw.tx_descriptor(self.tx_clean));
            if descriptor.status & TXD_STATUS_DD == 0 {
                break;
            }
            self.tx_clean = (self.tx_clean + 1) % TX_RING;
        }
    }
}

/// Wait `POLL_US`, then test `done`, up to `polls` times.
fn poll<R: Registers>(regs: &mut R, polls: u32, mut done: impl FnMut(&mut R) -> bool) -> bool {
    for _ in 0..polls {
        regs.delay_us(POLL_US);
        if done(regs) {
            return true;
        }
    }
    false
}

/// Steps 1 and 2 of the bring-up.
///
/// Stopping receive and transmit before the reset: on the 8254x by clearing
/// RCTL.EN and TCTL.EN. On the 82574 by the master-disable handshake, the
/// procedure DS 10.2.2.1 names for this; TCTL is not touched, because firmware
/// may leave its queue with TDH != TDT and DS 10.2.6.1 (MULR) forbids
/// modifying TCTL then. DS 3.1.3.10 lets the drain time out. On a timeout the
/// function is refused without a reset: DS 10.2.2.1 requires TX and RX
/// stopped first, and the master disable, which a reset would clear, stays
/// set and keeps blocking new requests.
fn reset<R: Registers>(regs: &mut R, model: Model) -> Result<(), InitError> {
    regs.write(IMC, model.all_causes());
    match model {
        Model::Pci8254x => {
            let rctl = regs.read(RCTL);
            regs.write(RCTL, rctl & !RCTL_EN);
            let tctl = regs.read(TCTL);
            regs.write(TCTL, tctl & !TCTL_EN);
        }
        Model::Pcie82574 => {
            let ctrl = regs.read(CTRL);
            regs.write(CTRL, ctrl | CTRL_GIO_MASTER_DISABLE);
            if !poll(regs, MASTER_DISABLE_POLLS, |regs| {
                regs.read(STATUS) & STATUS_GIO_MASTER_ENABLE == 0
            }) {
                return Err(InitError::MasterDrainTimeout);
            }
        }
    }
    let ctrl = regs.read(CTRL);
    regs.write(CTRL, ctrl | CTRL_RST);
    if !poll(regs, RESET_POLLS, |regs| regs.read(CTRL) & CTRL_RST == 0) {
        return Err(InitError::ResetTimeout);
    }
    regs.write(IMC, model.all_causes());
    acknowledge_interrupt(regs, model);
    if model == Model::Pcie82574 {
        // The reset reloads RAL0/RAH0, CTRL fields and others from the NVM
        // (DS 4.2, 6.1.1); EEC.Auto_RD reports the load done, and is set also
        // with no valid NVM (DS 10.2.2.3). Configuring before then would let
        // a late load rewrite CTRL, so a load that does not finish refuses
        // the function.
        if !poll(regs, NVM_POLLS, |regs| regs.read(EEC) & EEC_AUTO_RD != 0) {
            return Err(InitError::NvmLoadTimeout);
        }
    }
    Ok(())
}

/// Step 3: link up with the MAC following the PHY's resolved speed and duplex
/// (SDM 14.3: ASDE with SLU; DS 4.6.3.2's first mechanism: FRCSPD, FRCDPLX and
/// ASDE clear), no forced settings, no VLAN mode, flow control off.
fn configure_general(regs: &mut impl Registers, model: Model) {
    let mut ctrl = regs.read(CTRL);
    ctrl &= !(CTRL_FRCSPD
        | CTRL_FRCDPLX
        | CTRL_ILOS
        | CTRL_VME
        | CTRL_PHY_RST
        | CTRL_RFCE
        | CTRL_TFCE
        | CTRL_GIO_MASTER_DISABLE);
    ctrl |= CTRL_SLU;
    match model {
        // LRST clear (SDM 14.3).
        Model::Pci8254x => ctrl = (ctrl | CTRL_ASDE) & !CTRL_BIT3,
        // Bit 3 is reserved and set (DS 10.2.2.1).
        Model::Pcie82574 => ctrl = (ctrl & !CTRL_ASDE) | CTRL_BIT3,
    }
    regs.write(CTRL, ctrl);
    // No flow control: these are written with zero (SDM 14.3; DS 4.6.2).
    for offset in [FCAL, FCAH, FCT, FCTTV] {
        regs.write(offset, 0);
    }
    if model == Model::Pcie82574 {
        let gcr = regs.read(GCR);
        regs.write(GCR, gcr | GCR_BIT22);
        let gcr2 = regs.read(GCR2);
        regs.write(GCR2, gcr2 | GCR2_BIT0);
    }
}

/// The station address: RAL0/RAH0 when an NVM load left it valid (SDM 5.6.1;
/// DS 6.1.1.1), otherwise NVM words 0-2 through EERD. On the 82574 `reset`
/// has already waited for EEC.Auto_RD. On the 8254x a global reset clears
/// RAH0's valid bit (SDM 14.7), so EERD is the usual path there.
fn station_address<R: Registers>(regs: &mut R, model: Model) -> Result<[u8; 6], InitError> {
    let high = regs.read(RAH0);
    if high & RAH_AV != 0 {
        let mac = address_from_registers(regs.read(RAL0), high);
        if is_station_address(mac) {
            return Ok(mac);
        }
    }
    let words = read_nvm_words(regs, model)?;
    let mut mac = [0; 6];
    for (pair, word) in mac.chunks_exact_mut(2).zip(words) {
        pair.copy_from_slice(&word.to_le_bytes());
    }
    if is_station_address(mac) {
        Ok(mac)
    } else {
        Err(InitError::NoStationAddress)
    }
}

/// NVM words 0-2. Each holds two address bytes, the first byte in the low half
/// (SDM 5.6.1: 12-34-56-78-90-AB is stored as 3412, 7856, AB90). On the 82574
/// software holds the NVM semaphore around EERD (DS 4.5.2): a request
/// registers only when written while no other agent holds it, so it is
/// written again until it reads back granted. Without the grant the read is
/// not attempted; with it, it is released afterwards.
fn read_nvm_words<R: Registers>(regs: &mut R, model: Model) -> Result<[u16; 3], InitError> {
    let (done, shift) = match model {
        Model::Pci8254x => (EERD_8254X_DONE, EERD_8254X_ADDR_SHIFT),
        Model::Pcie82574 => (EERD_82574_DONE, EERD_82574_ADDR_SHIFT),
    };
    if model == Model::Pcie82574
        && !poll(regs, NVM_POLLS, |regs| {
            let extcnf = regs.read(EXTCNF_CTRL);
            regs.write(EXTCNF_CTRL, extcnf | EXTCNF_CTRL_SW_OWNERSHIP);
            regs.read(EXTCNF_CTRL) & EXTCNF_CTRL_SW_OWNERSHIP != 0
        })
    {
        return Err(InitError::NvmOwnershipTimeout);
    }
    let mut words = [0u16; 3];
    let mut result = Ok(());
    for (address, word) in words.iter_mut().enumerate() {
        regs.write(EERD, EERD_START | ((address as u32) << shift));
        let mut value = 0;
        if !poll(regs, NVM_POLLS, |regs| {
            value = regs.read(EERD);
            value & done != 0
        }) {
            result = Err(InitError::NvmReadTimeout);
            break;
        }
        *word = (value >> EERD_DATA_SHIFT) as u16;
    }
    if model == Model::Pcie82574 {
        let extcnf = regs.read(EXTCNF_CTRL);
        regs.write(EXTCNF_CTRL, extcnf & !EXTCNF_CTRL_SW_OWNERSHIP);
    }
    result.map(|()| words)
}

/// RAL holds address bytes 0-3 and RAH bits 15:0 bytes 4-5, first byte lowest:
/// the NVM words load into them in order (SDM 5.6.1, 13.5.2-13.5.3).
fn address_from_registers(low: u32, high: u32) -> [u8; 6] {
    let low = low.to_le_bytes();
    let high = high.to_le_bytes();
    [low[0], low[1], low[2], low[3], high[0], high[1]]
}

/// A unicast address that is not all zeros.
fn is_station_address(mac: [u8; 6]) -> bool {
    mac != [0; 6] && mac[0] & 1 == 0
}

/// Step 4. RAL0 then RAH0 with AV; the other entries the driver owns cleared
/// high before low; the multicast table zeroed (SDM 13.5.3 note, 14.4; DS
/// 4.6.5, 10.2.5.23).
fn program_receive_addresses(regs: &mut impl Registers, model: Model, mac: [u8; 6]) {
    let low = u32::from_le_bytes([mac[0], mac[1], mac[2], mac[3]]);
    let high = u32::from(u16::from_le_bytes([mac[4], mac[5]]));
    regs.write(RAL0, low);
    regs.write(RAH0, high | RAH_AV);
    for entry in 1..model.receive_addresses() {
        regs.write(RAH0 + 8 * entry, 0);
        regs.write(RAL0 + 8 * entry, 0);
    }
    for entry in 0..MTA_ENTRIES {
        regs.write(MTA + 4 * entry, 0);
    }
}

/// Step 5. Every descriptor gets its buffer; head 0 and tail at the last
/// descriptor hand all but one to the device (SDM 14.4). Then RCTL: broadcast
/// accepted, CRC stripped, 2048-byte buffers, legacy descriptors, no loopback,
/// promiscuity, long packets or VLAN filter, and the receiver enabled last.
fn initialize_receive(hw: &mut impl Rings) {
    for index in 0..RX_RING {
        let descriptor = RxDescriptor {
            buffer: hw.rx_buffer_address(index),
            ..RxDescriptor::default()
        };
        hw.set_rx_descriptor(index, descriptor.to_words());
    }
    let ring = hw.rx_ring_address();
    hw.write(RDBAL, ring as u32);
    hw.write(RDBAH, (ring >> 32) as u32);
    hw.write(RDLEN, (RX_RING * DESCRIPTOR_BYTES) as u32);
    hw.write(RDH, 0);
    hw.write(RDT, (RX_RING - 1) as u32);
    let cleared = RCTL_EN
        | RCTL_SBP
        | RCTL_UPE
        | RCTL_MPE
        | RCTL_LPE
        | RCTL_LBM_MASK
        | RCTL_RDMTS_MASK
        | RCTL_DTYP_MASK
        | RCTL_MO_MASK
        | RCTL_BSIZE_MASK
        | RCTL_VFE
        | RCTL_BSEX;
    let rctl = (hw.read(RCTL) & !cleared) | RCTL_BAM | RCTL_SECRC;
    hw.write(RCTL, rctl);
    hw.write(RCTL, rctl | RCTL_EN);
}

/// Step 6. Descriptors start empty with their buffers attached; head and tail
/// at 0 (SDM 14.5). On the 82574, TXDCTL writes back per descriptor (DS
/// 4.6.6, with the reserved bit 22 set per DS 10.2.6.11). TCTL keeps the
/// fields it is not told about and is enabled last with padding of short
/// frames and the model's full-duplex collision fields.
fn initialize_transmit(hw: &mut impl Rings, model: Model) {
    for index in 0..TX_RING {
        let descriptor = TxDescriptor {
            buffer: hw.tx_buffer_address(index),
            ..TxDescriptor::default()
        };
        hw.set_tx_descriptor(index, descriptor.to_words());
    }
    let ring = hw.tx_ring_address();
    hw.write(TDBAL, ring as u32);
    hw.write(TDBAH, (ring >> 32) as u32);
    hw.write(TDLEN, (TX_RING * DESCRIPTOR_BYTES) as u32);
    hw.write(TDH, 0);
    hw.write(TDT, 0);
    if model == Model::Pcie82574 {
        hw.write(TXDCTL, TXDCTL_GRAN_DESCRIPTORS | (1 << TXDCTL_WTHRESH_SHIFT) | TXDCTL_BIT22);
    }
    hw.write(TIPG, model.tipg());
    let tctl = hw.read(TCTL) & !(TCTL_EN | TCTL_CT_MASK | TCTL_COLD_MASK);
    hw.write(TCTL, tctl | TCTL_EN | TCTL_PSP | model.collision_fields());
}
