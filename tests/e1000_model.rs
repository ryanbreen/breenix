//! The e1000 driver's MAC programming (`kernel/src/drivers/e1000/nic.rs`),
//! executed against a model of the two Intel manuals it is written from:
//! the 8254x Software Developer's Manual ("SDM", 82540EM) and the 82574
//! datasheet ("DS").
//!
//! The model takes its register offsets, bits and descriptor layouts from the
//! manuals itself (module `hw`), not from the driver's `regs.rs`, so a wrong
//! constant in the driver shows up as a disagreement. It implements the
//! behaviour the manuals specify: CTRL.RST self-clearing after about 1 us; the
//! 82574 master-disable handshake, NVM auto-read (EEC.Auto_RD) and the NVM
//! semaphore with a hardware owner; both EERD layouts; ICR clear-on-read and
//! write-1-to-clear, with the 82574's INT_ASSERTED and its read-side rules;
//! the receive-address entries a reset clears and the one the 82574 reserves
//! for manageability; and descriptor-ring DMA. Each rule a "must" in the
//! manuals states is a recorded violation when the driver breaks it.

#[path = "../kernel/src/drivers/e1000/regs.rs"]
mod regs;
#[path = "../kernel/src/drivers/e1000/nic.rs"]
mod nic;

use std::collections::HashMap;

use nic::{InitError, Model, Nic, Registers, Rings, RxError, TxError};

/// Offsets and bits as the manuals give them.
mod hw {
    pub const CTRL: u32 = 0x0000;
    pub const STATUS: u32 = 0x0008;
    pub const EEC: u32 = 0x0010;
    pub const EERD: u32 = 0x0014;
    pub const FCAL: u32 = 0x0028;
    pub const FCAH: u32 = 0x002C;
    pub const FCT: u32 = 0x0030;
    pub const FCTTV: u32 = 0x0170;
    pub const ICR: u32 = 0x00C0;
    pub const IMS: u32 = 0x00D0;
    pub const IMC: u32 = 0x00D8;
    pub const RCTL: u32 = 0x0100;
    pub const TCTL: u32 = 0x0400;
    pub const TIPG: u32 = 0x0410;
    pub const EXTCNF_CTRL: u32 = 0x0F00;
    pub const RDBAL: u32 = 0x2800;
    pub const RDBAH: u32 = 0x2804;
    pub const RDLEN: u32 = 0x2808;
    pub const RDH: u32 = 0x2810;
    pub const RDT: u32 = 0x2818;
    pub const TDBAL: u32 = 0x3800;
    pub const TDBAH: u32 = 0x3804;
    pub const TDLEN: u32 = 0x3808;
    pub const TDH: u32 = 0x3810;
    pub const TDT: u32 = 0x3818;
    pub const TXDCTL: u32 = 0x3828;
    pub const MTA: u32 = 0x5200;
    pub const RAL: u32 = 0x5400;
    pub const RAH: u32 = 0x5404;
    pub const GCR: u32 = 0x5B00;
    pub const GCR2: u32 = 0x5B64;

    pub const CTRL_GIO_MASTER_DISABLE: u32 = 1 << 2;
    pub const CTRL_BIT3: u32 = 1 << 3;
    pub const CTRL_ASDE: u32 = 1 << 5;
    pub const CTRL_SLU: u32 = 1 << 6;
    pub const CTRL_ILOS: u32 = 1 << 7;
    pub const CTRL_FRCSPD: u32 = 1 << 11;
    pub const CTRL_FRCDPLX: u32 = 1 << 12;
    pub const CTRL_RST: u32 = 1 << 26;
    pub const CTRL_RFCE: u32 = 1 << 27;
    pub const CTRL_TFCE: u32 = 1 << 28;
    pub const CTRL_VME: u32 = 1 << 30;
    pub const CTRL_PHY_RST: u32 = 1 << 31;
    pub const STATUS_LU: u32 = 1 << 1;
    pub const STATUS_GIO_MASTER_ENABLE: u32 = 1 << 19;
    pub const EEC_AUTO_RD: u32 = 1 << 9;
    pub const EXTCNF_SW: u32 = 1 << 5;
    pub const EXTCNF_HW: u32 = 1 << 6;
    pub const INT_TXDW: u32 = 1 << 0;
    pub const INT_LSC: u32 = 1 << 2;
    pub const INT_RXDMT0: u32 = 1 << 4;
    pub const INT_RXO: u32 = 1 << 6;
    pub const INT_RXT0: u32 = 1 << 7;
    pub const INT_ASSERTED: u32 = 1 << 31;
    pub const RCTL_EN: u32 = 1 << 1;
    pub const RCTL_BAM: u32 = 1 << 15;
    pub const RCTL_SECRC: u32 = 1 << 26;
    pub const TCTL_EN: u32 = 1 << 1;
    pub const TCTL_PSP: u32 = 1 << 3;
    pub const TCTL_MULR: u32 = 1 << 28;
    pub const TCTL_RRTHRESH_01: u32 = 0b01 << 29;
    pub const RAH_AV: u32 = 1 << 31;
    pub const GCR_BIT22: u32 = 1 << 22;
    pub const GCR2_BIT0: u32 = 1 << 0;
    pub const RXD_DD: u8 = 1 << 0;
    pub const RXD_EOP: u8 = 1 << 1;
    pub const TXD_DD: u8 = 1 << 0;
    pub const TXD_CMD_EOP_IFCS_RS: u8 = 0b1011;

    /// MDI Control (SDM 13.4.7 Table 13-14; DS 10.2.2.7).
    pub const MDIC: u32 = 0x0020;
    pub const MDIC_REG_SHIFT: u32 = 16;
    pub const MDIC_PHY_SHIFT: u32 = 21;
    pub const MDIC_OP_WRITE: u32 = 0b01 << 26;
    pub const MDIC_OP_READ: u32 = 0b10 << 26;
    pub const MDIC_READY: u32 = 1 << 28;
    pub const MDIC_ERROR: u32 = 1 << 30;
    /// The internal PHY's MDI address (SDM 13.4.7; DS 10.2.2.7).
    pub const PHY_ADDRESS: u32 = 1;
    /// PHY register 0 (SDM 13.4.7.1.1 Table 13-16; DS 10.2.11.1).
    pub const PHY_RESET: u16 = 1 << 15;
    pub const PHY_LOOPBACK: u16 = 1 << 14;
    pub const PHY_AN_ENABLE: u16 = 1 << 12;
    pub const PHY_POWER_DOWN: u16 = 1 << 11;
    pub const PHY_ISOLATE: u16 = 1 << 10;
    pub const PHY_RESTART_AN: u16 = 1 << 9;
    /// The hardware-reset value: auto-negotiation enabled, full duplex, speed
    /// bits 10b (SDM Table 13-16; DS 10.2.11.1).
    pub const PHY_CONTROL_DEFAULT: u16 = PHY_AN_ENABLE | (1 << 8) | (1 << 6);
}

