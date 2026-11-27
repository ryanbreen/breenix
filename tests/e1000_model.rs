//! The e1000 driver's MAC programming (`kernel/src/drivers/e1000/nic.rs`),
//! executed against a model of the two Intel manuals it is written from:
//! the 8254x Software Developer's Manual ("SDM", 82540EM) and the 82574
//! datasheet ("DS"). The model implements the register behaviour those
//! documents specify (reset self-clearing, the 82574 master-disable handshake,
//! the two EERD layouts and the NVM semaphore, ICR clear-on-read against
//! write-1-to-clear, and descriptor-ring DMA) and records every access, so the
//! tests can check the order and values the manuals require.

#[path = "../kernel/src/drivers/e1000/regs.rs"]
mod regs;
#[path = "../kernel/src/drivers/e1000/nic.rs"]
mod nic;

use std::collections::HashMap;

use nic::{Model, Nic, Registers, Rings, RxDescriptor, RxError, TxDescriptor, TxError};
use regs::*;

const RX_RING_ADDRESS: u64 = 0x1_0000_0000;
const TX_RING_ADDRESS: u64 = 0x1_0000_0200;
const RX_BUFFERS: u64 = 0x2_0000_0000;
const TX_BUFFERS: u64 = 0x3_0000_0000;
/// The SDM 5.6.1 example address 12-34-56-78-90-AB as stored in NVM words 0-2.
const NVM_WORDS: [u16; 3] = [0x3412, 0x7856, 0xAB90];
const NVM_MAC: [u8; 6] = [0x12, 0x34, 0x56, 0x78, 0x90, 0xAB];

