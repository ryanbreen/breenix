use crate::cfi::{CfaRule, Endian, Program, RegisterRule, Rules};
use core::cell::{Cell, RefCell};
use std::vec::Vec;

use crate::ehframe::{self, Memory, Section, Sections};
use crate::level1::{
    backtrace, delete_exception, forced_unwind, raise_exception, resume, Actions, Context,
    Exception, Frame, FrameSource, LookupError, ReasonCode,
};
use std::thread_local;

#[test]
fn cfa_rules_from_dwarf5_table_6_3() {
    // DWARF 5 Appendix D.6, Tables D.5 and D.6, encoded using Table 7.29.
    // D.6 specifies advance_loc(12) at foo+20, which reaches foo+68
    // at code alignment 4. The comment's "44/<caf>" does not match it.
    let cie = [
        0x0c, 7, 0, // def_cfa R7, 0
        0x08, 0, // same_value R0
        0x07, 1, // undefined R1
        0x07, 2, // undefined R2
        0x07, 3, // undefined R3
        0x08, 4, // same_value R4
        0x08, 5, // same_value R5
        0x08, 6, // same_value R6
        0x08, 7, // same_value R7
        0x09, 8, 1, // register R8, R1
        0, 0, 0, // padding
    ];
    let fde = [
        0x41, 0x0e, 12, // +4: def_cfa_offset 12
        0x41, 0x88, 1, // +8: offset R8, -4
        0x41, 0x86, 2, // +12: offset R6, -8
        0x41, 0x0d, 6, // +16: def_cfa_register R6
        0x41, 0x84, 3, // +20: offset R4, -12
        0x4c, 0xc4, // +68: restore R4
        0x41, 0xc6, 0x0d, 7, // +72: restore R6, def_cfa_register R7
        0x41, 0xc8, // +76: restore R8
        0x41, 0x0e, 0, // +80: def_cfa_offset 0
        0, 0, // padding
    ];
    let foo = 0x1000;
    let program = Program {
        cie: &cie,
        fde: &fde,
        initial_location: foo,
        address_range: 84,
        code_alignment: 4,
        data_alignment: -4,
        address_size: 4,
        endian: Endian::Little,
    };
    let mut actual = Vec::new();
    program
        .rows(Rules::default(), &mut [], |row| {
            actual.push((row.start - foo, row.end - foo, *row.rules));
        })
        .unwrap();

    use RegisterRule::{Offset, Register, SameValue as Same, Undefined as Undef};
    let expected_row = |start, end, register, offset, r4, r6, r8| {
        (
            start,
            end,
            Rules {
                cfa: Some(CfaRule::RegisterOffset { register, offset }),
                // D.4 describes R7 architecturally. D.5 sets its CFI column
                // to same_value, which remains in effect through the FDE.
                registers: [Same, Undef, Undef, Undef, r4, Same, r6, Same, r8],
            },
        )
    };
    assert_eq!(
        actual,
        [
            expected_row(0, 4, 7, 0, Same, Same, Register(1)),
            expected_row(4, 8, 7, 12, Same, Same, Register(1)),
            expected_row(8, 12, 7, 12, Same, Same, Offset(-4)),
            expected_row(12, 16, 7, 12, Same, Offset(-8), Offset(-4)),
            expected_row(16, 20, 6, 12, Same, Offset(-8), Offset(-4)),
            // Also covers the unchanged row at +64 shown in Table D.4.
            expected_row(20, 68, 6, 12, Offset(-12), Offset(-8), Offset(-4)),
            expected_row(68, 72, 6, 12, Same, Offset(-8), Offset(-4)),
            expected_row(72, 76, 7, 12, Same, Same, Offset(-4)),
            expected_row(76, 80, 7, 12, Same, Same, Register(1)),
            expected_row(80, 84, 7, 0, Same, Same, Register(1)),
        ]
    );
}