use hw::*;

const RX_RING_ADDRESS: u64 = 0x1_0000_0000;
const TX_RING_ADDRESS: u64 = 0x1_0000_0200;
const RX_BUFFERS: u64 = 0x2_0000_0000;
const TX_BUFFERS: u64 = 0x3_0000_0000;
/// The SDM 5.6.1 example address 12-34-56-78-90-AB as stored in NVM words 0-2.
const NVM_WORDS: [u16; 3] = [0x3412, 0x7856, 0xAB90];
const NVM_MAC: [u8; 6] = [0x12, 0x34, 0x56, 0x78, 0x90, 0xAB];
/// A filter manageability firmware keeps in 82574 entry 15 (DS 10.2.5.23).
const FIRMWARE_ENTRY: (u32, u32) = (0x0102_0304, 0x0506 | RAH_AV);
/// How long the 82574's NVM auto-read takes after a reset, in the model.
const AUTO_READ_NS: u64 = 50_000;
/// One MDI cycle, and one auto-negotiation, in the model.
const MDI_CYCLE_NS: u64 = 30_000;
const NEGOTIATION_NS: u64 = 2_000_000_000;

fn rx_words(words: [u64; 2]) -> (u64, u16, u8, u8) {
    let high = words[1];
    (words[0], high as u16, (high >> 32) as u8, (high >> 40) as u8)
}

fn rx_written(buffer: u64, length: u16, status: u8, errors: u8) -> [u64; 2] {
    [buffer, u64::from(length) | (u64::from(status) << 32) | (u64::from(errors) << 40)]
}

fn tx_words(words: [u64; 2]) -> (u64, u16, u8, u8) {
    let high = words[1];
    (words[0], high as u16, (high >> 24) as u8, (high >> 32) as u8)
}

struct Device {
    model: Model,
    registers: HashMap<u32, u32>,
    /// Each register write, in order: (offset, value).
    log: Vec<(u32, u32)>,
    now_ns: u64,
    reset_at: Option<u64>,
    master_idle_at: Option<u64>,
    auto_read_at: Option<u64>,
    /// Whether the 82574's auto-read loads RAL0/RAH0 from the NVM.
    loads_address: bool,
    /// Until when hardware owns the NVM semaphore; `u64::MAX` for the whole run.
    hardware_owns_nvm_until: u64,
    /// An 82574 whose master requests stay pending, or whose NVM auto-read
    /// does not finish.
    master_stuck: bool,
    auto_read_stuck: bool,
    nvm: [u16; 3],
    rx_ring: Vec<[u64; 2]>,
    tx_ring: Vec<[u64; 2]>,
    rx_buffers: Vec<Vec<u8>>,
    tx_buffers: Vec<Vec<u8>>,
    sent: Vec<(Vec<u8>, u8)>,
    transmit_paused: bool,
    missed: usize,
    /// The PHY's control register. CTRL.RST does not reach the PHY (SDM 14.7:
    /// "PHY: No effect"; DS 4.2 Table 29), so it keeps what firmware left.
    phy_control: u16,
    /// When the PHY's negotiation with the link partner completes; `None`
    /// while it is not negotiating. The partner only auto-negotiates.
    negotiated_at: Option<u64>,
    /// When the MDI cycle in flight completes, with the data a read returns.
    mdi_done_at: Option<(u64, u16)>,
    /// A PHY that never completes an MDI cycle.
    mdi_stuck: bool,
    /// Each PHY control value written through MDIC.
    phy_control_writes: Vec<u16>,
    violations: Vec<String>,
}

impl Device {
    fn new(model: Model, loads_address: bool) -> Self {
        let mut device = Self {
            model,
            registers: HashMap::new(),
            log: Vec::new(),
            now_ns: 0,
            reset_at: None,
            master_idle_at: None,
            auto_read_at: None,
            loads_address,
            hardware_owns_nvm_until: 0,
            master_stuck: false,
            auto_read_stuck: false,
            nvm: NVM_WORDS,
            rx_ring: vec![[0; 2]; nic::RX_RING],
            tx_ring: vec![[0; 2]; nic::TX_RING],
            rx_buffers: vec![vec![0; nic::BUFFER_SIZE]; nic::RX_RING],
            tx_buffers: vec![vec![0; nic::BUFFER_SIZE]; nic::TX_RING],
            sent: Vec::new(),
            transmit_paused: false,
            missed: 0,
            phy_control: PHY_CONTROL_DEFAULT,
            negotiated_at: Some(0),
            mdi_done_at: None,
            mdi_stuck: false,
            phy_control_writes: Vec::new(),
            violations: Vec::new(),
        };
        device.power_on();
        device.finish_auto_read();
        // Firmware left the port running with a transmit queue in flight, as
        // a boot loader may, and manageability firmware owns entry 15.
        let tctl = device.reg(TCTL) | TCTL_EN;
        device.set(RCTL, RCTL_EN);
        device.set(TCTL, tctl);
        device.set(IMS, INT_RXT0);
        device.set(TDH, 3);
        device.set(TDT, 5);
        device.set(RAL + 8 * 15, FIRMWARE_ENTRY.0);
        device.set(RAH + 8 * 15, FIRMWARE_ENTRY.1);
        device
    }

    fn reg(&self, offset: u32) -> u32 {
        self.registers.get(&offset).copied().unwrap_or(0)
    }

    fn set(&mut self, offset: u32, value: u32) {
        self.registers.insert(offset, value);
    }

    fn violation(&mut self, text: impl Into<String>) {
        self.violations.push(text.into());
    }

