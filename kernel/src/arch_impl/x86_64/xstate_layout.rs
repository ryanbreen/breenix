//! Intel SDM Vol. 1, chapter 13. Layout and signal-image policy only.
pub const BASELINE: u64 = 3;
pub const AVX: u64 = 1 << 2;
pub const AVX512: u64 = (1 << 5) | (1 << 6) | (1 << 7);
pub const LEGACY_SIZE: usize = 512;
const EXTENDED_START: usize = 576;
const COMPACTED: u64 = 1 << 63;
const DEFAULT_MXCSR: [u8; 4] = 0x1f80u32.to_le_bytes();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Fxsave,
    Xsave,
    Xsaveopt,
    Xsaves,
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
struct Component {
    size: u32,
    standard: u32,
    align64: bool,
}

#[derive(Clone, Copy)]
pub struct CpuState {
    user: u64,
    supervisor: u64,
    instructions: u32,
    components: [Component; 63],
}

impl CpuState {
    pub fn probe(
        user: u64,
        supervisor: u64,
        max_standard: u32,
        instructions: u32,
        mut read_component: impl FnMut(u32) -> (u32, u32, u32),
    ) -> Option<Self> {
        if user & BASELINE != BASELINE
            || supervisor & BASELINE != 0
            || user & supervisor != 0
            || (user | supervisor) & COMPACTED != 0
            || max_standard < EXTENDED_START as u32
        {
            return None;
        }
        let mut components = [Component::default(); 63];
        for bit in 2..63 {
            let component_bit = 1 << bit;
            if (user | supervisor) & component_bit == 0 {
                continue;
            }
            let (size, offset, flags) = read_component(bit);
            let xss = supervisor & component_bit != 0;
            if (flags & 1 != 0) != xss || flags & !7 != 0 || size == 0 {
                return None;
            }
            let standard = if xss {
                0
            } else {
                let end = offset.checked_add(size)?;
                if offset < EXTENDED_START as u32
                    || end > max_standard
                    || components[..bit as usize].iter().any(|other| {
                        other.standard != 0
                            && offset < other.standard + other.size
                            && other.standard < end
                    })
                {
                    return None;
                }
                offset
            };
            components[bit as usize] = Component {
                size,
                standard,
                align64: flags & 2 != 0,
            };
        }
        Some(Self {
            user,
            supervisor,
            instructions,
            components,
        })
    }

    pub fn reported(&self) -> (u64, u64) {
        (self.user, self.supervisor)
    }

    pub fn avx_layout_known(&self) -> bool {
        self.components[2].size == 256 && self.components[2].standard == 576
    }

    pub fn avx512_layout_known(&self) -> bool {
        self.components[5].size == 64
            && self.components[6].size == 512
            && self.components[7].size == 1024
    }

    pub fn best_backend(&self) -> Backend {
        if self.instructions & 8 != 0 {
            Backend::Xsaves
        } else if self.instructions & 1 != 0 {
            Backend::Xsaveopt
        } else {
            Backend::Xsave
        }
    }

    pub fn layout(&self, user: u64, supervisor: u64, backend: Backend) -> Option<Layout> {
        if user & BASELINE != BASELINE
            || user & !self.user != 0
            || supervisor & !self.supervisor != 0
            || (user & AVX512 != 0 && user & (AVX | AVX512) != AVX | AVX512)
        {
            return None;
        }
        match backend {
            Backend::Fxsave => return (user == BASELINE && supervisor == 0).then(Layout::legacy),
            Backend::Xsaves if self.instructions & 8 != 0 => {}
            Backend::Xsaveopt if self.instructions & 1 != 0 && supervisor == 0 => {}
            Backend::Xsave if supervisor == 0 => {}
            _ => return None,
        }
        let mut result = Layout {
            user,
            supervisor,
            backend,
            standard_size: EXTENDED_START,
            compacted_size: EXTENDED_START,
            components: [Component::default(); 63],
            offsets: [0; 63],
        };
        for bit in 2..63 {
            let component_bit = 1 << bit;
            if (user | supervisor) & component_bit == 0 {
                continue;
            }
            let component = self.components[bit];
            result.components[bit] = component;
            if user & component_bit != 0 {
                let end = (component.standard as usize).checked_add(component.size as usize)?;
                result.standard_size = result.standard_size.max(end);
            }
            if backend == Backend::Xsaves {
                if component.align64 {
                    result.compacted_size = result.compacted_size.checked_add(63)? & !63;
                }
                result.offsets[bit] = u32::try_from(result.compacted_size).ok()?;
                result.compacted_size =
                    result.compacted_size.checked_add(component.size as usize)?;
                u32::try_from(result.compacted_size).ok()?;
            } else {
                result.offsets[bit] = component.standard;
            }
        }
        Some(result)
    }

