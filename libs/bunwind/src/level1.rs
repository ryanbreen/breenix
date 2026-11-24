//! The Itanium C++ ABI exception-handling Level I engine (§§1.2–1.6).
//! See <https://itanium-cxx-abi.github.io/cxx-abi/abi-eh.html>.
//!
//! The engine walks saved contexts. A successful unwind yields the context
//! to install at a landing pad. The target supplies register capture and
//! restoration, memory and expression access, and personality dispatch.

use core::ops::BitOr;

use crate::cfi::{self, CfaRule, Program, RegisterRule, Rules};
use crate::ehframe;

const VERSION: u32 = 1;

/// The ABI's `_Unwind_Reason_Code` values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum ReasonCode {
    NoReason = 0,
    ForeignExceptionCaught = 1,
    FatalPhase2Error = 2,
    FatalPhase1Error = 3,
    NormalStop = 4,
    EndOfStack = 5,
    HandlerFound = 6,
    InstallContext = 7,
    ContinueUnwind = 8,
}

/// The ABI's `_Unwind_Action` bit mask.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct Actions(i32);

impl Actions {
    pub const SEARCH_PHASE: Self = Self(1);
    pub const CLEANUP_PHASE: Self = Self(2);
    pub const HANDLER_FRAME: Self = Self(4);
    pub const FORCE_UNWIND: Self = Self(8);
    /// LSB Core 5.0 §14, libgcc_s data definitions for unwind.h.
    pub const END_OF_STACK: Self = Self(16);

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn bits(self) -> i32 {
        self.0
    }
}

impl BitOr for Actions {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// A captured register file and the metadata of the current frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Context<const N: usize> {
    registers: [Option<u64>; N],
    ip: u64,
    ip_before_insn: bool,
    cfa: u64,
    region_start: u64,
    lsda: Option<u64>,
    text_base: Option<u64>,
    data_base: Option<u64>,
}

impl<const N: usize> Context<N> {
    /// Create a cursor from the target's captured registers. The frame
    /// source resolves its CFA, region start and LSDA when it is visited.
    pub fn new(registers: [Option<u64>; N], ip: u64, ip_before_insn: bool) -> Self {
        Self {
            registers,
            ip,
            ip_before_insn,
            cfa: 0,
            region_start: 0,
            lsda: None,
            text_base: None,
            data_base: None,
        }
    }

    /// Read a defined DWARF register within the register file.
    pub fn get_gr(&self, register: u64) -> Option<u64> {
        self.registers
            .get(usize::try_from(register).ok()?)
            .copied()
            .flatten()
    }

    /// Write a DWARF register, returning whether the column exists.
    pub fn set_gr(&mut self, register: u64, value: u64) -> bool {
        let Some(slot) = usize::try_from(register)
            .ok()
            .and_then(|index| self.registers.get_mut(index))
        else {
            return false;
        };
        *slot = Some(value);
        true
    }

    pub fn get_ip(&self) -> u64 {
        self.ip
    }

    /// The raw IP and whether it refers to an instruction about to execute
    /// (AMD64 psABI §6.2.5). Otherwise it is a return address.
    pub fn get_ip_info(&self) -> (u64, bool) {
        (self.ip, self.ip_before_insn)
    }

    pub fn set_ip(&mut self, ip: u64) {
        self.ip = ip;
        self.ip_before_insn = true;
    }

    pub fn get_cfa(&self) -> u64 {
        self.cfa
    }

    pub fn get_language_specific_data(&self) -> Option<u64> {
        self.lsda
    }

    pub fn get_text_base(&self) -> u64 {
        self.text_base.unwrap_or(0)
    }

    pub fn get_data_base(&self) -> u64 {
        self.data_base.unwrap_or(0)
    }

    pub fn get_region_start(&self) -> u64 {
        self.region_start
    }
}

pub type ExceptionCleanup<const N: usize> = fn(ReasonCode, &mut Exception<N>);
pub type StopFn<const N: usize> =
    fn(u32, Actions, &mut Exception<N>, &Context<N>, usize) -> ReasonCode;

