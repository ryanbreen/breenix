//! Itanium C++ ABI, Exception Handling Level I (§§1.2–1.6), and LSB Core
//! 5.0, Generic Part, Table 14-56 and §§14.13–14.14 (unwind.h).
//! The assembly entries capture their ABI caller before a Rust prologue
//! can change any nonvolatile register. Context pointers are opaque to C.

use core::ffi::{c_int, c_void};
use core::{mem, ptr};

use crate::arch::{self, NativeContext, RegisterContext, REGISTER_COUNT};
use crate::ehframe;
use crate::level1::{self, Actions, Continuation, Exception, Frame, FrameSource, ReasonCode};
use crate::loaded::LoadedObjects;

type ExceptionState = Exception<REGISTER_COUNT>;
type Cleanup = unsafe extern "C" fn(c_int, *mut UnwindException);
type Personality =
    unsafe extern "C" fn(c_int, c_int, u64, *mut UnwindException, *mut c_void) -> c_int;
type Stop = unsafe extern "C" fn(
    c_int,
    c_int,
    u64,
    *mut UnwindException,
    *mut c_void,
    *mut c_void,
) -> c_int;
type Trace = unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int;

// The ABI exception header is 16-byte aligned on both LP64 targets.
#[repr(C, align(16))]
pub(crate) struct UnwindException {
    class: u64,
    cleanup: Option<Cleanup>,
    private: [usize; 2],
}

struct Runtime {
    exception: *mut UnwindException,
}

impl FrameSource<REGISTER_COUNT> for Runtime {
    type Error = level1::LookupError;

    fn find_frame(&self, pc: u64) -> Result<Option<Frame<'_, REGISTER_COUNT>>, Self::Error> {
        let defaults = arch::initial_rules();
        // Caller-owned workspace for nested DW_CFA_remember_state rules.
        let mut saved = [defaults; 8];
        Frame::lookup(&LoadedObjects, pc, defaults, &mut saved)
    }

    fn stack_pointer_register(&self) -> u64 {
        arch::SP as u64
    }

    fn read_saved_register(&self, register: u64, address: u64) -> Option<u64> {
        if address == 0 || !arch::is_captured_register(register as usize) {
            return None;
        }
        let saved = ptr::with_exposed_provenance::<u64>(address as usize);
        // SAFETY: the ABI entry walks a live stack using trusted compiler CFI.
        // A saved scalar register may occupy an unaligned stack slot.
        Some(unsafe { saved.read_unaligned() })
    }

    fn evaluate_expression(
        &self,
        _expression: &[u8],
        _context: &NativeContext,
        _cfa: Option<u64>,
    ) -> Option<u64> {
        // The static hosted targets use register and offset rules. Reject
        // expression-based rows until there is a native expression evaluator.
        None
    }

    fn personality(
        &self,
        address: u64,
        version: u32,
        actions: Actions,
        exception: &mut ExceptionState,
        context: &mut NativeContext,
    ) -> ReasonCode {
        if !LoadedObjects::is_code(address) {
            return ReasonCode::FatalPhase2Error;
        }
        // SAFETY: the trusted CIE identifies an ABI personality in an
        // executable load of a resident object.
        let call = unsafe { mem::transmute::<usize, Personality>(address as usize) };
        // SAFETY: the opaque context is live for the call and the ABI caller
        // supplied the original exception object.
        let result = unsafe {
            call(
                version as c_int,
                actions.bits(),
                exception.class(),
                self.exception,
                ptr::from_mut(context).cast(),
            )
        };
        reason(result)
    }
}

fn reason(value: c_int) -> ReasonCode {
    match value {
        0 => ReasonCode::NoReason,
        1 => ReasonCode::ForeignExceptionCaught,
        2 => ReasonCode::FatalPhase2Error,
        3 => ReasonCode::FatalPhase1Error,
        4 => ReasonCode::NormalStop,
        5 => ReasonCode::EndOfStack,
        6 => ReasonCode::HandlerFound,
        7 => ReasonCode::InstallContext,
        8 => ReasonCode::ContinueUnwind,
        _ => ReasonCode::FatalPhase2Error,
    }
}