    /// Power-on values (SDM 13.4.1, 13.4.33; DS 10.2.2.1, 10.2.6.1). A reset
    /// clears the receive-address valid bits it covers (SDM 14.7: each
    /// entry; DS 10.2.5.23: entries 0-14, not 15) but no other address bits.
    fn power_on(&mut self) {
        let addresses: Vec<(u32, u32)> = (0..16)
            .flat_map(|entry| [RAL + 8 * entry, RAH + 8 * entry])
            .map(|offset| (offset, self.reg(offset)))
            .collect();
        self.registers.clear();
        // STATUS.LU follows the PHY (`settle`).
        let (ctrl, tctl, status) = match self.model {
            Model::Pci8254x => (CTRL_BIT3 | (0b10 << 8) | CTRL_FRCSPD, 0, 0),
            Model::Pcie82574 => (
                1 | CTRL_BIT3 | (0b10 << 8) | (1 << 20),
                TCTL_PSP | TCTL_MULR | TCTL_RRTHRESH_01,
                STATUS_GIO_MASTER_ENABLE,
            ),
        };
        self.set(CTRL, ctrl);
        self.set(TCTL, tctl);
        self.set(STATUS, status);
        // MDIC.R: 1b on the 82574, 0b on the 8254x after reset.
        if self.model == Model::Pcie82574 {
            self.set(MDIC, MDIC_READY);
        }
        if self.model == Model::Pcie82574 {
            self.set(EXTCNF_CTRL, 1 << 3);
        }
        let kept_entries = if self.model == Model::Pcie82574 { 15 } else { 16 };
        for (offset, value) in addresses {
            let entry = (offset - RAL) / 8;
            let is_high = (offset - RAL) % 8 == 4;
            let value = if is_high && entry < kept_entries { value & !RAH_AV } else { value };
            self.set(offset, value);
        }
    }

    /// DS 10.2.2.3: EEC.Auto_RD sets when the hardware's NVM read is done,
    /// which loads RAL0/RAH0 (DS 6.1.1.1). The 8254x model has no auto-read
    /// bit (SDM 13.4.3) and a global reset leaves RAH0 invalid (SDM 14.7).
    fn finish_auto_read(&mut self) {
        self.auto_read_at = None;
        if self.model == Model::Pcie82574 {
            let eec = self.reg(EEC) | EEC_AUTO_RD;
            self.set(EEC, eec);
            if self.loads_address {
                let [a, b, c] = self.nvm;
                self.set(RAL, u32::from(a) | (u32::from(b) << 16));
                self.set(RAH, u32::from(c) | RAH_AV);
            }
        }
    }

    fn settle(&mut self) {
        if let Some(at) = self.reset_at {
            if self.now_ns < at + 1000 {
                // SDM 13.4.1, DS 10.2.2.1: wait about 1 us after setting RST
                // before any register access.
                self.violation("register accessed within 1 us of CTRL.RST");
            } else {
                self.reset_at = None;
                self.master_idle_at = None;
                self.power_on();
                if self.model == Model::Pcie82574 && !self.auto_read_stuck {
                    self.auto_read_at = Some(at + AUTO_READ_NS);
                }
            }
        }
        if let Some(at) = self.auto_read_at {
            if self.now_ns >= at {
                self.finish_auto_read();
            }
        }
        if let Some(at) = self.master_idle_at {
            if self.now_ns >= at {
                let status = self.reg(STATUS) & !STATUS_GIO_MASTER_ENABLE;
                self.set(STATUS, status);
            }
        }
        if self.model == Model::Pcie82574 {
            let extcnf = self.reg(EXTCNF_CTRL);
            let hardware = if self.now_ns < self.hardware_owns_nvm_until { EXTCNF_HW } else { 0 };
            self.set(EXTCNF_CTRL, (extcnf & !EXTCNF_HW) | hardware);
        }
        if let Some((at, data)) = self.mdi_done_at {
            if self.now_ns >= at {
                self.mdi_done_at = None;
                let mdic = (self.reg(MDIC) & !0xFFFF) | u32::from(data) | MDIC_READY;
                self.set(MDIC, mdic);
            }
        }
        // The MAC sees link once the PHY has negotiated (SDM 13.4.2; DS
        // 4.6.3.2: with CTRL.SLU set, which `link_up` callers rely on).
        let linked = self.negotiated_at.is_some_and(|at| self.now_ns >= at);
        let status = (self.reg(STATUS) & !STATUS_LU) | if linked { STATUS_LU } else { 0 };
        self.set(STATUS, status);
    }

    /// A PHY control write. Auto-negotiation enable, like the speed and
    /// duplex bits, takes effect only with a PHY reset, a restart of
    /// auto-negotiation or a return from power down (SDM 11.1.3; DS
    /// 10.2.11.1). Negotiation runs while enabled, powered, not isolated and
    /// not looped back.
    fn write_phy_control(&mut self, value: u16) {
        self.phy_control_writes.push(value);
        let previous = self.phy_control;
        let applies = value & (PHY_RESET | PHY_RESTART_AN) != 0
            || (previous & PHY_POWER_DOWN != 0 && value & PHY_POWER_DOWN == 0);
        let mut next = value & !(PHY_RESET | PHY_RESTART_AN);
        if !applies {
            next = (next & !PHY_AN_ENABLE) | (previous & PHY_AN_ENABLE);
        }
        self.phy_control = next;
        let blocked = PHY_POWER_DOWN | PHY_ISOLATE | PHY_LOOPBACK;
        if next & PHY_AN_ENABLE == 0 || next & blocked != 0 {
            self.negotiated_at = None;
        } else if applies {
            self.negotiated_at = Some(self.now_ns + NEGOTIATION_NS);
        }
    }

    /// An MDIC command (SDM 13.4.7; DS 10.2.2.7). Only PHY address 1 answers,
    /// and only register 0 is modelled; another register reads as 0.
    fn mdi_command(&mut self, value: u32) {
        if value & (1 << 31) != 0 {
            self.violation("MDIC reserved bit 31 written as 1b");
        }
        if self.mdi_done_at.is_some() {
            self.violation("MDIC command written before the last one was Ready");
        }
        if self.model == Model::Pcie82574 && self.reg(EXTCNF_CTRL) & EXTCNF_HW != 0 {
            self.violation("MDIC command while hardware owns MDIO (DS 4.5.2)");
        }
        if value & MDIC_READY != 0 {
            self.violation("MDIC command written with Ready set");
        }
        self.set(MDIC, value & !(MDIC_READY | MDIC_ERROR));
        if self.mdi_stuck {
            return;
        }
        let phy = (value >> MDIC_PHY_SHIFT) & 0x1F;
        let register = (value >> MDIC_REG_SHIFT) & 0x1F;
        let op = value & (0b11 << 26);
        if phy != PHY_ADDRESS {
            let mdic = self.reg(MDIC) | MDIC_ERROR;
            self.set(MDIC, mdic);
            self.mdi_done_at = Some((self.now_ns + MDI_CYCLE_NS, 0));
            return;
        }
        let data = match op {
            MDIC_OP_READ if register == 0 => self.phy_control,
            MDIC_OP_READ => 0,
            MDIC_OP_WRITE => {
                if register == 0 {
                    self.write_phy_control(value as u16);
                }
                value as u16
            }
            _ => {
                self.violation("MDIC reserved op-code");
                0
            }
        };
        self.mdi_done_at = Some((self.now_ns + MDI_CYCLE_NS, data));
    }