    pub fn supports(&self, layout: &Layout) -> bool {
        self.layout(layout.user, layout.supervisor, layout.backend)
            .as_ref()
            == Some(layout)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub user: u64,
    pub supervisor: u64,
    pub backend: Backend,
    pub standard_size: usize,
    compacted_size: usize,
    components: [Component; 63],
    offsets: [u32; 63],
}

impl Layout {
    pub const fn legacy() -> Self {
        Self {
            user: BASELINE,
            supervisor: 0,
            backend: Backend::Fxsave,
            standard_size: LEGACY_SIZE,
            compacted_size: 0,
            components: [Component {
                size: 0,
                standard: 0,
                align64: false,
            }; 63],
            offsets: [0; 63],
        }
    }

    pub fn offset(&self, bit: usize) -> usize {
        self.offsets[bit] as usize
    }

    pub fn image_size(&self) -> usize {
        match self.backend {
            Backend::Xsaves => self.compacted_size,
            _ => self.standard_size,
        }
    }

    pub fn signal_size(&self) -> usize {
        if self.user == BASELINE {
            LEGACY_SIZE
        } else {
            self.standard_size
        }
    }

    pub fn check_reported(&self, standard: usize, compacted: usize) -> bool {
        self.standard_size == standard
            && (self.backend != Backend::Xsaves || self.compacted_size == compacted)
    }

    pub fn initial(&self, image: &mut [u8]) {
        initialize(
            image,
            self.backend == Backend::Xsaves,
            self.user | self.supervisor,
        );
    }

    pub fn import(
        &self,
        backing: &mut [u8],
        standard: &mut [u8],
        extended: bool,
        mask: u32,
    ) -> bool {
        let expected = if extended {
            self.signal_size()
        } else {
            LEGACY_SIZE
        };
        if standard.len() != expected || (extended && self.user == BASELINE) {
            return false;
        }
        let bv = if extended {
            let bv = word(standard, 512);
            if bv & !self.user != 0 || standard[520..576].iter().any(|&byte| byte != 0) {
                return false;
            }
            bv
        } else {
            BASELINE
        };
        // Standard XRSTOR loads MXCSR even when XSTATE_BV[1] is clear.
        let mxcsr = u32::from_le_bytes(standard[24..28].try_into().unwrap()) & mask;
        standard[24..28].copy_from_slice(&mxcsr.to_le_bytes());
        self.initial(backing);
        transfer_legacy(backing, standard, bv, true);
        if self.backend != Backend::Fxsave {
            let bv = if mxcsr.to_le_bytes() != DEFAULT_MXCSR {
                bv | 2
            } else {
                bv
            };
            backing[512..520].copy_from_slice(&bv.to_le_bytes());
            if extended {
                self.transfer_extensions(backing, standard, bv);
            }
        }
        true
    }

    pub fn export(&self, standard: &mut [u8], backing: &[u8]) {
        self.export_from(standard, backing, false);
    }

    pub fn export_from(&self, standard: &mut [u8], backing: &[u8], source_standard: bool) {
        initialize(standard, false, 0);
        let bv = if self.backend == Backend::Fxsave {
            BASELINE
        } else {
            word(backing, 512) & self.user
        };
        let carry_mxcsr = source_standard || self.backend != Backend::Xsaves || bv & 2 != 0;
        transfer_legacy(standard, backing, bv, carry_mxcsr);
        if standard.len() > LEGACY_SIZE {
            standard[512..520].copy_from_slice(&bv.to_le_bytes());
            for bit in 2..63 {
                if bv & (1 << bit) == 0 {
                    continue;
                }
                let component = self.components[bit];
                let dest = component.standard as usize;
                let source = if source_standard {
                    dest
                } else {
                    self.offset(bit)
                };
                let size = component.size as usize;
                standard[dest..dest + size].copy_from_slice(&backing[source..source + size]);
            }
        }
    }