#[derive(Clone, Copy)]
struct ForcedCall {
    exception: *mut UnwindException,
    stop: Stop,
    parameter: *mut c_void,
}

fn call_stop(
    version: u32,
    actions: Actions,
    exception: &mut ExceptionState,
    context: &NativeContext,
    state: usize,
) -> ReasonCode {
    let state = ptr::with_exposed_provenance::<ForcedCall>(state);
    // SAFETY: The active entry stores its live ForcedCall as the adapter's
    // parameter. Resume reconstructs it before calling the Level I engine.
    let state = unsafe { *state };
    let mut context = *context;
    // SAFETY: the ABI supplied the stop function, exception and parameter.
    // The local copy of the opaque context lives through the callback.
    let result = unsafe {
        (state.stop)(
            version as c_int,
            actions.bits(),
            exception.class(),
            state.exception,
            ptr::from_mut(&mut context).cast(),
            state.parameter,
        )
    };
    if result != 0 {
        // LSB §14.14: a returning stop error calls cleanup, then aborts.
        // SAFETY: if the stop returns, its exception object is still live.
        unsafe { notify_cleanup(state.exception, result) };
        crate::abort();
    }
    ReasonCode::NoReason
}

unsafe fn notify_cleanup(exception: *mut UnwindException, result: c_int) {
    // SAFETY: the caller provides a live exception header.
    let cleanup = unsafe { (*exception).cleanup };
    if let Some(cleanup) = cleanup {
        // SAFETY: the ABI exception owner supplied this cleanup callback.
        unsafe { cleanup(result, exception) };
    }
}

impl Runtime {
    unsafe fn exception_state(&self) -> ExceptionState {
        // SAFETY: the caller supplies a live exception header.
        let class = unsafe { (*self.exception).class };
        ExceptionState::new(class, None)
    }

    unsafe fn finish(
        &self,
        result: Result<NativeContext, ReasonCode>,
        state: &ExceptionState,
        forced: Option<ForcedCall>,
    ) -> ReasonCode {
        let context = match result {
            Ok(context) => context,
            Err(error) => return error,
        };
        let Some(landing_pad) = RegisterContext::landing_pad(&context) else {
            return ReasonCode::FatalPhase2Error;
        };

        // The two ABI-private words hold either a handler's CFA or the
        // external stop function and its untouched opaque parameter.
        let private = match state.continuation {
            Some(Continuation::Handler { cfa, .. }) => [0, cfa as usize],
            Some(Continuation::Forced { .. }) => {
                let Some(forced) = forced else {
                    return ReasonCode::FatalPhase2Error;
                };
                [forced.stop as usize, forced.parameter.expose_provenance()]
            }
            None => [0, 0],
        };
        // SAFETY: the original exception is live and the ABI reserves
        // these words for the unwinder.
        unsafe { (*self.exception).private = private };
        // SAFETY: phase 2 selected a live frame and its personality supplied
        // the instruction pointer for the landing pad in that frame.
        unsafe { arch::restore_context(&landing_pad) }
    }
}

unsafe extern "C" fn raise(
    captured: &RegisterContext,
    exception: *mut UnwindException,
) -> ReasonCode {
    if exception.is_null() {
        return ReasonCode::FatalPhase1Error;
    }
    let runtime = Runtime { exception };
    // SAFETY: the ABI entry requires the original live exception header.
    let mut state = unsafe { runtime.exception_state() };
    // SAFETY: the two private words belong to this new invocation.
    unsafe { (*exception).private = [0, 0] };
    let result = level1::raise_exception(&runtime, captured.cursor(), &mut state);
    // SAFETY: the exception remains live through the unwind or error return.
    unsafe { runtime.finish(result, &state, None) }
}