    fn raise(&mut self, causes: u32) {
        let icr = self.reg(ICR) | causes;
        self.set(ICR, icr);
    }

    fn rx_index(&self, address: u64) -> Option<usize> {
        let offset = address.checked_sub(RX_BUFFERS)? as usize;
        (offset % nic::BUFFER_SIZE == 0 && offset / nic::BUFFER_SIZE < nic::RX_RING)
            .then_some(offset / nic::BUFFER_SIZE)
    }

    fn tx_index(&self, address: u64) -> Option<usize> {
        let offset = address.checked_sub(TX_BUFFERS)? as usize;
        (offset % nic::BUFFER_SIZE == 0 && offset / nic::BUFFER_SIZE < nic::TX_RING)
            .then_some(offset / nic::BUFFER_SIZE)
    }

    /// The receive DMA engine (SDM 3.2.4-3.2.6) writing one buffer's worth at
    /// RDH: data, length, DD, EOP on the last piece of a frame and the frame's
    /// errors there. Returns false when head has reached tail.
    fn fill(&mut self, piece: &[u8], last: bool, errors: u8) -> bool {
        if self.reg(RCTL) & RCTL_EN == 0 {
            return false;
        }
        let head = self.reg(RDH) as usize;
        if head == self.reg(RDT) as usize {
            self.missed += 1;
            self.raise(INT_RXO);
            return false;
        }
        let (buffer, _, status, _) = rx_words(self.rx_ring[head]);
        let Some(slot) = self.rx_index(buffer) else {
            self.violation(format!("RX descriptor {head} names buffer {buffer:#x}"));
            return false;
        };
        if status != 0 {
            self.violation(format!("RX descriptor {head} handed to hardware with status set"));
        }
        self.rx_buffers[slot][..piece.len()].copy_from_slice(piece);
        let status = RXD_DD | if last { RXD_EOP } else { 0 };
        self.rx_ring[head] = rx_written(buffer, piece.len() as u16, status, if last { errors } else { 0 });
        self.set(RDH, ((head + 1) % nic::RX_RING) as u32);
        if last {
            self.raise(INT_RXT0);
        }
        true
    }

    fn deliver(&mut self, frame: &[u8], errors: u8) -> bool {
        let pieces: Vec<&[u8]> = frame.chunks(nic::BUFFER_SIZE).collect();
        let free = (self.reg(RDT) as usize + nic::RX_RING - self.reg(RDH) as usize) % nic::RX_RING;
        if free < pieces.len() {
            self.missed += 1;
            return false;
        }
        let count = pieces.len();
        pieces.iter().enumerate().all(|(index, piece)| self.fill(piece, index + 1 == count, errors))
    }

    /// The transmit DMA engine (SDM 3.3): each descriptor from TDH to TDT,
    /// writing DD back for those with RS.
    fn run_transmit(&mut self) {
        if self.transmit_paused || self.reg(TCTL) & TCTL_EN == 0 {
            return;
        }
        while self.reg(TDH) != self.reg(TDT) {
            let head = self.reg(TDH) as usize;
            let (buffer, length, cmd, status) = tx_words(self.tx_ring[head]);
            match self.tx_index(buffer) {
                Some(slot) if slot == head => {
                    let frame = self.tx_buffers[slot][..usize::from(length)].to_vec();
                    self.sent.push((frame, cmd));
                }
                _ => self.violation(format!("TX descriptor {head} names buffer {buffer:#x}")),
            }
            if cmd & 0b1000 != 0 {
                self.tx_ring[head][1] |= u64::from(status | TXD_DD) << 32;
            }
            self.set(TDH, ((head + 1) % nic::TX_RING) as u32);
        }
    }

    fn written(&self, offset: u32) -> Vec<(usize, u32)> {
        self.log
            .iter()
            .enumerate()
            .filter(|(_, (written, _))| *written == offset)
            .map(|(at, (_, value))| (at, *value))
            .collect()
    }

    fn first_write(&self, offset: u32, test: impl Fn(u32) -> bool) -> Option<usize> {
        self.written(offset).into_iter().find(|&(_, value)| test(value)).map(|(at, _)| at)
    }

    fn last_write(&self, offset: u32) -> Option<usize> {
        self.written(offset).last().map(|&(at, _)| at)
    }

    fn last_value(&self, offset: u32) -> Option<u32> {
        self.written(offset).last().map(|&(_, value)| value)
    }
}

impl Registers for Device {
    fn read(&mut self, offset: u32) -> u32 {
        self.settle();
        match offset {
            CTRL if self.reset_at.is_some() => self.reg(CTRL) | CTRL_RST,
            ICR => {
                let icr = self.reg(ICR);
                match self.model {
                    // SDM 13.4.17: reading clears the register.
                    Model::Pci8254x => {
                        self.set(ICR, 0);
                        icr
                    }
                    // DS 10.2.4.1: INT_ASSERTED reads set while an unmasked
                    // cause is pending. Reading clears only with each cause
                    // masked; otherwise it has no side effect (auto-mask is
                    // off: CTRL_EXT.IAME is 0 after reset).
                    Model::Pcie82574 => {
                        let asserted = if icr & self.reg(IMS) != 0 { INT_ASSERTED } else { 0 };
                        if self.reg(IMS) == 0 {
                            self.set(ICR, 0);
                        }
                        icr | asserted
                    }
                }
            }
            RAL | RAH if self.model == Model::Pcie82574 && self.reg(EEC) & EEC_AUTO_RD == 0 => {
                self.violation("RAL0/RAH0 read before EEC.Auto_RD (DS 10.2.2.3)");
                self.reg(offset)
            }
            _ => self.reg(offset),
        }
    }