/// Rust-side exception header and the private state for resuming cleanup.
pub struct Exception<const N: usize> {
    class: u64,
    cleanup: Option<ExceptionCleanup<N>>,
    pub(crate) continuation: Option<Continuation<N>>,
}

impl<const N: usize> Exception<N> {
    pub fn new(class: u64, cleanup: Option<ExceptionCleanup<N>>) -> Self {
        Self {
            class,
            cleanup,
            continuation: None,
        }
    }

    pub fn class(&self) -> u64 {
        self.class
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Continuation<const N: usize> {
    Handler { ip: Option<u64>, cfa: u64 },
    Forced { stop: StopFn<N>, parameter: usize },
}

/// The CFI row at a PC and the corresponding FDE/CIE metadata.
pub struct Frame<'a, const N: usize> {
    pub rules: Rules<'a, N>,
    pub region_start: u64,
    pub text_base: Option<u64>,
    pub data_base: Option<u64>,
    pub lsda: Option<u64>,
    pub personality: Option<u64>,
    pub return_address_register: u64,
    pub signal_frame: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LookupError {
    Fde(ehframe::Error),
    Cfi(cfi::Error),
    MissingRow,
}

impl<'a, const N: usize> Frame<'a, N> {
    /// Locate the FDE and select the DWARF 5 §6.4 row for an adjusted PC.
    /// The caller supplies the target's initial register rules and scratch
    /// space for DW_CFA_remember_state.
    pub fn lookup(
        source: &'a (impl ehframe::FrameSource + ?Sized),
        pc: u64,
        defaults: Rules<'a, N>,
        saved: &mut [Rules<'a, N>],
    ) -> Result<Option<Self>, LookupError> {
        let Some(sections) = source.sections_for_pc(pc) else {
            return Ok(None);
        };
        let Some(fde) = sections.find_fde(source, pc).map_err(LookupError::Fde)? else {
            return Ok(None);
        };
        let mut rules = None;
        Program {
            cie: fde.cie.initial_instructions,
            fde: fde.instructions,
            initial_location: fde.pc_range.start,
            address_range: fde.pc_range.end - fde.pc_range.start,
            code_alignment: fde.cie.code_alignment,
            data_alignment: fde.cie.data_alignment,
            address_size: sections.address_size,
            endian: sections.endian,
        }
        .rows(defaults, saved, |row| {
            if (row.start..row.end).contains(&pc) {
                rules = Some(*row.rules);
            }
        })
        .map_err(LookupError::Cfi)?;
        Ok(Some(Self {
            rules: rules.ok_or(LookupError::MissingRow)?,
            region_start: fde.pc_range.start,
            text_base: sections.text_base,
            data_base: sections.data_base,
            lsda: fde.lsda,
            personality: fde.cie.personality,
            return_address_register: fde.cie.return_address_register,
            signal_frame: fde.cie.signal_frame,
        }))
    }
}

/// Target operations used by the unwinder. Both phases query the same source
/// from the original cursor, so frame lookup must be repeatable.
pub trait FrameSource<const N: usize> {
    type Error;

    /// Look up metadata at the call or interrupted instruction. A nonzero
    /// IP without frame metadata is an unwind error.
    fn find_frame(&self, pc: u64) -> Result<Option<Frame<'_, N>>, Self::Error>;

    fn stack_pointer_register(&self) -> u64;

    /// Load a saved register at its target address.
    fn read_saved_register(&self, register: u64, address: u64) -> Option<u64>;

    /// Evaluate a DWARF expression. CFA is available for register rules.
    /// A missing result denotes a failed evaluation.
    fn evaluate_expression(
        &self,
        expression: &[u8],
        context: &Context<N>,
        cfa: Option<u64>,
    ) -> Option<u64>;

    fn personality(
        &self,
        address: u64,
        version: u32,
        actions: Actions,
        exception: &mut Exception<N>,
        context: &mut Context<N>,
    ) -> ReasonCode;
}

fn frame_at<'a, const N: usize>(
    source: &'a (impl FrameSource<N> + ?Sized),
    context: &mut Context<N>,
) -> Option<Frame<'a, N>> {
    // A return address may be just outside the caller's FDE.
    let pc = context.ip.checked_sub(u64::from(!context.ip_before_insn))?;
    let frame = source.find_frame(pc).ok()??;
    context.cfa = match frame.rules.cfa? {
        CfaRule::RegisterOffset { register, offset } => {
            context.get_gr(register)?.checked_add_signed(offset)?
        }
        CfaRule::Expression(expression) => source.evaluate_expression(expression, context, None)?,
    };
    context.region_start = frame.region_start;
    context.text_base = frame.text_base;
    context.data_base = frame.data_base;
    context.lsda = frame.lsda;
    Some(frame)
}

impl<const N: usize> Frame<'_, N> {
    /// Reconstruct the caller using DWARF 5 §6.4.1. Each rule reads
    /// the current frame, even when it refers to a different register.
    fn caller(
        &self,
        source: &(impl FrameSource<N> + ?Sized),
        context: &Context<N>,
    ) -> Option<Context<N>> {
        let return_column = usize::try_from(self.return_address_register).ok()?;
        if *self.rules.registers.get(return_column)? == RegisterRule::Undefined {
            return Some(Context::new([None; N], 0, false));
        }

        let cfa = context.cfa;
        let mut registers = [None; N];
        for (index, (slot, rule)) in registers.iter_mut().zip(self.rules.registers).enumerate() {
            let register = index as u64;
            *slot = match rule {
                RegisterRule::Undefined => None,
                RegisterRule::SameValue => context.get_gr(register),
                RegisterRule::Offset(offset) => {
                    Some(source.read_saved_register(register, cfa.checked_add_signed(offset)?)?)
                }
                RegisterRule::ValOffset(offset) => Some(cfa.checked_add_signed(offset)?),
                RegisterRule::Register(other) => context.get_gr(other),
                RegisterRule::Expression(expression) => {
                    let address = source.evaluate_expression(expression, context, Some(cfa))?;
                    Some(source.read_saved_register(register, address)?)
                }
                RegisterRule::ValExpression(expression) => {
                    Some(source.evaluate_expression(expression, context, Some(cfa))?)
                }
            };
        }
        let ip = registers[return_column]?;
        let caller = Context::new(registers, ip, self.signal_frame);
        let sp = source.stack_pointer_register();
        // Reject a row that leaves the cursor on the same instruction and stack slot.
        if ip != 0
            && caller.get_ip_info() == context.get_ip_info()
            && caller.get_gr(sp) == context.get_gr(sp)
        {
            return None;
        }
        Some(caller)
    }
}

