//! Linear scan over the scalar operations recorded by the AMD64 emitter.

use std::collections::HashMap;
use std::ops::Range;

use super::{Emitter, Layout, Position, Slot, Type};
use crate::pp::Diagnostic;
use std::fmt::Write;

// AMD64 psABI §3.2.1, Fig. 3.4. R8/R9 are otherwise used only when
// receiving or staging arguments. Keep R15 reserved for aligned frames.
const CALLER_SAVED: [&str; 2] = ["r8", "r9"];
const CALLEE_SAVED: [&str; 4] = ["rbx", "r12", "r13", "r14"];
const REGISTER_COUNT: usize = CALLER_SAVED.len() + CALLEE_SAVED.len();
const WORD: Layout = Layout { size: 8, align: 8 };

pub(super) type ValueId = usize;

pub(super) enum Access {
    Read,
    Write,
    Address,
}

enum Operation {
    Value(ValueId, Access),
    LoadParameter(ValueId),
    SaveCallers,
    RestoreCallers,
}

struct Event {
    offset: usize,
    operation: Operation,
}

struct Value {
    ty: Option<Type>,
    layout: Layout,
    parameter: Option<Parameter>,
    interval: Range<usize>,
    address_taken: bool,
}

struct Parameter {
    definition: usize,
    slot: Slot,
}

impl Value {
    fn touch(&mut self, point: usize) {
        if self.interval.is_empty() {
            self.interval.start = self
                .parameter
                .as_ref()
                .map_or(point, |param| param.definition);
        }
        self.interval.end = point + 1;
    }
}

#[derive(Default)]
pub(super) struct Scan {
    values: Vec<Value>,
    events: Vec<Event>,
    calls: Vec<usize>,
    loops: Vec<Range<usize>>,
    labels: HashMap<usize, usize>,
    jumps: Vec<(usize, usize)>,
}

impl Scan {
    pub(super) fn local(&mut self, ty: &Type, layout: Layout) -> ValueId {
        self.value(Some(ty.clone()), layout)
    }

    pub(super) fn temporary(&mut self, offset: usize) -> ValueId {
        let id = self.value(None, WORD);
        self.access(offset, id, Access::Write);
        id
    }

    fn value(&mut self, ty: Option<Type>, layout: Layout) -> ValueId {
        let id = self.values.len();
        self.values.push(Value {
            ty,
            layout,
            parameter: None,
            interval: 0..0,
            address_taken: false,
        });
        id
    }

    pub(super) fn point(&self) -> usize {
        self.events.len()
    }

    pub(super) fn loop_back(&mut self, start: usize) {
        self.loops.push(start..self.point());
    }

    pub(super) fn label(&mut self, id: usize) {
        self.labels.insert(id, self.point());
    }

    pub(super) fn jump(&mut self, id: usize) {
        self.jumps.push((id, self.point()));
    }

    pub(super) fn access(&mut self, offset: usize, id: ValueId, access: Access) {
        let point = self.point();
        let value = &mut self.values[id];
        value.touch(point);
        value.address_taken |= matches!(access, Access::Address);
        self.push(offset, Operation::Value(id, access));
    }

    pub(super) fn parameter(&mut self, offset: usize, id: ValueId, slot: Slot) {
        self.values[id].parameter = Some(Parameter {
            definition: self.point(),
            slot,
        });
        // Start the interval at the incoming definition only when a body
        // operation touches the parameter. Load registers after capturing R8/R9.
        self.push(offset, Operation::LoadParameter(id));
    }

    pub(super) fn before_call(&mut self, offset: usize) {
        self.calls.push(self.point());
        self.push(offset, Operation::SaveCallers);
    }

    pub(super) fn after_call(&mut self, offset: usize) {
        self.push(offset, Operation::RestoreCallers);
    }

    fn push(&mut self, offset: usize, operation: Operation) {
        self.events.push(Event { offset, operation });
    }

    fn extend_intervals(&mut self) {
        // A forward jump needs no extension. Treat a backward goto as a
        // loop as well, including overlapping or nested back edges.
        for &(id, end) in &self.jumps {
            if let Some(&start) = self.labels.get(&id).filter(|&&start| start < end) {
                self.loops.push(start..end);
            }
        }
        self.loops.sort_by_key(|range| range.start);
        let mut loops: Vec<Range<usize>> = Vec::new();
        for range in self.loops.drain(..).filter(|range| !range.is_empty()) {
            if let Some(previous) = loops.last_mut().filter(|last| last.end > range.start) {
                previous.end = previous.end.max(range.end);
            } else {
                loops.push(range);
            }
        }

        for value in &mut self.values {
            // Expression temporaries are produced and consumed within one
            // evaluation. Only mutable C objects can carry state on a back edge.
            if value.ty.is_none() || value.interval.is_empty() {
                continue;
            }
            let first = loops.partition_point(|range| range.end <= value.interval.start);
            let last = loops.partition_point(|range| range.start < value.interval.end);
            if first < last {
                value.interval.start = value.interval.start.min(loops[first].start);
                value.interval.end = value.interval.end.max(loops[last - 1].end);
            }
        }
    }

    fn allocate(&self, order: &[ValueId]) -> Vec<Option<usize>> {
        let mut registers = vec![None; self.values.len()];
        let mut active: [Option<ValueId>; REGISTER_COUNT] = [None; REGISTER_COUNT];
        for &id in order {
            let interval = &self.values[id].interval;
            for slot in &mut active {
                if slot.is_some_and(|other| self.values[other].interval.end <= interval.start) {
                    *slot = None;
                }
            }
            let register = if let Some(free) = active.iter().position(Option::is_none) {
                free
            } else {
                let (register, spill) = active
                    .iter()
                    .enumerate()
                    .map(|(register, other)| (register, other.unwrap()))
                    .max_by_key(|&(_, other)| self.values[other].interval.end)
                    .unwrap();
                if self.values[spill].interval.end <= interval.end {
                    continue;
                }
                // Assign the displaced interval to its stack slot throughout.
                // This also works at a join that bypasses the eviction point.
                registers[spill] = None;
                register
            };
            active[register] = Some(id);
            registers[id] = Some(register);
        }
        registers
    }