    fn write(&mut self, offset: u32, value: u32) {
        self.settle();
        self.log.push((offset, value));
        let is_82574 = self.model == Model::Pcie82574;
        match offset {
            CTRL if value & CTRL_RST != 0 => {
                if is_82574 && self.reg(STATUS) & STATUS_GIO_MASTER_ENABLE != 0 {
                    self.violation("82574 reset with master requests not drained (DS 3.1.3.10)");
                }
                self.reset_at = Some(self.now_ns);
                self.set(CTRL, value & !CTRL_RST);
            }
            CTRL => {
                if is_82574 && value & CTRL_GIO_MASTER_DISABLE != 0 && !self.master_stuck {
                    self.master_idle_at = Some(self.now_ns + 20_000);
                }
                self.set(CTRL, value);
            }
            ICR => {
                // Write 1b to clear, on both (SDM 13.4.17; DS 10.2.4.1).
                let icr = self.reg(ICR) & !value;
                self.set(ICR, icr);
            }
            IMS => {
                let ims = self.reg(IMS) | value;
                self.set(IMS, ims);
            }
            IMC => {
                if is_82574 && value >> 25 != 0 {
                    self.violation("IMC reserved bits 31:25 written as 1b (DS 10.2.4.6)");
                }
                let ims = self.reg(IMS) & !value;
                self.set(IMS, ims);
            }
            EXTCNF_CTRL => {
                if !is_82574 {
                    self.violation("EXTCNF_CTRL written on the 8254x");
                }
                // DS 4.5.2: a request registers only while no other agent
                // holds the semaphore.
                let hardware = self.reg(EXTCNF_CTRL) & EXTCNF_HW;
                let mut next = (value & !EXTCNF_HW) | hardware;
                if hardware != 0 && self.reg(EXTCNF_CTRL) & EXTCNF_SW == 0 {
                    next &= !EXTCNF_SW;
                }
                self.set(EXTCNF_CTRL, next);
            }
            EERD if value & 1 != 0 => {
                let (done, address) = match self.model {
                    Model::Pci8254x => (1 << 4, (value >> 8) & 0xFF),
                    Model::Pcie82574 => (1 << 1, (value >> 2) & 0x3FFF),
                };
                if is_82574 && self.reg(EXTCNF_CTRL) & EXTCNF_SW == 0 {
                    self.violation("EERD read without NVM ownership (DS 4.5.2)");
                }
                let data = self.nvm.get(address as usize).copied().unwrap_or(0xFFFF);
                self.set(EERD, (u32::from(data) << 16) | (value & 0xFFFC) | done);
            }
            RCTL => {
                if value & RCTL_EN != 0
                    && (self.reg(RDLEN) as usize != nic::RX_RING * nic::DESCRIPTOR_BYTES
                        || self.reg(RDBAL) != RX_RING_ADDRESS as u32
                        || self.reg(RDBAH) != (RX_RING_ADDRESS >> 32) as u32)
                {
                    self.violation("receiver enabled before its ring was programmed");
                }
                self.set(RCTL, value);
            }
            TCTL => {
                if is_82574 && self.reg(TDH) != self.reg(TDT) {
                    self.violation("TCTL modified while TDH != TDT (DS 10.2.6.1, MULR)");
                }
                if value & TCTL_EN != 0
                    && (self.reg(TDLEN) as usize != nic::TX_RING * nic::DESCRIPTOR_BYTES
                        || self.reg(TDBAL) != TX_RING_ADDRESS as u32
                        || self.reg(TDBAH) != (TX_RING_ADDRESS >> 32) as u32)
                {
                    self.violation("transmitter enabled before its ring was programmed");
                }
                self.set(TCTL, value);
            }
            TDT => {
                self.set(TDT, value);
                self.run_transmit();
            }
            MDIC => self.mdi_command(value),
            _ => {
                if is_82574 && (offset == RAL + 8 * 15 || offset == RAH + 8 * 15) {
                    self.violation("82574 receive address 15 written; it is manageability's (DS 10.2.5.23)");
                }
                self.set(offset, value);
            }
        }
    }

    fn delay_us(&mut self, us: u32) {
        self.now_ns += u64::from(us) * 1000;
    }
}

impl Rings for Device {
    fn rx_ring_address(&self) -> u64 {
        RX_RING_ADDRESS
    }
    fn tx_ring_address(&self) -> u64 {
        TX_RING_ADDRESS
    }
    fn rx_buffer_address(&self, index: usize) -> u64 {
        RX_BUFFERS + (index * nic::BUFFER_SIZE) as u64
    }
    fn tx_buffer_address(&self, index: usize) -> u64 {
        TX_BUFFERS + (index * nic::BUFFER_SIZE) as u64
    }
    fn rx_descriptor(&mut self, index: usize) -> [u64; 2] {
        self.rx_ring[index]
    }
    fn set_rx_descriptor(&mut self, index: usize, words: [u64; 2]) {
        self.rx_ring[index] = words;
    }
    fn tx_descriptor(&mut self, index: usize) -> [u64; 2] {
        self.tx_ring[index]
    }
    fn set_tx_descriptor(&mut self, index: usize, words: [u64; 2]) {
        self.tx_ring[index] = words;
    }
    fn read_rx_buffer(&mut self, index: usize, out: &mut [u8]) {
        let length = out.len();
        out.copy_from_slice(&self.rx_buffers[index][..length]);
    }
    fn write_tx_buffer(&mut self, index: usize, data: &[u8]) {
        self.tx_buffers[index][..data.len()].copy_from_slice(data);
    }
}

const MODELS: [Model; 2] = [Model::Pci8254x, Model::Pcie82574];

fn started_with(mut device: Device, model: Model) -> (Device, Nic) {
    let nic = Nic::bring_up(&mut device, model).expect("bring-up");
    assert!(device.violations.is_empty(), "{model:?}: {:?}", device.violations);
    (device, nic)
}

fn started(model: Model) -> (Device, Nic) {
    started_with(Device::new(model, true), model)
}

fn frame(seed: usize, length: usize) -> Vec<u8> {
    (0..length).map(|byte| (seed * 31 + byte) as u8).collect()
}