/// Run phase 1, then start phase 2 from the original exception context.
pub fn raise_exception<const N: usize>(
    source: &(impl FrameSource<N> + ?Sized),
    start: Context<N>,
    exception: &mut Exception<N>,
) -> Result<Context<N>, ReasonCode> {
    exception.continuation = None;
    let mut context = start;
    while context.ip != 0 {
        let frame = frame_at(source, &mut context).ok_or(ReasonCode::FatalPhase1Error)?;
        if let Some(address) = frame.personality {
            let mut search_context = context;
            match source.personality(
                address,
                VERSION,
                Actions::SEARCH_PHASE,
                exception,
                &mut search_context,
            ) {
                ReasonCode::HandlerFound => {
                    let handler = Continuation::Handler {
                        ip: Some(context.ip),
                        cfa: context.cfa,
                    };
                    return cleanup(source, start, exception, handler);
                }
                ReasonCode::ContinueUnwind => {}
                _ => return Err(ReasonCode::FatalPhase1Error),
            }
        }
        context = frame
            .caller(source, &context)
            .ok_or(ReasonCode::FatalPhase1Error)?;
    }
    Err(ReasonCode::EndOfStack)
}

/// Run cleanup under the stop callback's control. A target stop callback
/// transfers control itself when it reaches its destination.
pub fn forced_unwind<const N: usize>(
    source: &(impl FrameSource<N> + ?Sized),
    start: Context<N>,
    exception: &mut Exception<N>,
    stop: StopFn<N>,
    parameter: usize,
) -> Result<Context<N>, ReasonCode> {
    exception.continuation = None;
    cleanup(
        source,
        start,
        exception,
        Continuation::Forced { stop, parameter },
    )
}