unsafe extern "C" fn forced_unwind(
    captured: &RegisterContext,
    exception: *mut UnwindException,
    stop: Option<Stop>,
    parameter: *mut c_void,
) -> ReasonCode {
    let Some(stop) = stop.filter(|_| !exception.is_null()) else {
        return ReasonCode::FatalPhase2Error;
    };
    let runtime = Runtime { exception };
    let forced = ForcedCall {
        exception,
        stop,
        parameter,
    };
    // SAFETY: the ABI entry requires the original live exception header.
    let mut state = unsafe { runtime.exception_state() };
    // SAFETY: the two private words belong to this new invocation.
    unsafe { (*exception).private = [0, 0] };
    let result = level1::forced_unwind(
        &runtime,
        captured.cursor(),
        &mut state,
        call_stop,
        ptr::from_ref(&forced).expose_provenance(),
    );
    // SAFETY: the caller's exception remains live, and forced stores the
    // same stop function and parameter just passed to the engine.
    unsafe { runtime.finish(result, &state, Some(forced)) }
}

unsafe extern "C" fn resume(captured: &RegisterContext, exception: *mut UnwindException) -> ! {
    if exception.is_null() {
        crate::abort();
    }
    let runtime = Runtime { exception };
    // SAFETY: the caller is the cleanup pad for this live exception object.
    let mut state = unsafe { runtime.exception_state() };
    // SAFETY: the previous phase 2 transfer stored its continuation here.
    let [stop, word] = unsafe { (*exception).private };
    let forced = if stop == 0 {
        state.continuation = Some(Continuation::Handler {
            ip: None,
            cfa: word as u64,
        });
        None
    } else {
        // SAFETY: finish put the original ABI stop function in this word.
        let stop = unsafe { mem::transmute::<usize, Stop>(stop) };
        Some(ForcedCall {
            exception,
            stop,
            parameter: ptr::with_exposed_provenance_mut(word),
        })
    };
    if let Some(forced) = &forced {
        state.continuation = Some(Continuation::Forced {
            stop: call_stop,
            parameter: ptr::from_ref(forced).expose_provenance(),
        });
    }
    let result = level1::resume(&runtime, captured.cursor(), &mut state);
    // SAFETY: the ABI caller's exception remains live through this cleanup.
    unsafe { runtime.finish(result, &state, forced) };
    crate::abort()
}

unsafe extern "C" fn resume_or_rethrow(
    captured: &RegisterContext,
    exception: *mut UnwindException,
) -> ReasonCode {
    if exception.is_null() {
        return ReasonCode::FatalPhase1Error;
    }
    // SAFETY: the caller provides a live exception previously raised here.
    let [stop, _] = unsafe { (*exception).private };
    if stop != 0 {
        // SAFETY: a nonzero first word identifies a forced cleanup saved by finish.
        unsafe { resume(captured, exception) }
    } else {
        // SAFETY: the ABI permits reraising the original, still live exception.
        unsafe { raise(captured, exception) }
    }
}

extern "C" fn backtrace(
    captured: &RegisterContext,
    trace: Option<Trace>,
    parameter: *mut c_void,
) -> ReasonCode {
    let Some(trace) = trace else {
        return ReasonCode::FatalPhase1Error;
    };
    let runtime = Runtime {
        exception: ptr::null_mut(),
    };
    level1::backtrace(&runtime, captured.cursor(), |context| {
        let mut context = *context;
        // SAFETY: the ABI caller supplied the trace callback and parameter.
        // The opaque copy of the context lives through the callback.
        reason(unsafe { trace(ptr::from_mut(&mut context).cast(), parameter) })
    })
}

macro_rules! captured_entry {
    ($entry:ident($($arg:ident: $ty:ty),*) -> $ret:ty, $worker:path) => {
        #[no_mangle]
        #[unsafe(naked)]
        pub(crate) unsafe extern "C" fn $entry($($arg: $ty),*) -> $ret {
            arch::entry_asm!($worker)
        }
    };
}

captured_entry!(_Unwind_RaiseException(_object: *mut UnwindException) -> ReasonCode, raise);
captured_entry!(_Unwind_Resume(_object: *mut UnwindException) -> !, resume);
captured_entry!(_Unwind_Resume_or_Rethrow(_object: *mut UnwindException) -> ReasonCode, resume_or_rethrow);
captured_entry!(_Unwind_ForcedUnwind(_object: *mut UnwindException, _stop: Option<Stop>, _parameter: *mut c_void) -> ReasonCode, forced_unwind);
captured_entry!(_Unwind_Backtrace(_trace: Option<Trace>, _parameter: *mut c_void) -> ReasonCode, backtrace);