#[test]
fn bring_up_follows_the_manuals_reset_and_ring_setup() {
    assert_eq!(Model::from_device_id(0x100E), Some(Model::Pci8254x));
    assert_eq!(Model::from_device_id(0x10D3), Some(Model::Pcie82574));
    assert_eq!(Model::from_device_id(0x10D4), None);
    for model in MODELS {
        let (device, nic) = started(model);
        let is_82574 = model == Model::Pcie82574;
        let reset = device.first_write(CTRL, |value| value & CTRL_RST != 0).expect("CTRL.RST");
        let all_causes = if is_82574 { 0x01FF_FFFF } else { 0xFFFF_FFFF };
        let masked = device.first_write(IMC, |value| value == all_causes).expect("IMC");
        assert!(masked < reset, "{model:?}: interrupts masked before the reset (DS 4.6.1)");
        assert!(
            device.written(IMC).iter().any(|&(at, value)| at > reset && value == all_causes),
            "{model:?}: interrupts masked again after the reset (DS 4.6.1)"
        );
        if is_82574 {
            let disable = device
                .first_write(CTRL, |value| value & CTRL_GIO_MASTER_DISABLE != 0)
                .expect("master disable");
            assert!(disable < reset, "master disable precedes the reset (DS 3.1.3.10)");
            assert!(device.written(TCTL).iter().all(|&(at, _)| at > reset), "TCTL untouched before reset");
            assert_ne!(device.reg(GCR) & GCR_BIT22, 0, "GCR bit 22 (DS 4.6.2)");
            assert_ne!(device.reg(GCR2) & GCR2_BIT0, 0, "GCR2 bit 0 (DS 10.2.3.12)");
            assert_eq!(device.reg(TXDCTL), (1 << 24) | (1 << 22) | (1 << 16), "TXDCTL (DS 4.6.6)");
        } else {
            let rctl_off = device.first_write(RCTL, |value| value & RCTL_EN == 0).expect("RCTL.EN cleared");
            let tctl_off = device.first_write(TCTL, |value| value & TCTL_EN == 0).expect("TCTL.EN cleared");
            assert!(rctl_off < reset && tctl_off < reset, "8254x receive and transmit stopped first");
            assert!(device.written(GCR).is_empty() && device.written(GCR2).is_empty());
            assert!(device.written(TXDCTL).is_empty());
        }

        let ctrl = device.reg(CTRL);
        assert_ne!(ctrl & CTRL_SLU, 0, "{model:?}: set link up");
        for bit in [CTRL_FRCSPD, CTRL_FRCDPLX, CTRL_PHY_RST, CTRL_VME, CTRL_ILOS, CTRL_RFCE, CTRL_TFCE] {
            assert_eq!(ctrl & bit, 0, "{model:?}: CTRL bit {bit:#x} clear");
        }
        match model {
            Model::Pci8254x => assert_eq!(ctrl & (CTRL_ASDE | CTRL_BIT3), CTRL_ASDE, "SDM 14.3"),
            Model::Pcie82574 => assert_eq!(ctrl & (CTRL_ASDE | CTRL_BIT3), CTRL_BIT3, "DS 4.6.3.2"),
        }
        for offset in [FCAL, FCAH, FCT, FCTTV] {
            assert_eq!(device.last_value(offset), Some(0), "{model:?}: {offset:#x} written 0");
        }

        assert_eq!(nic.mac(), NVM_MAC, "{model:?}: station address");
        // The 8254x's RAH0 is invalid after a global reset (SDM 14.7), so its
        // address comes through EERD; the 82574 reloads RAL0/RAH0.
        assert_eq!(device.written(EERD).len(), if is_82574 { 0 } else { 3 }, "{model:?}: NVM reads");
        assert_eq!(device.reg(RAL), 0x7856_3412);
        assert_eq!(device.reg(RAH), 0xAB90 | RAH_AV);
        assert!(device.last_write(RAL) < device.last_write(RAH), "RAL before RAH (SDM 13.5.3)");
        for entry in 1..15 {
            assert_eq!(device.reg(RAH + 8 * entry), 0, "{model:?}: entry {entry} cleared");
        }
        let entry_15 = (device.reg(RAL + 8 * 15), device.reg(RAH + 8 * 15));
        if is_82574 {
            assert_eq!(entry_15, FIRMWARE_ENTRY, "manageability's entry 15 kept (DS 10.2.5.23)");
        } else {
            assert_eq!(entry_15.1, 0, "8254x entry 15 cleared");
        }
        for entry in 0..128 {
            assert_eq!(device.last_value(MTA + 4 * entry), Some(0));
        }

        assert_eq!(device.reg(RDBAL), RX_RING_ADDRESS as u32);
        assert_eq!(device.reg(RDBAH), (RX_RING_ADDRESS >> 32) as u32);
        assert_eq!(device.reg(RDLEN) as usize, nic::RX_RING * 16);
        assert_eq!((device.reg(RDH), device.reg(RDT)), (0, nic::RX_RING as u32 - 1));
        for (index, words) in device.rx_ring.iter().enumerate() {
            let (buffer, _, status, _) = rx_words(*words);
            assert_eq!((buffer, status), (device.rx_buffer_address(index), 0));
        }
        assert_eq!(device.reg(RCTL), RCTL_EN | RCTL_BAM | RCTL_SECRC, "{model:?}: RCTL");
        let enabled = device.first_write(RCTL, |value| value & RCTL_EN != 0).unwrap();
        assert!(enabled > device.last_write(RDT).unwrap(), "receiver enabled last");

        assert_eq!(device.reg(TDBAL), TX_RING_ADDRESS as u32);
        assert_eq!(device.reg(TDLEN) as usize, nic::TX_RING * 16);
        assert_eq!((device.reg(TDH), device.reg(TDT)), (0, 0));
        let expected_tipg = if is_82574 { 0x0060_2006 } else { 0x0060_200A };
        assert_eq!(device.reg(TIPG), expected_tipg, "{model:?}: TIPG");
        let (ct, cold, kept) = if is_82574 { (0x0F, 0x3F, TCTL_MULR | TCTL_RRTHRESH_01) } else { (0x10, 0x40, 0) };
        assert_eq!(device.reg(TCTL), TCTL_EN | TCTL_PSP | (ct << 4) | (cold << 12) | kept, "{model:?}: TCTL");
        assert!(device.last_write(TCTL) > device.last_write(TIPG), "transmitter enabled last");
        assert_eq!(device.reg(IMS), 0, "{model:?}: interrupts stay masked until enabled");
        assert!(nic::link_up(&mut Device::new(model, true)));
    }
}

/// Run the model's clock forward, as a caller waiting for carrier does.
fn wait_ms(device: &mut Device, ms: u32) {
    device.delay_us(ms * 1000);
}