#[test]
fn fde_found_through_eh_frame_hdr() {
    use crate::ehframe::{Memory, Section, Sections};

    struct ProbeMemory;
    impl Memory for ProbeMemory {
        fn read(&self, _: u64, _: usize) -> Option<&[u8]> {
            None
        }
    }

    // The ranges, return columns and instructions are from the saved
    // llvm-dwarfdump output. DWARF 5 Table 7.29 gives the instruction bytes.
    let probes = [
        (
            &include_bytes!("../tests/fixtures/k1-hosted/x86_64.eh_frame")[..],
            include_bytes!("../tests/fixtures/k1-hosted/x86_64.eh_frame_hdr"),
            0x4001_d248,
            0x4001_d230,
            0x4000_4dcc..0x4000_4dcd,
            16,
            -8,
            &[0x0c, 7, 8, 0x90, 1, 0, 0][..], // CFA = RSP + 8, RIP at CFA - 8
        ),
        (
            &include_bytes!("../tests/fixtures/k1-hosted/aarch64.eh_frame")[..],
            include_bytes!("../tests/fixtures/k1-hosted/aarch64.eh_frame_hdr"),
            0x4001_e190,
            0x4001_e17c,
            0x4000_4f34..0x4000_4f38,
            30,
            -4,
            &[0x0c, 31, 0][..], // CFA = SP
        ),
    ];
    for (frame, header, frame_address, header_address, range, register, alignment, initial) in
        probes
    {
        let sections = Sections {
            frame: Section {
                address: frame_address,
                bytes: frame,
            },
            header: Some(Section {
                address: header_address,
                bytes: header,
            }),
            data_base: None,
            address_size: 8,
            endian: Endian::Little,
        };
        for pc in [range.start, range.end - 1] {
            let fde = sections.find_fde(&ProbeMemory, pc).unwrap().unwrap();
            assert_eq!(fde.pc_range, range);
            assert_eq!(fde.cie.return_address_register, register);
            assert_eq!(fde.cie.code_alignment, 1);
            assert_eq!(fde.cie.data_alignment, alignment);
            assert_eq!(fde.cie.initial_instructions, initial);
        }
        for pc in [range.start - 1, range.end] {
            assert!(sections.find_fde(&ProbeMemory, pc).unwrap().is_none());
        }
    }
}

const STACK: u64 = 0x8000;
const PERSONALITY: u64 = 0x9000;
const EXCEPTION_CLASS: u64 = 0x4252_4545_5445_5354;
const REGIONS: [u64; 4] = [0x1000, 0x1100, 0x1200, 0x1300];
const CLEANUP_PAD: u64 = 0x1120;
const HANDLER_PAD: u64 = 0x1220;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Event {
    Personality(u64, Actions),
    Stop(u64, Actions),
}

struct Chain {
    eh_frame: Vec<u8>,
    stack: [u8; 32],
    handler: bool,
}

thread_local! {
    static EVENTS: RefCell<Vec<Event>> = const { RefCell::new(Vec::new()) };
    static DELETED: Cell<Option<ReasonCode>> = const { Cell::new(None) };
}

impl Chain {
    fn new(handler: bool) -> Self {
        // DWARF 5 Table 7.29 and LSB Core 5.0 §10.6. A single CIE
        // describes a frame with CFA = R1 + 8 and the return PC at CFA - 8.
        fn record(section: &mut Vec<u8>, body: &[u8]) {
            section.extend((body.len() as u32).to_le_bytes());
            section.extend(body);
        }
        let mut cie = Vec::from([0, 0, 0, 0, 1, b'z', b'P', b'L', b'R', 0, 1, 0x78, 2]);
        cie.extend([11, 0]); // augmentation size and absolute personality encoding
        cie.extend(PERSONALITY.to_le_bytes());
        cie.extend([0, 0]); // absolute LSDA and code addresses
        cie.extend([0x0c, 1, 8, 0x82, 1, 0x08, 0]); // CFA, R2 offset, R0 same_value

        let mut eh_frame = Vec::new();
        record(&mut eh_frame, &cie);
        for region in REGIONS {
            let mut fde = Vec::from(((eh_frame.len() + 4) as u32).to_le_bytes());
            fde.extend(region.to_le_bytes());
            fde.extend(0x100u64.to_le_bytes());
            fde.push(8);
            fde.extend((region + 0x4000).to_le_bytes());
            record(&mut eh_frame, &fde);
        }

        let mut stack = [0; 32];
        // Each saved IP lies at the end of its caller's region.
        for (slot, pc) in stack
            .chunks_exact_mut(8)
            .zip([0x1200u64, 0x1300, 0x1400, 0])
        {
            slot.copy_from_slice(&pc.to_le_bytes());
        }
        Self {
            eh_frame,
            stack,
            handler,
        }
    }

