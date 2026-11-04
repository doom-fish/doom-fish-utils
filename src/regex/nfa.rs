#![allow(clippy::cast_possible_truncation)]

use super::hir::{Class, Hir, Look, WORD_LOOKS};
use super::utf8;
use std::collections::HashMap;

pub type StateId = u32;

const UNPATCHED: StateId = StateId::MAX;

#[derive(Clone, Debug)]
pub enum State {
    Range { lo: u8, hi: u8, next: StateId },
    Sparse(Box<[(u8, u8, StateId)]>),
    Look { look: Look, next: StateId },
    Union(Box<[StateId]>),
    Capture { slot: u32, next: StateId },
    Fail,
    Match,
}

#[derive(Clone, Debug)]
pub struct ByteClasses {
    pub map: [u8; 256],
    pub count: usize,
}

#[derive(Clone, Debug)]
pub struct Nfa {
    pub states: Vec<State>,
    pub start_anchored: StateId,
    pub start_unanchored: StateId,
    pub looks: u32,
    pub slots: usize,
    pub classes: ByteClasses,
}

#[derive(Debug)]
pub struct TooBig;

#[derive(Clone, Debug)]
enum Building {
    Empty(StateId),
    Range(u8, u8, StateId),
    Sparse(Vec<(u8, u8, StateId)>),
    Look(Look, StateId),
    Union(Vec<StateId>),
    UnionReverse(Vec<StateId>),
    Capture(u32, StateId),
    Fail,
    Match,
}

#[derive(Clone, Copy)]
struct Fragment {
    start: StateId,
    end: StateId,
}

struct Builder {
    states: Vec<Building>,
    memory: usize,
    limit: usize,
    reverse: bool,
    captures: bool,
}

impl Builder {
    fn add(&mut self, state: Building) -> Result<StateId, TooBig> {
        self.memory += match &state {
            Building::Sparse(ranges) => 24 + 12 * ranges.len(),
            Building::Union(alternatives) | Building::UnionReverse(alternatives) => {
                24 + 4 * alternatives.len()
            }
            _ => 24,
        };
        if self.memory > self.limit {
            return Err(TooBig);
        }
        let id = StateId::try_from(self.states.len()).map_err(|_| TooBig)?;
        if id == UNPATCHED {
            return Err(TooBig);
        }
        self.states.push(state);
        Ok(id)
    }

    fn patch(&mut self, from: StateId, to: StateId) -> Result<(), TooBig> {
        match &mut self.states[from as usize] {
            Building::Empty(next)
            | Building::Range(_, _, next)
            | Building::Look(_, next)
            | Building::Capture(_, next) => *next = to,
            Building::Union(alternatives) | Building::UnionReverse(alternatives) => {
                alternatives.push(to);
                self.memory += 4;
                if self.memory > self.limit {
                    return Err(TooBig);
                }
            }
            Building::Sparse(_) | Building::Fail | Building::Match => {}
        }
        Ok(())
    }

    fn empty(&mut self) -> Result<StateId, TooBig> {
        self.add(Building::Empty(UNPATCHED))
    }

    fn union(&mut self, greedy: bool) -> Result<StateId, TooBig> {
        self.add(if greedy {
            Building::Union(Vec::new())
        } else {
            Building::UnionReverse(Vec::new())
        })
    }