struct Device {
    model: Model,
    registers: HashMap<u32, u32>,
    /// Every register write, in order: (offset, value).
    log: Vec<(u32, u32)>,
    now_ns: u64,
    /// When the global reset started.
    reset_at: Option<u64>,
    /// When STATUS.GIO_MASTER_ENABLE clears after a master disable.
    master_idle_at: Option<u64>,
    /// Whether a reset reloads RAL0/RAH0 from the NVM.
    loads_address: bool,
    nvm: [u16; 3],
    rx_ring: Vec<[u64; 2]>,
    tx_ring: Vec<[u64; 2]>,
    rx_buffers: Vec<Vec<u8>>,
    tx_buffers: Vec<Vec<u8>>,
    /// Frames the transmit engine sent, with their command bytes.
    sent: Vec<(Vec<u8>, u8)>,
    transmit_paused: bool,
    missed: usize,
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
            loads_address,
            nvm: NVM_WORDS,
            rx_ring: vec![[0; 2]; nic::RX_RING],
            tx_ring: vec![[0; 2]; nic::TX_RING],
            rx_buffers: vec![vec![0; nic::BUFFER_SIZE]; nic::RX_RING],
            tx_buffers: vec![vec![0; nic::BUFFER_SIZE]; nic::TX_RING],
            sent: Vec::new(),
            transmit_paused: false,
            missed: 0,
            violations: Vec::new(),
        };
        device.power_on();
        // Firmware left the port running, as a boot loader may.
        device.registers.insert(RCTL, RCTL_EN);
        device.registers.insert(TCTL, TCTL_EN | TCTL_PSP);
        device.registers.insert(IMS, INT_RXT0);
        device
    }

    fn reg(&self, offset: u32) -> u32 {
        self.registers.get(&offset).copied().unwrap_or(0)
    }

    /// Power-on values of the registers this model gives meaning to (SDM
    /// 13.4.1 and 13.4.33; DS 10.2.2.1 and 10.2.6.1). Receive addresses load
    /// from the NVM (SDM 5.6.1; DS 6.1.1.1) when `loads_address`.
    fn power_on(&mut self) {
        let keep: Vec<(u32, u32)> = [RAL0, RAH0]
            .iter()
            .map(|&offset| (offset, self.reg(offset)))
            .collect();
        self.registers.clear();
        let (ctrl, tctl) = match self.model {
            Model::Pci8254x => (CTRL_BIT3 | (0b10 << 8) | CTRL_FRCSPD, 0),
            Model::Pcie82574 => (
                1 | CTRL_BIT3 | (0b10 << 8) | (1 << 20),
                TCTL_PSP | (1 << 28) | (0b01 << 29),
            ),
        };
        self.registers.insert(CTRL, ctrl);
        self.registers.insert(TCTL, tctl);
        self.registers.insert(STATUS, STATUS_LU | STATUS_GIO_MASTER_ENABLE);
        self.registers.insert(EXTCNF_CTRL, 1 << 3);
        for (offset, value) in keep {
            self.registers.insert(offset, value);
        }
        self.registers.insert(RAH0, self.reg(RAH0) & !RAH_AV);
        if self.loads_address {
            let [a, b, c] = self.nvm;
            self.registers.insert(RAL0, u32::from(a) | (u32::from(b) << 16));
            self.registers.insert(RAH0, u32::from(c) | RAH_AV);
        }
    }

    fn violation(&mut self, text: impl Into<String>) {
        self.violations.push(text.into());
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
            }
        }
        if let Some(at) = self.master_idle_at {
            if self.now_ns >= at {
                let status = self.reg(STATUS) & !STATUS_GIO_MASTER_ENABLE;
                self.registers.insert(STATUS, status);
            }
        }
    }

    fn raise(&mut self, causes: u32) {
        let icr = self.reg(ICR) | causes;
        self.registers.insert(ICR, icr);
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

    /// The receive DMA engine (SDM 3.2.4-3.2.6): fill the descriptor at RDH
    /// unless head has reached tail, write status back, advance RDH and raise
    /// RXT0. A frame longer than one buffer spans descriptors, EOP on the last.
    fn deliver(&mut self, frame: &[u8], errors: u8) -> bool {
        if self.reg(RCTL) & RCTL_EN == 0 {
            return false;
        }
        let pieces: Vec<&[u8]> = frame.chunks(nic::BUFFER_SIZE).collect();
        let free = (self.reg(RDT) as usize + nic::RX_RING - self.reg(RDH) as usize) % nic::RX_RING;
        if free < pieces.len() {
            self.missed += 1;
            self.raise(INT_RXO);
            return false;
        }
        for (piece_index, piece) in pieces.iter().enumerate() {
            let head = self.reg(RDH) as usize;
            let descriptor = RxDescriptor::from_words(self.rx_ring[head]);
            let Some(buffer) = self.rx_index(descriptor.buffer) else {
                self.violation(format!("RX descriptor {head} names buffer {:#x}", descriptor.buffer));
                return false;
            };
            if descriptor.status != 0 {
                self.violation(format!("RX descriptor {head} handed to hardware with status set"));
            }
            self.rx_buffers[buffer][..piece.len()].copy_from_slice(piece);
            let last = piece_index + 1 == pieces.len();
            let written = RxDescriptor {
                length: piece.len() as u16,
                status: RXD_STATUS_DD | if last { RXD_STATUS_EOP } else { 0 },
                errors: if last { errors } else { 0 },
                ..descriptor
            };
            self.rx_ring[head] = written.to_words();
            self.registers.insert(RDH, ((head + 1) % nic::RX_RING) as u32);
        }
        self.raise(INT_RXT0);
        true
    }

    /// The transmit DMA engine (SDM 3.3): send every descriptor from TDH up to
    /// TDT, writing DD back for those with RS.
    fn run_transmit(&mut self) {
        if self.transmit_paused || self.reg(TCTL) & TCTL_EN == 0 {
            return;
        }
        while self.reg(TDH) != self.reg(TDT) {
            let head = self.reg(TDH) as usize;
            let descriptor = TxDescriptor::from_words(self.tx_ring[head]);
            match self.tx_index(descriptor.buffer) {
                Some(buffer) if buffer == head => {
                    let frame = self.tx_buffers[buffer][..usize::from(descriptor.length)].to_vec();
                    self.sent.push((frame, descriptor.cmd));
                }
                _ => self.violation(format!("TX descriptor {head} names buffer {:#x}", descriptor.buffer)),
            }
            if descriptor.cmd & TXD_CMD_RS != 0 {
                let done = TxDescriptor { status: descriptor.status | TXD_STATUS_DD, ..descriptor };
                self.tx_ring[head] = done.to_words();
            }
            self.registers.insert(TDH, ((head + 1) % nic::TX_RING) as u32);
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
}

impl Registers for Device {
    fn read(&mut self, offset: u32) -> u32 {
        self.settle();
        let value = match offset {
            CTRL if self.reset_at.is_some() => self.reg(CTRL) | CTRL_RST,
            ICR => {
                let value = self.reg(ICR);
                // SDM 13.4.17: reading clears. DS 10.2.4.1: reading clears only
                // while every cause is masked; otherwise it has no side effect
                // and software writes 1b to clear.
                if self.model == Model::Pci8254x || self.reg(IMS) == 0 {
                    self.registers.insert(ICR, 0);
                }
                value
            }
            _ => self.reg(offset),
        };
        value
    }

    fn write(&mut self, offset: u32, value: u32) {
        self.settle();
        self.log.push((offset, value));
        match offset {
            CTRL => {
                if value & CTRL_RST != 0 {
                    if self.reg(RCTL) & RCTL_EN != 0 || self.reg(TCTL) & TCTL_EN != 0 {
                        self.violation("reset with receive or transmit enabled (DS 10.2.2.1)");
                    }
                    if self.model == Model::Pcie82574
                        && self.reg(STATUS) & STATUS_GIO_MASTER_ENABLE != 0
                    {
                        self.violation("82574 reset with master requests not drained (DS 3.1.3.10)");
                    }
                    self.reset_at = Some(self.now_ns);
                    self.registers.insert(CTRL, value & !CTRL_RST);
                    return;
                }
                if self.model == Model::Pcie82574 && value & CTRL_GIO_MASTER_DISABLE != 0 {
                    self.master_idle_at = Some(self.now_ns + 20_000);
                }
                self.registers.insert(CTRL, value);
            }
            ICR => match self.model {
                Model::Pci8254x => self.violation("ICR written on the 8254x, where it is read-only"),
                Model::Pcie82574 => {
                    let icr = self.reg(ICR) & !value;
                    self.registers.insert(ICR, icr);
                }
            },
            IMS => {
                let ims = self.reg(IMS) | value;
                self.registers.insert(IMS, ims);
            }
            IMC => {
                let ims = self.reg(IMS) & !value;
                self.registers.insert(IMS, ims);
            }
            EXTCNF_CTRL => {
                if self.model == Model::Pci8254x {
                    self.violation("EXTCNF_CTRL written on the 8254x");
                }
                self.registers.insert(EXTCNF_CTRL, value);
            }
            EERD if value & EERD_START != 0 => {
                let (done, shift) = match self.model {
                    Model::Pci8254x => (EERD_8254X_DONE, EERD_8254X_ADDR_SHIFT),
                    Model::Pcie82574 => (EERD_82574_DONE, EERD_82574_ADDR_SHIFT),
                };
                if self.model == Model::Pcie82574
                    && self.reg(EXTCNF_CTRL) & EXTCNF_CTRL_SW_OWNERSHIP == 0
                {
                    self.violation("EERD read without NVM ownership (DS 4.5.2)");
                }
                let address = ((value >> shift) & 0xFF) as usize;
                let data = self.nvm.get(address).copied().unwrap_or(0xFFFF);
                let width_mask = if shift == EERD_8254X_ADDR_SHIFT { 0xFF } else { 0x3FFF };
                let address_field = ((address as u32) & width_mask) << shift;
                self.registers.insert(EERD, (u32::from(data) << EERD_DATA_SHIFT) | address_field | done);
            }
            RCTL => {
                if value & RCTL_EN != 0
                    && (self.reg(RDLEN) as usize != nic::RX_RING * nic::DESCRIPTOR_BYTES
                        || self.reg(RDBAL) != RX_RING_ADDRESS as u32
                        || self.reg(RDBAH) != (RX_RING_ADDRESS >> 32) as u32)
                {
                    self.violation("receiver enabled before its ring was programmed");
                }
                self.registers.insert(RCTL, value);
            }
            TCTL => {
                if value & TCTL_EN != 0
                    && (self.reg(TDLEN) as usize != nic::TX_RING * nic::DESCRIPTOR_BYTES
                        || self.reg(TDBAL) != TX_RING_ADDRESS as u32
                        || self.reg(TDBAH) != (TX_RING_ADDRESS >> 32) as u32)
                {
                    self.violation("transmitter enabled before its ring was programmed");
                }
                self.registers.insert(TCTL, value);
            }
            TDT => {
                self.registers.insert(TDT, value);
                self.run_transmit();
            }
            _ => {
                self.registers.insert(offset, value);
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

fn started(model: Model, loads_address: bool) -> (Device, Nic) {
    let mut device = Device::new(model, loads_address);
    let nic = Nic::bring_up(&mut device, model).expect("bring-up");
    assert!(device.violations.is_empty(), "{model:?}: {:?}", device.violations);
    (device, nic)
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
        let (device, nic) = started(model, true);
        let reset = device.first_write(CTRL, |value| value & CTRL_RST != 0).expect("CTRL.RST");
        let masked = device.first_write(IMC, |value| value == INT_ALL).expect("IMC");
        assert!(masked < reset, "{model:?}: interrupts masked before the reset (DS 4.6.1)");
        assert!(
            device.written(IMC).iter().any(|&(at, value)| at > reset && value == INT_ALL),
            "{model:?}: interrupts masked again after the reset (DS 4.6.1)"
        );
        if model == Model::Pcie82574 {
            let disable = device
                .first_write(CTRL, |value| value & CTRL_GIO_MASTER_DISABLE != 0)
                .expect("master disable");
            assert!(disable < reset, "master disable precedes the reset (DS 3.1.3.10)");
            assert_ne!(device.reg(GCR) & GCR_BIT22, 0, "GCR bit 22 (DS 4.6.2)");
            let txdctl = device.reg(TXDCTL);
            assert_eq!(txdctl, TXDCTL_GRAN_DESCRIPTORS | (1 << TXDCTL_WTHRESH_SHIFT) | TXDCTL_BIT22);
        } else {
            assert!(device.written(GCR).is_empty() && device.written(TXDCTL).is_empty());
            assert!(device.written(ICR).is_empty(), "ICR is read-only on the 8254x");
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
            assert_eq!(device.written(offset).last().map(|&(_, v)| v), Some(0), "{model:?}: {offset:#x}");
        }

        assert_eq!(nic.mac(), NVM_MAC, "{model:?}: the address loaded into RAL0/RAH0");
        assert!(device.written(EERD).is_empty(), "{model:?}: no NVM read when RAH0 is valid");
        assert_eq!(device.reg(RAL0), 0x7856_3412);
        assert_eq!(device.reg(RAH0), 0xAB90 | RAH_AV);
        assert!(device.last_write(RAL0) < device.last_write(RAH0), "RAL before RAH (SDM 13.5.3)");
        for entry in 1..RECEIVE_ADDRESSES {
            assert_eq!(device.reg(RAH0 + 8 * entry), 0);
        }
        for entry in 0..MTA_ENTRIES {
            assert_eq!(device.written(MTA + 4 * entry).last().map(|&(_, v)| v), Some(0));
        }

        assert_eq!(device.reg(RDBAL), RX_RING_ADDRESS as u32);
        assert_eq!(device.reg(RDBAH), (RX_RING_ADDRESS >> 32) as u32);
        assert_eq!(device.reg(RDLEN) as usize, nic::RX_RING * nic::DESCRIPTOR_BYTES);
        assert_eq!((device.reg(RDH), device.reg(RDT)), (0, nic::RX_RING as u32 - 1));
        for (index, words) in device.rx_ring.iter().enumerate() {
            let descriptor = RxDescriptor::from_words(*words);
            assert_eq!(descriptor.buffer, device.rx_buffer_address(index));
            assert_eq!(descriptor.status, 0);
        }
        let rctl = device.reg(RCTL);
        assert_eq!(rctl, RCTL_EN | RCTL_BAM | RCTL_SECRC, "{model:?}: RCTL");
        let enabled = device.first_write(RCTL, |value| value & RCTL_EN != 0).unwrap();
        assert!(enabled > device.last_write(RDT).unwrap(), "receiver enabled last");

        assert_eq!(device.reg(TDBAL), TX_RING_ADDRESS as u32);
        assert_eq!(device.reg(TDLEN) as usize, nic::TX_RING * nic::DESCRIPTOR_BYTES);
        assert_eq!((device.reg(TDH), device.reg(TDT)), (0, 0));
        let expected_tipg = match model {
            Model::Pci8254x => 0x0060_200A,
            Model::Pcie82574 => 0x0060_2006,
        };
        assert_eq!(device.reg(TIPG), expected_tipg, "{model:?}: TIPG");
        let tctl = device.reg(TCTL);
        let (ct, cold, kept) = match model {
            Model::Pci8254x => (0x10, 0x40, 0),
            Model::Pcie82574 => (0x0F, 0x3F, (1 << 28) | (0b01 << 29)),
        };
        assert_eq!(
            tctl,
            TCTL_EN | TCTL_PSP | (ct << TCTL_CT_SHIFT) | (cold << TCTL_COLD_SHIFT) | kept,
            "{model:?}: TCTL"
        );
        assert!(device.last_write(TCTL) > device.last_write(TIPG), "transmitter enabled last");
        assert_eq!(device.reg(IMS), 0, "{model:?}: interrupts stay masked until enabled");
        assert!(nic::link_up(&mut Device::new(model, true)));
    }
}

#[test]
fn station_address_falls_back_to_the_nvm_in_each_models_eerd_layout() {
    for model in MODELS {
        let (device, nic) = started(model, false);
        assert_eq!(nic.mac(), NVM_MAC, "{model:?}: SDM 5.6.1 byte order");
        assert_eq!(device.written(EERD).len(), 3, "{model:?}: words 0-2");
        assert_eq!(device.reg(RAH0), 0xAB90 | RAH_AV);
        if model == Model::Pcie82574 {
            let taken = device
                .first_write(EXTCNF_CTRL, |value| value & EXTCNF_CTRL_SW_OWNERSHIP != 0)
                .expect("ownership requested");
            let released = device.last_write(EXTCNF_CTRL).unwrap();
            let reads = device.written(EERD);
            assert!(taken < reads[0].0 && reads[2].0 < released, "EERD inside the semaphore");
            assert_eq!(device.reg(EXTCNF_CTRL) & EXTCNF_CTRL_SW_OWNERSHIP, 0, "released");
        }
    }

    let mut blank = Device::new(Model::Pci8254x, false);
    blank.nvm = [0xFFFF; 3];
    let refused = Nic::bring_up(&mut blank, Model::Pci8254x).err();
    assert_eq!(refused, Some(nic::InitError::NoStationAddress), "a broadcast address is refused");
    assert!(!refused.unwrap().as_str().is_empty());
}

#[test]
fn receive_ring_delivers_good_frames_and_keeps_the_device_supplied() {
    for model in MODELS {
        let (mut device, mut nic) = started(model, true);
        let mut out = vec![0u8; nic::BUFFER_SIZE];
        assert!(!nic.can_receive(&mut device));
        assert_eq!(nic.receive(&mut device, &mut out), Err(RxError::NoFrame));

        let mut expected = Vec::new();
        for round in 0..5 {
            for slot in 0..20 {
                let seed = round * 100 + slot;
                let length = 60 + (seed * 37) % 1400;
                let data = frame(seed, length);
                match slot {
                    // A frame with a CRC error is dropped (SDM Table 3-3).
                    3 => assert!(device.deliver(&data, 0x01)),
                    // A frame spanning two buffers is dropped whole.
                    7 => assert!(device.deliver(&frame(seed, nic::BUFFER_SIZE + 100), 0)),
                    _ => {
                        assert!(device.deliver(&data, 0));
                        expected.push(data);
                    }
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
            let start = expected.len() - received.len();
            assert_eq!(received, expected[start..], "{model:?} round {round}");
            let tail = device.reg(RDT) as usize;
            let descriptor = RxDescriptor::from_words(device.rx_ring[tail]);
            assert_eq!(descriptor.status, 0, "the returned descriptor is re-armed");
            assert_eq!(descriptor.buffer, device.rx_buffer_address(tail));
            assert_eq!((tail + 1) % nic::RX_RING, device.reg(RDH) as usize, "every buffer back");
        }
        assert_eq!(device.missed, 0, "{model:?}: the device never ran out of descriptors");

        assert!(device.deliver(&frame(9, 600), 0));
        let mut small = [0u8; 64];
        assert_eq!(nic.receive(&mut device, &mut small), Err(RxError::BufferTooSmall));
        assert!(!nic.can_receive(&mut device), "the oversized frame's descriptor went back");
        assert!(device.violations.is_empty(), "{model:?}: {:?}", device.violations);
    }
}

#[test]
fn transmit_ring_recycles_completed_slots_and_reports_a_full_ring() {
    for model in MODELS {
        let (mut device, mut nic) = started(model, true);
        assert_eq!(nic.transmit(&mut device, &[]), Err(TxError::Empty));
        let too_long = frame(0, nic::MAX_FRAME + 1);
        assert_eq!(nic.transmit(&mut device, &too_long), Err(TxError::TooLong));

        let mut expected = Vec::new();
        for seed in 0..3 * nic::TX_RING {
            let data = frame(seed, 42 + seed * 13 % nic::MAX_FRAME);
            nic.transmit(&mut device, &data).expect("transmit");
            expected.push(data);
        }
        let commands: Vec<u8> = device.sent.iter().map(|(_, cmd)| *cmd).collect();
        assert!(commands.iter().all(|&cmd| cmd == TXD_CMD_EOP | TXD_CMD_IFCS | TXD_CMD_RS));
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
fn interrupt_causes_are_unmasked_and_acknowledged_for_each_model() {
    for model in MODELS {
        let (mut device, _nic) = started(model, true);
        nic::enable_interrupts(&mut device, model);
        let rx_causes = INT_RXT0 | INT_RXO | INT_RXDMT0 | INT_LSC;
        let expected = match model {
            Model::Pci8254x => rx_causes | INT_RXSEQ,
            Model::Pcie82574 => rx_causes,
        };
        assert_eq!(device.reg(IMS), expected, "{model:?}: IMS");

        assert!(device.deliver(&frame(1, 80), 0));
        let causes = nic::acknowledge_interrupt(&mut device, model);
        assert_ne!(causes & INT_RXT0, 0);
        assert!(nic::needs_receive_work(causes, model));
        assert_eq!(device.reg(ICR), 0, "{model:?}: every cause read is cleared");

        device.raise(INT_LSC);
        assert!(nic::needs_receive_work(nic::acknowledge_interrupt(&mut device, model), model));
        device.raise(1 << 0);
        let transmit_only = nic::acknowledge_interrupt(&mut device, model);
        assert!(!nic::needs_receive_work(transmit_only, model), "TXDW alone needs no receive work");
        assert_eq!(nic::acknowledge_interrupt(&mut device, model), 0, "nothing pending");
        assert!(device.violations.is_empty(), "{model:?}: {:?}", device.violations);
    }
}