    fn start() -> Context<3> {
        Context::new([Some(42), Some(STACK), None], 0x1010, true)
    }
}

impl Memory for Chain {
    fn read(&self, address: u64, size: usize) -> Option<&[u8]> {
        let offset = usize::try_from(address.checked_sub(STACK)?).ok()?;
        self.stack.get(offset..offset.checked_add(size)?)
    }
}

impl ehframe::FrameSource for Chain {
    fn sections_for_pc(&self, pc: u64) -> Option<Sections<'_>> {
        (0x1000..0x1400).contains(&pc).then_some(Sections {
            frame: Section {
                address: 0xa000,
                bytes: &self.eh_frame,
            },
            header: None,
            data_base: None,
            address_size: 8,
            endian: Endian::Little,
        })
    }
}

impl FrameSource<3> for Chain {
    type Error = LookupError;

    fn find_frame(&self, pc: u64) -> Result<Option<Frame<'_, 3>>, Self::Error> {
        let mut defaults = Rules::default();
        defaults.registers[1] = RegisterRule::ValOffset(0);
        Frame::lookup(self, pc, defaults, &mut [])
    }

    fn stack_pointer_register(&self) -> u64 {
        1
    }

    fn read_saved_register(&self, _register: u64, address: u64) -> Option<u64> {
        Some(u64::from_le_bytes(self.read(address, 8)?.try_into().ok()?))
    }

    fn evaluate_expression(&self, _: &[u8], _: &Context<3>, _: Option<u64>) -> Option<u64> {
        panic!("fixture has no DWARF expressions");
    }

    fn personality(
        &self,
        address: u64,
        version: u32,
        actions: Actions,
        exception: &mut Exception<3>,
        context: &mut Context<3>,
    ) -> ReasonCode {
        assert_eq!(address, PERSONALITY);
        assert_eq!(version, 1);
        assert_eq!(exception.class(), EXCEPTION_CLASS);
        let region = context.get_region_start();
        EVENTS.with_borrow_mut(|events| events.push(Event::Personality(region, actions)));
        assert_eq!(context.get_language_specific_data(), Some(region + 0x4000));
        assert_eq!(context.get_gr(0), Some(42));
        assert_eq!(
            context.get_gr(1),
            Some(STACK + (region - 0x1000) / 0x100 * 8)
        );
        assert_eq!(context.get_cfa(), context.get_gr(1).unwrap() + 8);
        if region == REGIONS[0] {
            assert_eq!(context.get_ip_info(), (0x1010, true));
        } else {
            assert_eq!(context.get_ip_info(), (region + 0x100, false));
        }

        if actions == Actions::SEARCH_PHASE {
            if self.handler && region == REGIONS[2] {
                return ReasonCode::HandlerFound;
            }
        } else if actions.contains(Actions::HANDLER_FRAME) {
            assert_eq!(region, REGIONS[2]);
            assert!(context.set_gr(0, 77));
            context.set_ip(HANDLER_PAD);
            return ReasonCode::InstallContext;
        } else if self.handler && region == REGIONS[1] {
            context.set_ip(CLEANUP_PAD);
            return ReasonCode::InstallContext;
        }
        ReasonCode::ContinueUnwind
    }
}

fn take_events() -> Vec<Event> {
    EVENTS.with_borrow_mut(core::mem::take)
}