    fn compile(&mut self, hir: &Hir) -> Result<Fragment, TooBig> {
        match hir {
            Hir::Empty => {
                let id = self.empty()?;
                Ok(Fragment { start: id, end: id })
            }
            Hir::Literal(bytes) => self.literal(bytes),
            Hir::Class(class) => self.class(class),
            Hir::Look(look) => {
                let look = if self.reverse { look.reversed() } else { *look };
                let id = self.add(Building::Look(look, UNPATCHED))?;
                Ok(Fragment { start: id, end: id })
            }
            Hir::Capture { index, sub } => {
                if self.reverse || !self.captures {
                    return self.compile(sub);
                }
                let open = self.add(Building::Capture(index * 2, UNPATCHED))?;
                let inner = self.compile(sub)?;
                let close = self.add(Building::Capture(index * 2 + 1, UNPATCHED))?;
                self.patch(open, inner.start)?;
                self.patch(inner.end, close)?;
                Ok(Fragment {
                    start: open,
                    end: close,
                })
            }
            Hir::Concat(items) => {
                if self.reverse {
                    self.concat(items.iter().rev())
                } else {
                    self.concat(items.iter())
                }
            }
            Hir::Alternation(items) => {
                let union = self.union(true)?;
                let end = self.empty()?;
                for item in items {
                    let fragment = self.compile(item)?;
                    self.patch(union, fragment.start)?;
                    self.patch(fragment.end, end)?;
                }
                Ok(Fragment { start: union, end })
            }
            Hir::Repeat {
                min,
                max,
                greedy,
                sub,
            } => self.repeat(sub, *min, *max, *greedy),
        }
    }

