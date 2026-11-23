//! Relocation codes and field rules from AAELF64 2025Q1 §§5.7.5–5.7.8 and 5.7.11.4.

use bobj::elf::aarch64::{
    R_AARCH64_CALL26, R_AARCH64_CONDBR19, R_AARCH64_JUMP26, R_AARCH64_TSTBR14,
};

use crate::{Location, Result};

#[derive(Clone, Copy)]
#[repr(u32)]
pub(crate) enum Relocation {
    Abs64 = 257,
    Abs32 = 258,
    Prel32 = 261,
    Literal = 273,
    Adr = 274,
    AdrPrelPgHi21 = 275,
    AddAbsLo12Nc = 277,
    Ldst8AbsLo12Nc = 278,
    Test14 = R_AARCH64_TSTBR14,
    Cond19 = R_AARCH64_CONDBR19,
    Jump26 = R_AARCH64_JUMP26,
    Call26 = R_AARCH64_CALL26,
    Ldst16AbsLo12Nc = 284,
    Ldst32AbsLo12Nc = 285,
    Ldst64AbsLo12Nc = 286,
    Ldst128AbsLo12Nc = 299,
    AdrGotPage = 311,
    Ld64GotLo12Nc = 312,
    TlsleAddTprelHi12 = 549,
    TlsleAddTprelLo12Nc = 551,
}

impl Relocation {
    pub(crate) fn uses_tls(self) -> bool {
        matches!(self, Self::TlsleAddTprelHi12 | Self::TlsleAddTprelLo12Nc)
    }

    pub(crate) fn width(self) -> usize {
        if matches!(self, Self::Abs64) {
            8
        } else {
            4
        }
    }

    pub(crate) fn patch_local(
        self,
        field: &mut [u8],
        displacement: i128,
        at: Location,
    ) -> Result<bool> {
        // AAELF64 §5.7.6: signed S + A - P, scaled by four and inserted
        // into the instruction's immediate field.
        let (width, shift) = match self {
            Self::Jump26 | Self::Call26 => (26, 0),
            Self::Cond19 | Self::Literal => (19, 5),
            Self::Test14 => (14, 5),
            Self::Adr => {
                // C6.2.12: signed byte displacement in immhi:immlo.
                if !(-(1 << 20)..(1 << 20)).contains(&displacement) {
                    return Err(at.error("ADR target out of range"));
                }
                let imm = displacement as u32 & 0x1f_ffff;
                let bits = ((imm & 3) << 29) | ((imm >> 2) << 5);
                patch_word(field, bits);
                return Ok(true);
            }
            _ => return Ok(false),
        };
        let (alignment_error, range_error) = if matches!(self, Self::Literal) {
            // C6.2.216 and C7.2.231: signed imm19 scaled by four.
            (
                "literal target must be four-byte aligned",
                "literal target out of range",
            )
        } else {
            (
                "branch target must be four-byte aligned",
                "branch target out of range",
            )
        };
        if displacement % 4 != 0 {
            return Err(at.error(alignment_error));
        }
        let scaled = displacement / 4;
        let limit = 1i128 << (width - 1);
        if !(-limit..limit).contains(&scaled) {
            return Err(at.error(range_error));
        }
        let mask = (1u32 << width) - 1;
        patch_word(field, (scaled as u32 & mask) << shift);
        Ok(true)
    }
}

fn patch_word(field: &mut [u8], bits: u32) {
    // The instruction encoders leave the immediate unset before recording the reference.
    let word = u32::from_le_bytes([field[0], field[1], field[2], field[3]]);
    field.copy_from_slice(&(word | bits).to_le_bytes());
}