    pub(super) fn render(
        mut self,
        emitter: &mut Emitter<'_, '_>,
        start: usize,
        site: &Position,
    ) -> Result<Vec<(&'static str, Slot)>, Diagnostic> {
        self.extend_intervals();
        let mut order: Vec<_> = (0..self.values.len())
            .filter(|&id| !self.values[id].interval.is_empty() && !self.values[id].address_taken)
            .collect();
        order.sort_by_key(|&id| self.values[id].interval.start);
        let registers = self.allocate(&order);
        let mut saved = [false; CALLEE_SAVED.len()];
        let mut homes = Vec::with_capacity(self.values.len());
        for (value, &register) in self.values.iter().zip(&registers) {
            if value.interval.is_empty() {
                homes.push(None);
                continue;
            }
            let mut home = Home {
                register: None,
                stack: None,
            };
            if let Some(register) = register {
                if register < CALLER_SAVED.len() {
                    home.register = Some(CALLER_SAVED[register]);
                    let first_call = self
                        .calls
                        .partition_point(|&call| call < value.interval.start);
                    if self
                        .calls
                        .get(first_call)
                        .is_some_and(|&call| call < value.interval.end)
                    {
                        home.stack = Some(stack_home(emitter, value, WORD, site)?);
                    }
                } else {
                    let index = register - CALLER_SAVED.len();
                    saved[index] = true;
                    home.register = Some(CALLEE_SAVED[index]);
                }
            } else {
                home.stack = Some(stack_home(emitter, value, value.layout, site)?);
            }
            homes.push(Some(home));
        }

        let callee_saves = CALLEE_SAVED
            .into_iter()
            .zip(saved)
            .filter(|&(_, used)| used)
            .map(|(reg, _)| Ok((reg, emitter.frame_slot(WORD, site)?)))
            .collect::<Result<_, Diagnostic>>()?;

        let body = emitter.text.split_off(start);
        let mut cursor = 0;
        let mut active = [None; CALLER_SAVED.len()];
        let mut upcoming = order.into_iter().peekable();
        for (point, event) in self.events.into_iter().enumerate() {
            while upcoming
                .peek()
                .is_some_and(|&id| self.values[id].interval.start <= point)
            {
                let id = upcoming.next().unwrap();
                if let Some(reg) = registers[id].filter(|&reg| reg < CALLER_SAVED.len()) {
                    active[reg] = Some(id);
                }
            }
            let offset = event.offset - start;
            emitter.text.push_str(&body[cursor..offset]);
            cursor = offset;
            match event.operation {
                Operation::Value(id, access) => {
                    let home = homes[id].as_ref().unwrap();
                    home.emit(emitter, self.values[id].ty.as_ref(), access);
                }
                Operation::LoadParameter(id) => {
                    if let Some(reg) = homes[id].as_ref().and_then(|home| home.register) {
                        let slot = &self.values[id].parameter.as_ref().unwrap().slot;
                        writeln!(emitter.text, "    mov {reg}, qword {}", slot.memory(0)).unwrap();
                    }
                }
                Operation::SaveCallers | Operation::RestoreCallers => {
                    for id in active.into_iter().flatten() {
                        if point < self.values[id].interval.end {
                            let home = homes[id].as_ref().unwrap();
                            let reg = home.register.unwrap();
                            let slot = home.stack.as_ref().unwrap().memory(0);
                            if matches!(event.operation, Operation::SaveCallers) {
                                writeln!(emitter.text, "    mov qword {slot}, {reg}").unwrap();
                            } else {
                                writeln!(emitter.text, "    mov {reg}, qword {slot}").unwrap();
                            }
                        }
                    }
                }
            }
        }
        emitter.text.push_str(&body[cursor..]);
        Ok(callee_saves)
    }
}

fn stack_home(
    emitter: &mut Emitter<'_, '_>,
    value: &Value,
    layout: Layout,
    site: &Position,
) -> Result<Slot, Diagnostic> {
    if let Some(param) = &value.parameter {
        Ok(param.slot.clone())
    } else {
        emitter.frame_slot(layout, site)
    }
}

struct Home {
    register: Option<&'static str>,
    stack: Option<Slot>,
}

impl Home {
    fn emit(&self, emitter: &mut Emitter<'_, '_>, ty: Option<&Type>, access: Access) {
        if let Some(reg) = self.register {
            match access {
                Access::Read => {
                    writeln!(emitter.text, "    mov rax, {reg}").unwrap();
                    if let Some(ty) = ty {
                        emitter.normalize_integer(ty);
                    }
                }
                Access::Write => writeln!(emitter.text, "    mov {reg}, rax").unwrap(),
                Access::Address => unreachable!("addressable values have a stack home"),
            }
            return;
        }
        let memory = self.stack.as_ref().unwrap().memory(0);
        match (access, ty) {
            (Access::Read, Some(ty)) => emitter.load(ty, &memory),
            (Access::Write, Some(ty)) => emitter.store_object(ty, &memory),
            (Access::Read, None) => writeln!(emitter.text, "    mov rax, qword {memory}").unwrap(),
            (Access::Write, None) => writeln!(emitter.text, "    mov qword {memory}, rax").unwrap(),
            (Access::Address, _) => writeln!(emitter.text, "    lea rax, {memory}").unwrap(),
        }
    }
}