    fn concat<'h>(&mut self, items: impl Iterator<Item = &'h Hir>) -> Result<Fragment, TooBig> {
        let mut result: Option<Fragment> = None;
        for item in items {
            let fragment = self.compile(item)?;
            result = Some(match result {
                None => fragment,
                Some(previous) => {
                    self.patch(previous.end, fragment.start)?;
                    Fragment {
                        start: previous.start,
                        end: fragment.end,
                    }
                }
            });
        }
        if let Some(fragment) = result {
            return Ok(fragment);
        }
        let id = self.empty()?;
        Ok(Fragment { start: id, end: id })
    }

    fn literal(&mut self, bytes: &[u8]) -> Result<Fragment, TooBig> {
        let mut result: Option<Fragment> = None;
        let mut push = |builder: &mut Self, byte: u8| -> Result<(), TooBig> {
            let id = builder.add(Building::Range(byte, byte, UNPATCHED))?;
            result = Some(match result {
                None => Fragment { start: id, end: id },
                Some(previous) => {
                    builder.patch(previous.end, id)?;
                    Fragment {
                        start: previous.start,
                        end: id,
                    }
                }
            });
            Ok(())
        };
        if self.reverse {
            for &byte in bytes.iter().rev() {
                push(self, byte)?;
            }
        } else {
            for &byte in bytes {
                push(self, byte)?;
            }
        }
        if let Some(fragment) = result {
            return Ok(fragment);
        }
        let id = self.empty()?;
        Ok(Fragment { start: id, end: id })
    }

    fn class(&mut self, class: &Class) -> Result<Fragment, TooBig> {
        let end = self.empty()?;
        if class.ranges.is_empty() {
            let fail = self.add(Building::Fail)?;
            return Ok(Fragment { start: fail, end });
        }
        if !class.unicode || class.is_ascii() {
            let ranges = class
                .ranges
                .iter()
                .map(|&(lo, hi)| (lo as u8, hi.min(0xFF) as u8, end))
                .collect();
            let start = self.add(Building::Sparse(ranges))?;
            return Ok(Fragment { start, end });
        }
        let mut sequences = Vec::new();
        for &(lo, hi) in &class.ranges {
            utf8::sequences(lo, hi, &mut sequences);
        }
        if self.reverse {
            self.reverse_utf8(&sequences, end)
        } else {
            self.forward_utf8(&sequences, end)
        }
    }

    fn reverse_utf8(&mut self, sequences: &[Vec<(u8, u8)>], end: StateId) -> Result<Fragment, TooBig> {
        let union = self.union(true)?;
        let mut cache: HashMap<(StateId, u8, u8), StateId> = HashMap::new();
        for sequence in sequences {
            let mut next = end;
            for &(lo, hi) in sequence {
                next = if let Some(&id) = cache.get(&(next, lo, hi)) {
                    id
                } else {
                    let id = self.add(Building::Range(lo, hi, next))?;
                    cache.insert((next, lo, hi), id);
                    id
                };
            }
            self.patch(union, next)?;
        }
        Ok(Fragment { start: union, end })
    }

    fn forward_utf8(&mut self, sequences: &[Vec<(u8, u8)>], end: StateId) -> Result<Fragment, TooBig> {
        let mut compiler = Utf8Compiler {
            target: end,
            stack: vec![Node::default()],
            cache: HashMap::new(),
        };
        for sequence in sequences {
            compiler.add(self, sequence)?;
        }
        compiler.collapse(self, 0)?;
        let root = core::mem::take(&mut compiler.stack[0].transitions);
        let start = compiler.freeze(self, root)?;
        Ok(Fragment { start, end })
    }

    fn repeat(
        &mut self,
        sub: &Hir,
        min: u32,
        max: Option<u32>,
        greedy: bool,
    ) -> Result<Fragment, TooBig> {
        match (min, max) {
            (0, None) => {
                if sub.min_len() > 0 {
                    let union = self.union(greedy)?;
                    let inner = self.compile(sub)?;
                    self.patch(union, inner.start)?;
                    self.patch(inner.end, union)?;
                    return Ok(Fragment {
                        start: union,
                        end: union,
                    });
                }
                let inner = self.compile(sub)?;
                let plus = self.union(greedy)?;
                self.patch(inner.end, plus)?;
                self.patch(plus, inner.start)?;
                let question = self.union(greedy)?;
                let end = self.empty()?;
                self.patch(question, inner.start)?;
                self.patch(question, end)?;
                self.patch(plus, end)?;
                Ok(Fragment { start: question, end })
            }
            (_, None) => {
                let prefix = self.copies(sub, min - 1)?;
                let inner = self.compile(sub)?;
                let union = self.union(greedy)?;
                self.patch(inner.end, union)?;
                self.patch(union, inner.start)?;
                let start = match prefix {
                    Some(prefix) => {
                        self.patch(prefix.end, inner.start)?;
                        prefix.start
                    }
                    None => inner.start,
                };
                Ok(Fragment { start, end: union })
            }
            (_, Some(max)) => {
                let prefix = self.copies(sub, min)?;
                let end = self.empty()?;
                let (start, mut previous) = if let Some(prefix) = prefix {
                    (prefix.start, prefix.end)
                } else {
                    let id = self.empty()?;
                    (id, id)
                };
                for _ in min..max {
                    let union = self.union(greedy)?;
                    self.patch(previous, union)?;
                    let inner = self.compile(sub)?;
                    self.patch(union, inner.start)?;
                    self.patch(union, end)?;
                    previous = inner.end;
                }
                self.patch(previous, end)?;
                Ok(Fragment { start, end })
            }
        }
    }

    fn copies(&mut self, sub: &Hir, count: u32) -> Result<Option<Fragment>, TooBig> {
        let mut result: Option<Fragment> = None;
        for _ in 0..count {
            let fragment = self.compile(sub)?;
            result = Some(match result {
                None => fragment,
                Some(previous) => {
                    self.patch(previous.end, fragment.start)?;
                    Fragment {
                        start: previous.start,
                        end: fragment.end,
                    }
                }
            });
        }
        Ok(result)
    }
}

#[derive(Default)]
struct Node {
    transitions: Vec<(u8, u8, StateId)>,
    last: Option<(u8, u8)>,
}

struct Utf8Compiler {
    target: StateId,
    stack: Vec<Node>,
    cache: HashMap<Vec<(u8, u8, StateId)>, StateId>,
}