#[no_mangle]
pub(crate) unsafe extern "C" fn _Unwind_DeleteException(exception: *mut UnwindException) {
    if !exception.is_null() {
        // SAFETY: the caller supplies the original live exception to delete.
        unsafe { notify_cleanup(exception, ReasonCode::ForeignExceptionCaught as c_int) };
    }
}

#[no_mangle]
pub(crate) extern "C" fn _Unwind_FindEnclosingFunction(ip: *mut c_void) -> *mut c_void {
    ehframe::find_fde(&LoadedObjects, ip.expose_provenance() as u64)
        .ok()
        .flatten()
        .map_or(ptr::null_mut(), |fde| {
            ptr::with_exposed_provenance_mut(fde.pc_range.start as usize)
        })
}

#[no_mangle]
pub(crate) unsafe extern "C" fn _Unwind_GetGR(context: *const c_void, index: c_int) -> u64 {
    // SAFETY: the callback supplies the opaque handle to a live NativeContext.
    let context = unsafe { &*context.cast::<NativeContext>() };
    context.get_gr(index as u64).unwrap_or(0)
}

#[no_mangle]
pub(crate) unsafe extern "C" fn _Unwind_SetGR(context: *mut c_void, index: c_int, value: u64) {
    // SAFETY: the personality has exclusive access to its landing-pad context.
    let context = unsafe { &mut *context.cast::<NativeContext>() };
    context.set_gr(index as u64, value);
}

#[no_mangle]
pub(crate) unsafe extern "C" fn _Unwind_GetIP(context: *const c_void) -> u64 {
    // SAFETY: the callback supplies the opaque handle to a live NativeContext.
    let context = unsafe { &*context.cast::<NativeContext>() };
    context.get_ip()
}

#[no_mangle]
pub(crate) unsafe extern "C" fn _Unwind_GetIPInfo(
    context: *const c_void,
    ip_before_insn: *mut c_int,
) -> u64 {
    // SAFETY: the callback supplies the opaque handle to a live NativeContext.
    let context = unsafe { &*context.cast::<NativeContext>() };
    let (ip, before_insn) = context.get_ip_info();
    // SAFETY: the caller supplies a writable int for the output flag.
    unsafe { ip_before_insn.write(c_int::from(before_insn)) };
    ip
}

#[no_mangle]
pub(crate) unsafe extern "C" fn _Unwind_SetIP(context: *mut c_void, ip: u64) {
    // SAFETY: the personality has exclusive access to its landing-pad context.
    let context = unsafe { &mut *context.cast::<NativeContext>() };
    context.set_ip(ip);
}

#[no_mangle]
pub(crate) unsafe extern "C" fn _Unwind_GetCFA(context: *const c_void) -> u64 {
    // SAFETY: the callback supplies the opaque handle to a live NativeContext.
    let context = unsafe { &*context.cast::<NativeContext>() };
    context.get_cfa()
}

#[no_mangle]
pub(crate) unsafe extern "C" fn _Unwind_GetLanguageSpecificData(
    context: *const c_void,
) -> *mut c_void {
    // SAFETY: the callback supplies the opaque handle to a live NativeContext.
    let context = unsafe { &*context.cast::<NativeContext>() };
    ptr::with_exposed_provenance_mut(context.get_language_specific_data().unwrap_or(0) as usize)
}

#[no_mangle]
pub(crate) unsafe extern "C" fn _Unwind_GetRegionStart(context: *const c_void) -> u64 {
    // SAFETY: the callback supplies the opaque handle to a live NativeContext.
    let context = unsafe { &*context.cast::<NativeContext>() };
    context.get_region_start()
}

#[no_mangle]
pub(crate) unsafe extern "C" fn _Unwind_GetTextRelBase(context: *const c_void) -> u64 {
    // SAFETY: the callback supplies a live unwind context.
    let context = unsafe { &*context.cast::<NativeContext>() };
    context.get_text_base()
}

#[no_mangle]
pub(crate) unsafe extern "C" fn _Unwind_GetDataRelBase(context: *const c_void) -> u64 {
    // SAFETY: the callback supplies a live unwind context.
    let context = unsafe { &*context.cast::<NativeContext>() };
    context.get_data_base()
}