#[test]
fn two_phase_search_then_cleanup_stops_at_handler() {
    use Actions as A;
    use Event::Personality;

    let chain = Chain::new(true);
    let mut exception = Exception::new(EXCEPTION_CLASS, None);
    let cleanup = raise_exception(&chain, Chain::start(), &mut exception).unwrap();
    assert_eq!(cleanup.get_ip(), CLEANUP_PAD);
    assert_eq!(cleanup.get_gr(1), Some(STACK + 8));
    assert_eq!(
        take_events(),
        [
            Personality(REGIONS[0], A::SEARCH_PHASE),
            Personality(REGIONS[1], A::SEARCH_PHASE),
            Personality(REGIONS[2], A::SEARCH_PHASE),
            Personality(REGIONS[0], A::CLEANUP_PHASE),
            Personality(REGIONS[1], A::CLEANUP_PHASE),
        ]
    );

    let handler = resume(&chain, cleanup, &mut exception).unwrap();
    assert_eq!(handler.get_ip(), HANDLER_PAD);
    assert_eq!(handler.get_gr(0), Some(77));
    assert_eq!(handler.get_gr(1), Some(STACK + 16));
    assert_eq!(
        take_events(),
        [Personality(REGIONS[2], A::CLEANUP_PHASE | A::HANDLER_FRAME)]
    );
}

#[test]
fn no_handler_returns_end_of_stack_and_backtrace_visits_each_frame() {
    let chain = Chain::new(false);
    let mut exception = Exception::new(
        EXCEPTION_CLASS,
        Some(|reason, exception| {
            assert_eq!(exception.class(), EXCEPTION_CLASS);
            DELETED.set(Some(reason));
        }),
    );
    assert_eq!(
        raise_exception(&chain, Chain::start(), &mut exception),
        Err(ReasonCode::EndOfStack)
    );
    assert_eq!(
        take_events(),
        REGIONS.map(|region| Event::Personality(region, Actions::SEARCH_PHASE))
    );

    let mut regions = Vec::new();
    assert_eq!(
        backtrace(&chain, Chain::start(), |context| {
            regions.push(context.get_region_start());
            ReasonCode::NoReason
        }),
        ReasonCode::EndOfStack
    );
    assert_eq!(regions, REGIONS);
    assert!(take_events().is_empty());
    delete_exception(&mut exception);
    assert_eq!(DELETED.take(), Some(ReasonCode::ForeignExceptionCaught));
}

#[test]
fn forced_unwind_calls_stop_before_personality_and_at_end() {
    use Actions as A;
    use Event::{Personality, Stop};

    fn stop(
        version: u32,
        actions: Actions,
        exception: &mut Exception<3>,
        context: &Context<3>,
        stop_parameter: usize,
    ) -> ReasonCode {
        assert_eq!(version, 1);
        assert_eq!(exception.class(), EXCEPTION_CLASS);
        assert_eq!(stop_parameter, 99);
        EVENTS.with_borrow_mut(|events| events.push(Stop(context.get_region_start(), actions)));
        if context.get_gr(1) == Some(0) {
            assert_eq!(context.get_ip(), 0);
            return ReasonCode::EndOfStack;
        }
        ReasonCode::NoReason
    }

    let chain = Chain::new(true);
    let mut exception = Exception::new(EXCEPTION_CLASS, None);
    let cleanup = forced_unwind(&chain, Chain::start(), &mut exception, stop, 99).unwrap();
    assert_eq!(cleanup.get_ip(), CLEANUP_PAD);
    let forced = A::CLEANUP_PHASE | A::FORCE_UNWIND;
    assert_eq!(
        take_events(),
        [
            Stop(REGIONS[0], forced),
            Personality(REGIONS[0], forced),
            Stop(REGIONS[1], forced),
            Personality(REGIONS[1], forced),
        ]
    );

    assert_eq!(
        resume(&chain, cleanup, &mut exception),
        Err(ReasonCode::FatalPhase2Error)
    );
    assert_eq!(
        take_events(),
        [
            Stop(REGIONS[2], forced),
            Personality(REGIONS[2], forced),
            Stop(REGIONS[3], forced),
            Personality(REGIONS[3], forced),
            Stop(0, forced | A::END_OF_STACK),
        ]
    );
}

include!("arch_test.rs");