impl Utf8Compiler {
    fn add(&mut self, builder: &mut Builder, sequence: &[(u8, u8)]) -> Result<(), TooBig> {
        let mut prefix = 0;
        while prefix + 1 < sequence.len()
            && prefix < self.stack.len()
            && self.stack[prefix].last == Some(sequence[prefix])
        {
            prefix += 1;
        }
        self.collapse(builder, prefix)?;
        self.stack[prefix].last = Some(sequence[prefix]);
        for &range in &sequence[prefix + 1..] {
            self.stack.push(Node {
                transitions: Vec::new(),
                last: Some(range),
            });
        }
        Ok(())
    }

    fn collapse(&mut self, builder: &mut Builder, depth: usize) -> Result<(), TooBig> {
        let mut child = self.target;
        while self.stack.len() > depth + 1 {
            let Some(mut node) = self.stack.pop() else {
                break;
            };
            if let Some((lo, hi)) = node.last.take() {
                node.transitions.push((lo, hi, child));
            }
            child = self.freeze(builder, node.transitions)?;
        }
        if let Some((lo, hi)) = self.stack[depth].last.take() {
            self.stack[depth].transitions.push((lo, hi, child));
        }
        Ok(())
    }

    fn freeze(
        &mut self,
        builder: &mut Builder,
        transitions: Vec<(u8, u8, StateId)>,
    ) -> Result<StateId, TooBig> {
        if let Some(&id) = self.cache.get(&transitions) {
            return Ok(id);
        }
        let id = builder.add(Building::Sparse(transitions.clone()))?;
        self.cache.insert(transitions, id);
        Ok(id)
    }
}

pub struct Options {
    pub reverse: bool,
    pub captures: bool,
    pub size_limit: usize,
    pub groups: usize,
}

pub fn build(hir: &Hir, options: &Options) -> Result<Nfa, TooBig> {
    let mut builder = Builder {
        states: Vec::new(),
        memory: 0,
        limit: options.size_limit,
        reverse: options.reverse,
        captures: options.captures,
    };
    let body = if options.captures && !options.reverse {
        Hir::Capture {
            index: 0,
            sub: Box::new(hir.clone()),
        }
    } else {
        hir.clone()
    };
    let fragment = builder.compile(&body)?;
    let matched = builder.add(Building::Match)?;
    builder.patch(fragment.end, matched)?;
    let start_anchored = fragment.start;
    let start_unanchored = if options.reverse {
        start_anchored
    } else {
        let union = builder.union(false)?;
        let any = builder.add(Building::Range(0, 255, union))?;
        builder.patch(union, any)?;
        builder.patch(union, start_anchored)?;
        union
    };
    let looks = hir.looks();
    let looks = if options.reverse {
        Look::ALL
            .iter()
            .filter(|look| looks & look.bit() != 0)
            .fold(0, |bits, look| bits | look.reversed().bit())
    } else {
        looks
    };
    let slots = if options.captures && !options.reverse {
        options.groups * 2
    } else {
        0
    };
    Ok(finish(builder, start_anchored, start_unanchored, looks, slots))
}

fn resolve(states: &[Building], mut id: StateId) -> StateId {
    let mut steps = 0;
    loop {
        match &states[id as usize] {
            Building::Empty(next) if *next != UNPATCHED => {
                id = *next;
                steps += 1;
                if steps > states.len() {
                    return id;
                }
            }
            Building::Union(alternatives) | Building::UnionReverse(alternatives)
                if alternatives.len() == 1 =>
            {
                id = alternatives[0];
                steps += 1;
                if steps > states.len() {
                    return id;
                }
            }
            _ => return id,
        }
    }
}