#[test]
fn bring_up_restores_auto_negotiation_firmware_left_off() {
    for model in MODELS {
        // A negotiating PHY with link is read, not rewritten: a restart would
        // drop the link for a new negotiation.
        let (mut device, _) = started(model);
        assert!(device.phy_control_writes.is_empty(), "{model:?}: {:?}", device.phy_control_writes);
        assert!(nic::link_up(&mut device), "{model:?}: link kept");

        // Firmware forced 10 Mb/s with auto-negotiation off, or powered the
        // PHY down, or looped it back; the partner only negotiates, so there
        // is no carrier until the driver turns negotiation back on.
        let left = [
            1 << 8,
            PHY_CONTROL_DEFAULT | PHY_POWER_DOWN,
            PHY_CONTROL_DEFAULT | PHY_LOOPBACK,
        ];
        for control in left {
            let mut device = Device::new(model, true);
            device.phy_control = control;
            device.negotiated_at = None;
            // The 82574's hardware holds MDIO for a while after the reset.
            device.hardware_owns_nvm_until = 400_000;
            let (mut device, _) = started_with(device, model);
            wait_ms(&mut device, 3000);
            assert!(nic::link_up(&mut device), "{model:?}: {control:#06x}: carrier after bring-up");
            let written = *device.phy_control_writes.last().expect("PHY control written");
            assert_ne!(written & PHY_AN_ENABLE, 0, "{model:?}: auto-negotiation enabled");
            assert_ne!(written & PHY_RESTART_AN, 0, "{model:?}: and restarted (SDM 11.1.3)");
            assert_eq!(written & (PHY_POWER_DOWN | PHY_LOOPBACK | PHY_ISOLATE), 0);
            if model == Model::Pcie82574 {
                assert_eq!(device.reg(EXTCNF_CTRL) & EXTCNF_SW, 0, "MDIO released");
            }
        }

        // A PHY that never answers MDIC is reported, not taken as configured.
        let mut silent = Device::new(model, true);
        silent.mdi_stuck = true;
        let refused = Nic::bring_up(&mut silent, model);
        assert!(refused.is_err(), "{model:?}: bring-up with an unreachable PHY");
        assert!(silent.violations.is_empty(), "{model:?}: {:?}", silent.violations);
    }
}

#[test]
fn station_address_comes_through_the_nvm_and_its_semaphore() {
    // The 82574 without an auto-loaded address: NVM words through EERD, with
    // the semaphore retried while hardware holds it (DS 4.5.2).
    let mut busy = Device::new(Model::Pcie82574, false);
    busy.hardware_owns_nvm_until = 400_000;
    let (device, nic) = started_with(busy, Model::Pcie82574);
    assert_eq!(nic.mac(), NVM_MAC, "SDM 5.6.1 byte order");
    let reads = device.written(EERD);
    assert_eq!(reads.len(), 3, "words 0-2");
    let granted = device.written(EXTCNF_CTRL).iter().filter(|&&(_, v)| v & EXTCNF_SW != 0).count();
    assert!(granted > 1, "the request was written again after hardware let go");
    let released = device.last_write(EXTCNF_CTRL).unwrap();
    assert!(reads[2].0 < released, "released after the reads");
    assert_eq!(device.reg(EXTCNF_CTRL) & EXTCNF_SW, 0, "released");

    // Hardware keeps the semaphore: the fallback fails without touching EERD.
    let mut held = Device::new(Model::Pcie82574, false);
    held.hardware_owns_nvm_until = u64::MAX;
    let refused = Nic::bring_up(&mut held, Model::Pcie82574).err();
    assert_eq!(refused, Some(InitError::NvmOwnershipTimeout));
    assert!(!refused.unwrap().as_str().is_empty());
    assert!(held.written(EERD).is_empty(), "no EERD without the semaphore");
    assert!(held.violations.is_empty(), "{:?}", held.violations);

    // An NVM without a unicast address is refused.
    let mut blank = Device::new(Model::Pci8254x, false);
    blank.nvm = [0xFFFF; 3];
    assert_eq!(Nic::bring_up(&mut blank, Model::Pci8254x).err(), Some(InitError::NoStationAddress));
}

#[test]
fn bring_up_refuses_an_82574_that_does_not_quiesce_or_load_its_nvm() {
    // Master requests stay pending: no reset while they may be in flight (DS
    // 10.2.2.1), and the master disable stays set, blocking new requests.
    let mut stuck = Device::new(Model::Pcie82574, true);
    stuck.master_stuck = true;
    let refused = Nic::bring_up(&mut stuck, Model::Pcie82574).err();
    assert_eq!(refused, Some(InitError::MasterDrainTimeout));
    assert!(!refused.unwrap().as_str().is_empty());
    assert!(stuck.first_write(CTRL, |value| value & CTRL_RST != 0).is_none(), "no reset");
    assert_ne!(stuck.reg(CTRL) & CTRL_GIO_MASTER_DISABLE, 0, "master disable held");
    assert_eq!(stuck.reg(IMS), 0, "interrupts masked");
    assert!(stuck.violations.is_empty(), "{:?}", stuck.violations);

    // The NVM auto-read does not finish: a late load could rewrite CTRL after
    // configuration (DS 4.2, 10.2.2.1), so configuration is skipped.
    let mut unloaded = Device::new(Model::Pcie82574, true);
    unloaded.auto_read_stuck = true;
    let refused = Nic::bring_up(&mut unloaded, Model::Pcie82574).err();
    assert_eq!(refused, Some(InitError::NvmLoadTimeout));
    assert!(!refused.unwrap().as_str().is_empty());
    let reset = unloaded.first_write(CTRL, |value| value & CTRL_RST != 0).expect("reset");
    assert!(unloaded.written(CTRL).iter().all(|&(at, value)| at <= reset || value & CTRL_SLU == 0));
    assert!(unloaded.written(RCTL).is_empty() && unloaded.written(TCTL).is_empty());
    assert!(unloaded.violations.is_empty(), "{:?}", unloaded.violations);
}