    fn transfer_extensions(&self, backing: &mut [u8], standard: &[u8], bv: u64) {
        for bit in 2..63 {
            if bv & (1 << bit) == 0 {
                continue;
            }
            let component = self.components[bit];
            let source = component.standard as usize;
            let dest = self.offset(bit);
            let size = component.size as usize;
            backing[dest..dest + size].copy_from_slice(&standard[source..source + size]);
        }
    }
}

pub fn initialize(memory: &mut [u8], compacted: bool, mask: u64) {
    memory.fill(0);
    memory[..2].copy_from_slice(&0x037fu16.to_le_bytes());
    memory[24..28].copy_from_slice(&DEFAULT_MXCSR);
    if compacted {
        memory[520..528].copy_from_slice(&(COMPACTED | mask).to_le_bytes());
    }
}

fn word(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

fn transfer_legacy(dest: &mut [u8], source: &[u8], bv: u64, mxcsr: bool) {
    if bv & 1 != 0 {
        dest[..5].copy_from_slice(&source[..5]);
        dest[6..24].copy_from_slice(&source[6..24]);
        for offset in (32..160).step_by(16) {
            dest[offset..offset + 10].copy_from_slice(&source[offset..offset + 10]);
        }
    }
    if mxcsr {
        dest[24..32].copy_from_slice(&source[24..32]);
    }
    if bv & 2 != 0 {
        dest[160..416].copy_from_slice(&source[160..416]);
    }
}

/// The caller excludes local dispatch and IRQs while registers are live.
/// After restore the persistent role must follow the address used by hardware.
pub fn replace_live<Area, Cpu>(
    saved: &mut Area,
    prepared: &mut Area,
    cpu: &mut Cpu,
    save: impl FnOnce(&mut Cpu, &mut Area),
    try_restore: impl FnOnce(&mut Cpu, &Area) -> bool,
    restore: impl FnOnce(&mut Cpu, &Area),
) -> bool {
    save(cpu, saved);
    if !try_restore(cpu, prepared) {
        restore(cpu, saved);
        return false;
    }
    core::mem::swap(saved, prepared);
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignalPlacement {
    pub image: u64,
    pub rsp: u64,
    pub trampoline: Option<u64>,
}

pub fn signal_placement(
    sp: u64,
    alt_base: u64,
    alt_top: u64,
    use_alt: bool,
    image_size: u64,
    frame_size: u64,
    trampoline: Option<u64>,
) -> Option<SignalPlacement> {
    let top = if use_alt {
        alt_top
    } else {
        sp.checked_sub(128)?
    };
    let image = top.checked_sub(image_size)? & !63;
    let trampoline = match trampoline {
        Some(size) => Some(image.checked_sub(size)? & !15),
        None => None,
    };
    let rsp = (trampoline.unwrap_or(image).checked_sub(frame_size)? & !15).checked_sub(8)?;
    if (use_alt || (alt_base..alt_top).contains(&sp))
        && (rsp < alt_base || image.checked_add(image_size)? > alt_top)
    {
        return None;
    }
    Some(SignalPlacement {
        image,
        rsp,
        trampoline,
    })
}

/// Usable features after applying the kernel's XCR0 policy.
pub fn capabilities(
    leaf1d: u32,
    leaf1c: u32,
    leaf7b: u32,
    leaf7d: u32,
    osxsave: bool,
    enabled: u64,
) -> [u64; 2] {
    use crate::auxv::x86::*;
    let bit = |reg: u32, index: u32, flag| if reg & (1 << index) != 0 { flag } else { 0 };
    let mut first = 0;
    if enabled & BASELINE == BASELINE {
        first |= bit(leaf1d, 26, HWCAP_SSE2)
            | bit(leaf1c, 25, HWCAP_AESNI)
            | bit(leaf1c, 1, HWCAP_PCLMULQDQ);
        if osxsave && leaf1c & (1 << 28) != 0 && enabled & AVX != 0 {
            first |= HWCAP_AVX | bit(leaf7b, 5, HWCAP_AVX2);
            if enabled & AVX512 == AVX512 {
                first |= bit(leaf7b, 16, HWCAP_AVX512F);
            }
        }
    }
    [
        first,
        bit(leaf7b, 9, HWCAP2_ERMS) | bit(leaf7d, 4, HWCAP2_FSRM),
    ]
}