#[allow(clippy::too_many_lines)]
fn finish(
    builder: Builder,
    start_anchored: StateId,
    start_unanchored: StateId,
    looks: u32,
    slots: usize,
) -> Nfa {
    let states = builder.states;
    let start_anchored = resolve(&states, start_anchored);
    let start_unanchored = resolve(&states, start_unanchored);
    let mut remap: Vec<StateId> = vec![UNPATCHED; states.len()];
    let mut order: Vec<StateId> = Vec::new();
    let mut stack = vec![start_unanchored, start_anchored];
    while let Some(id) = stack.pop() {
        if remap[id as usize] != UNPATCHED {
            continue;
        }
        remap[id as usize] = order.len() as StateId;
        order.push(id);
        let mut visit = |next: StateId| {
            if next != UNPATCHED {
                stack.push(resolve(&states, next));
            }
        };
        match &states[id as usize] {
            Building::Range(_, _, next) | Building::Look(_, next) | Building::Capture(_, next) => {
                visit(*next);
            }
            Building::Sparse(ranges) => {
                for &(_, _, next) in ranges {
                    visit(next);
                }
            }
            Building::Union(alternatives) | Building::UnionReverse(alternatives) => {
                for &next in alternatives {
                    visit(next);
                }
            }
            Building::Empty(_) | Building::Fail | Building::Match => {}
        }
    }
    let target = |next: StateId| -> StateId {
        if next == UNPATCHED {
            UNPATCHED
        } else {
            remap[resolve(&states, next) as usize]
        }
    };
    let mut out = Vec::with_capacity(order.len());
    for &id in &order {
        let state = match &states[id as usize] {
            Building::Range(lo, hi, next) => State::Range {
                lo: *lo,
                hi: *hi,
                next: target(*next),
            },
            Building::Sparse(ranges) => {
                if let [(lo, hi, next)] = ranges.as_slice() {
                    State::Range {
                        lo: *lo,
                        hi: *hi,
                        next: target(*next),
                    }
                } else {
                    State::Sparse(
                        ranges
                            .iter()
                            .map(|&(lo, hi, next)| (lo, hi, target(next)))
                            .collect(),
                    )
                }
            }
            Building::Look(look, next) => State::Look {
                look: *look,
                next: target(*next),
            },
            Building::Union(alternatives) => {
                if alternatives.is_empty() {
                    State::Fail
                } else {
                    State::Union(alternatives.iter().map(|&next| target(next)).collect())
                }
            }
            Building::UnionReverse(alternatives) => {
                if alternatives.is_empty() {
                    State::Fail
                } else {
                    State::Union(alternatives.iter().rev().map(|&next| target(next)).collect())
                }
            }
            Building::Capture(slot, next) => State::Capture {
                slot: *slot,
                next: target(*next),
            },
            Building::Empty(_) | Building::Fail => State::Fail,
            Building::Match => State::Match,
        };
        out.push(state);
    }
    let classes = byte_classes(&out, looks);
    Nfa {
        start_anchored: remap[start_anchored as usize],
        start_unanchored: remap[start_unanchored as usize],
        states: out,
        looks,
        slots,
        classes,
    }
}

fn byte_classes(states: &[State], looks: u32) -> ByteClasses {
    let mut boundary = [false; 257];
    let mut mark = |lo: u8, hi: u8| {
        boundary[usize::from(lo)] = true;
        boundary[usize::from(hi) + 1] = true;
    };
    for state in states {
        match state {
            State::Range { lo, hi, .. } => mark(*lo, *hi),
            State::Sparse(ranges) => {
                for &(lo, hi, _) in &**ranges {
                    mark(lo, hi);
                }
            }
            _ => {}
        }
    }
    if looks & (Look::StartLine.bit() | Look::EndLine.bit()) != 0 {
        mark(b'\n', b'\n');
    }
    if looks & WORD_LOOKS != 0 {
        mark(b'0', b'9');
        mark(b'A', b'Z');
        mark(b'_', b'_');
        mark(b'a', b'z');
        mark(0x80, 0xFF);
    }
    let mut map = [0u8; 256];
    let mut class = 0usize;
    for byte in 0..256usize {
        if byte > 0 && boundary[byte] {
            class += 1;
        }
        map[byte] = class as u8;
    }
    ByteClasses {
        map,
        count: class + 1,
    }
}