#[test]
fn receive_ring_delivers_good_frames_and_keeps_the_device_supplied() {
    for model in MODELS {
        let (mut device, mut nic) = started(model);
        let mut out = vec![0u8; nic::BUFFER_SIZE];
        assert!(!nic.can_receive(&mut device));
        assert_eq!(nic.receive(&mut device, &mut out), Err(RxError::NoFrame));

        for round in 0..5 {
            let mut expected = Vec::new();
            for slot in 0..20 {
                let seed = round * 100 + slot;
                let data = frame(seed, 60 + (seed * 37) % 1400);
                if slot == 3 {
                    // A descriptor written back with CE set (SDM Table 3-3);
                    // with RCTL.SBP clear the MAC would not store it, so this
                    // is an injected error descriptor.
                    assert!(device.deliver(&data, 0x01));
                } else {
                    assert!(device.deliver(&data, 0));
                    expected.push(data);
                }
            }
            let mut received = Vec::new();
            while nic.can_receive(&mut device) {
                match nic.receive(&mut device, &mut out) {
                    Ok(length) => received.push(out[..length].to_vec()),
                    Err(error) => panic!("{model:?}: {}", error.as_str()),
                }
            }
            assert_eq!(nic.receive(&mut device, &mut out), Err(RxError::NoFrame));
            assert_eq!(received, expected, "{model:?} round {round}: every good frame, in order");
            let tail = device.reg(RDT) as usize;
            let (buffer, _, status, _) = rx_words(device.rx_ring[tail]);
            assert_eq!((buffer, status), (device.rx_buffer_address(tail), 0), "re-armed");
            assert_eq!((tail + 1) % nic::RX_RING, device.reg(RDH) as usize, "every buffer back");
        }
        assert_eq!(device.missed, 0, "{model:?}: the device never ran out of descriptors");

        // A frame spanning two buffers, its pieces arriving between polls,
        // is dropped whole; the frame after it is delivered.
        assert!(device.fill(&frame(7, nic::BUFFER_SIZE), false, 0));
        assert_eq!(nic.receive(&mut device, &mut out), Err(RxError::NoFrame));
        assert!(device.fill(&frame(8, 100), true, 0));
        let good = frame(9, 300);
        assert!(device.deliver(&good, 0));
        assert_eq!(nic.receive(&mut device, &mut out), Ok(good.len()));
        assert_eq!(out[..good.len()], good[..]);

        assert!(device.deliver(&frame(10, 600), 0));
        let mut small = [0u8; 64];
        assert_eq!(nic.receive(&mut device, &mut small), Err(RxError::BufferTooSmall));
        assert!(!nic.can_receive(&mut device), "the oversized frame's descriptor went back");
        assert!(device.violations.is_empty(), "{model:?}: {:?}", device.violations);
    }
}

#[test]
fn transmit_ring_recycles_completed_slots_and_reports_a_full_ring() {
    for model in MODELS {
        let (mut device, mut nic) = started(model);
        assert_eq!(nic.transmit(&mut device, &[]), Err(TxError::Empty));
        let too_long = frame(0, nic::MAX_FRAME + 1);
        assert_eq!(nic.transmit(&mut device, &too_long), Err(TxError::TooLong));

        let mut expected = Vec::new();
        for seed in 0..3 * nic::TX_RING {
            let data = frame(seed, 42 + seed * 13 % nic::MAX_FRAME);
            nic.transmit(&mut device, &data).expect("transmit");
            expected.push(data);
        }
        assert!(device.sent.iter().all(|(_, cmd)| *cmd == TXD_CMD_EOP_IFCS_RS));
        let sent: Vec<Vec<u8>> = device.sent.drain(..).map(|(data, _)| data).collect();
        assert_eq!(sent, expected, "{model:?}: frames sent intact and in order");

        device.transmit_paused = true;
        for seed in 0..nic::TX_RING - 1 {
            nic.transmit(&mut device, &frame(seed, 100)).expect("ring has room");
        }
        let full = nic.transmit(&mut device, &frame(99, 100));
        assert_eq!(full, Err(TxError::RingFull), "{model:?}: one slot always stays free");
        assert!(!full.unwrap_err().as_str().is_empty());
        device.transmit_paused = false;
        device.run_transmit();
        assert_eq!(device.sent.len(), nic::TX_RING - 1);
        nic.transmit(&mut device, &frame(100, 100)).expect("slots reclaimed once done");
        assert!(device.violations.is_empty(), "{model:?}: {:?}", device.violations);
    }
}

#[test]
fn interrupt_causes_are_unmasked_acknowledged_and_handed_over() {
    for model in MODELS {
        let (mut device, nic) = started(model);
        // A frame arrives before interrupts are routed: once they are, its
        // cause is acknowledged and the bottom half is owed a drain.
        assert!(device.deliver(&frame(1, 80), 0));
        nic::enable_interrupts(&mut device);
        assert_eq!(device.reg(IMS), INT_RXT0 | INT_RXO | INT_RXDMT0 | INT_LSC, "{model:?}: IMS");
        assert!(nic.interrupts_routed(&mut device, model), "{model:?}: a pending frame is drained");
        assert_eq!(device.reg(ICR), 0, "{model:?}: its cause acknowledged");
        let mut out = vec![0u8; nic::BUFFER_SIZE];
        let mut nic = nic;
        assert_eq!(nic.receive(&mut device, &mut out), Ok(80));
        assert!(!nic.interrupts_routed(&mut device, model), "{model:?}: nothing owed");
        // An interrupt on a shared line acknowledged this frame's cause
        // before the route was live: the waiting frame alone is owed a drain.
        assert!(device.deliver(&frame(3, 70), 0));
        nic::acknowledge_interrupt(&mut device, model);
        assert!(nic.interrupts_routed(&mut device, model), "{model:?}: a waiting frame is drained");
        assert_eq!(nic.receive(&mut device, &mut out), Ok(70));

        assert!(device.deliver(&frame(2, 90), 0));
        let causes = nic::acknowledge_interrupt(&mut device, model);
        assert_ne!(causes & INT_RXT0, 0);
        assert!(nic::needs_receive_work(causes));
        assert_eq!(device.reg(ICR), 0, "{model:?}: every cause read is cleared");

        device.raise(INT_LSC);
        assert!(nic::needs_receive_work(nic::acknowledge_interrupt(&mut device, model)));
        device.raise(INT_TXDW);
        let transmit_only = nic::acknowledge_interrupt(&mut device, model);
        assert!(!nic::needs_receive_work(transmit_only), "TXDW alone needs no receive work");
        assert_eq!(nic::acknowledge_interrupt(&mut device, model), 0, "nothing pending");
        assert!(device.violations.is_empty(), "{model:?}: {:?}", device.violations);
    }
}