/// Continue phase 2 from a fresh capture of the cleanup landing pad's frame.
/// The pad's own frame is stepped past before any callback is made.
pub fn resume<const N: usize>(
    source: &(impl FrameSource<N> + ?Sized),
    mut landing_pad: Context<N>,
    exception: &mut Exception<N>,
) -> Result<Context<N>, ReasonCode> {
    let continuation = exception
        .continuation
        .take()
        .ok_or(ReasonCode::FatalPhase2Error)?;
    let frame = frame_at(source, &mut landing_pad).ok_or(ReasonCode::FatalPhase2Error)?;
    let caller = frame
        .caller(source, &landing_pad)
        .ok_or(ReasonCode::FatalPhase2Error)?;
    cleanup(source, caller, exception, continuation)
}

fn cleanup<const N: usize>(
    source: &(impl FrameSource<N> + ?Sized),
    mut context: Context<N>,
    exception: &mut Exception<N>,
    continuation: Continuation<N>,
) -> Result<Context<N>, ReasonCode> {
    let forced = Actions::CLEANUP_PHASE | Actions::FORCE_UNWIND;
    while context.ip != 0 {
        let frame = frame_at(source, &mut context).ok_or(ReasonCode::FatalPhase2Error)?;
        let handler = matches!(continuation, Continuation::Handler { ip, cfa }
            if cfa == context.cfa && ip.is_none_or(|ip| ip == context.ip));
        let actions = match continuation {
            Continuation::Forced { stop, parameter } => {
                if stop(VERSION, forced, exception, &context, parameter) != ReasonCode::NoReason {
                    return Err(ReasonCode::FatalPhase2Error);
                }
                forced
            }
            _ if handler => Actions::CLEANUP_PHASE | Actions::HANDLER_FRAME,
            _ => Actions::CLEANUP_PHASE,
        };

        let mut landing_pad = context;
        let result = frame
            .personality
            .map_or(ReasonCode::ContinueUnwind, |address| {
                source.personality(address, VERSION, actions, exception, &mut landing_pad)
            });
        match result {
            ReasonCode::InstallContext => {
                exception.continuation = (!handler).then_some(continuation);
                return Ok(landing_pad);
            }
            ReasonCode::ContinueUnwind if !handler => {}
            _ => return Err(ReasonCode::FatalPhase2Error),
        }

        context = frame
            .caller(source, &context)
            .ok_or(ReasonCode::FatalPhase2Error)?;
    }

    if let Continuation::Forced { stop, parameter } = continuation {
        let mut end = Context::new([None; N], 0, false);
        end.set_gr(source.stack_pointer_register(), 0);
        stop(
            VERSION,
            forced | Actions::END_OF_STACK,
            exception,
            &end,
            parameter,
        );
    }
    Err(ReasonCode::FatalPhase2Error)
}

/// Notify the owning runtime that it may destroy a caught foreign exception.
pub fn delete_exception<const N: usize>(exception: &mut Exception<N>) {
    exception.continuation = None;
    if let Some(cleanup) = exception.cleanup.take() {
        cleanup(ReasonCode::ForeignExceptionCaught, exception);
    }
}

/// Visit each frame without running its personality (LSB Core 5.0 §14).
/// A nonzero callback result stops the trace and is returned to the caller.
pub fn backtrace<const N: usize>(
    source: &(impl FrameSource<N> + ?Sized),
    mut context: Context<N>,
    mut trace: impl FnMut(&Context<N>) -> ReasonCode,
) -> ReasonCode {
    while context.ip != 0 {
        let Some(frame) = frame_at(source, &mut context) else {
            return ReasonCode::FatalPhase1Error;
        };
        let result = trace(&context);
        if result != ReasonCode::NoReason {
            return result;
        }
        let Some(caller) = frame.caller(source, &context) else {
            return ReasonCode::FatalPhase1Error;
        };
        context = caller;
    }
    ReasonCode::EndOfStack
}
