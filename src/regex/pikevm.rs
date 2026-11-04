use super::hir::Look;
use super::nfa::{Nfa, State, StateId};
use super::unicode;

pub const NONE: usize = usize::MAX;

#[derive(Clone, Debug)]
pub struct SparseSet {
    dense: Vec<StateId>,
    sparse: Vec<u32>,
    len: usize,
}

impl SparseSet {
    pub fn new(capacity: usize) -> Self {
        Self {
            dense: vec![0; capacity],
            sparse: vec![0; capacity],
            len: 0,
        }
    }

    #[inline]
    pub fn contains(&self, id: StateId) -> bool {
        let index = self.sparse[id as usize] as usize;
        index < self.len && self.dense[index] == id
    }

    #[inline]
    pub fn insert(&mut self, id: StateId) -> bool {
        if self.contains(id) {
            return false;
        }
        self.dense[self.len] = id;
        self.sparse[id as usize] = u32::try_from(self.len).unwrap_or(u32::MAX);
        self.len += 1;
        true
    }

    #[inline]
    pub fn clear(&mut self) {
        self.len = 0;
    }

    #[inline]
    pub const fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    pub fn get(&self, index: usize) -> StateId {
        self.dense[index]
    }

    pub fn as_slice(&self) -> &[StateId] {
        &self.dense[..self.len]
    }
}

#[derive(Clone, Debug)]
struct Threads {
    set: SparseSet,
    slots: Vec<usize>,
    stride: usize,
}

impl Threads {
    fn new(states: usize, stride: usize) -> Self {
        Self {
            set: SparseSet::new(states),
            slots: vec![NONE; states * stride],
            stride,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Frame {
    Explore(StateId),
    Restore(usize, usize),
}

#[derive(Clone, Debug)]
pub struct PikeCache {
    current: Threads,
    next: Threads,
    stack: Vec<Frame>,
    scratch: Vec<usize>,
}

impl PikeCache {
    pub fn new(nfa: &Nfa) -> Self {
        let stride = nfa.slots.max(2);
        Self {
            current: Threads::new(nfa.states.len(), stride),
            next: Threads::new(nfa.states.len(), stride),
            stack: Vec::new(),
            scratch: vec![NONE; stride],
        }
    }
}

#[inline]
pub const fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

const fn utf8_length(byte: u8) -> Option<usize> {
    match byte {
        0x00..=0x7F => Some(1),
        0xC0..=0xDF => Some(2),
        0xE0..=0xEF => Some(3),
        0xF0..=0xF7 => Some(4),
        _ => None,
    }
}

fn decode(bytes: &[u8]) -> Option<char> {
    let first = *bytes.first()?;
    let length = utf8_length(first)?;
    if length > bytes.len() {
        return None;
    }
    core::str::from_utf8(&bytes[..length])
        .ok()
        .and_then(|text| text.chars().next())
}

fn decode_last(bytes: &[u8]) -> Option<char> {
    if bytes.is_empty() {
        return None;
    }
    let mut start = bytes.len() - 1;
    let limit = bytes.len().saturating_sub(4);
    while start > limit && bytes[start] & 0xC0 == 0x80 {
        start -= 1;
    }
    decode(&bytes[start..])
}

pub fn word_after(haystack: &[u8], at: usize) -> bool {
    decode(&haystack[at..]).is_some_and(unicode::is_word_char)
}

pub fn word_before(haystack: &[u8], at: usize) -> bool {
    decode_last(&haystack[..at]).is_some_and(unicode::is_word_char)
}

fn strict_before(haystack: &[u8], at: usize) -> Option<bool> {
    if at == 0 {
        return Some(false);
    }
    decode_last(&haystack[..at]).map(unicode::is_word_char)
}

fn strict_after(haystack: &[u8], at: usize) -> Option<bool> {
    if at == haystack.len() {
        return Some(false);
    }
    decode(&haystack[at..]).map(unicode::is_word_char)
}

pub fn look_matches(look: Look, haystack: &[u8], at: usize) -> bool {
    match look {
        Look::Start => at == 0,
        Look::End => at == haystack.len(),
        Look::StartLine => at == 0 || haystack[at - 1] == b'\n',
        Look::EndLine => at == haystack.len() || haystack[at] == b'\n',
        Look::WordUnicodeNegate => match (strict_before(haystack, at), strict_after(haystack, at)) {
            (Some(before), Some(after)) => before == after,
            _ => false,
        },
        Look::WordStartHalfUnicode => strict_before(haystack, at) == Some(false),
        Look::WordEndHalfUnicode => strict_after(haystack, at) == Some(false),
        _ if look.is_unicode_word() => {
            look.holds(word_before(haystack, at), word_after(haystack, at))
        }
        _ => look.holds(
            at > 0 && is_word_byte(haystack[at - 1]),
            at < haystack.len() && is_word_byte(haystack[at]),
        ),
    }
}

#[inline]
fn byte_matches(state: &State, byte: u8) -> Option<StateId> {
    match state {
        State::Range { lo, hi, next } => (*lo <= byte && byte <= *hi).then_some(*next),
        State::Sparse(ranges) => {
            for &(lo, hi, next) in &**ranges {
                if byte < lo {
                    return None;
                }
                if byte <= hi {
                    return Some(next);
                }
            }
            None
        }
        _ => None,
    }
}

fn closure(
    nfa: &Nfa,
    threads: &mut Threads,
    stack: &mut Vec<Frame>,
    slots: &mut [usize],
    haystack: &[u8],
    at: usize,
    start: StateId,
) {
    stack.push(Frame::Explore(start));
    while let Some(frame) = stack.pop() {
        let mut id = match frame {
            Frame::Restore(slot, value) => {
                slots[slot] = value;
                continue;
            }
            Frame::Explore(id) => id,
        };
        loop {
            if !threads.set.insert(id) {
                break;
            }
            match &nfa.states[id as usize] {
                State::Range { .. } | State::Sparse(_) | State::Match => {
                    let offset = id as usize * threads.stride;
                    threads.slots[offset..offset + slots.len()].copy_from_slice(slots);
                    break;
                }
                State::Fail => break,
                State::Look { look, next } => {
                    if !look_matches(*look, haystack, at) {
                        break;
                    }
                    id = *next;
                }
                State::Union(alternatives) => {
                    for &alternative in alternatives[1..].iter().rev() {
                        stack.push(Frame::Explore(alternative));
                    }
                    id = alternatives[0];
                }
                State::Capture { slot, next } => {
                    let slot = *slot as usize;
                    if slot < slots.len() {
                        stack.push(Frame::Restore(slot, slots[slot]));
                        slots[slot] = at;
                    }
                    id = *next;
                }
            }
        }
    }
}

pub struct Search<'a> {
    pub haystack: &'a [u8],
    pub start: usize,
    pub end: usize,
    pub anchored: bool,
    pub earliest: bool,
}

pub fn search(
    nfa: &Nfa,
    cache: &mut PikeCache,
    search: &Search<'_>,
    skip: super::dfa::Skip<'_>,
    out: &mut [usize],
) -> bool {
    let PikeCache {
        current,
        next,
        stack,
        scratch,
    } = cache;
    let haystack = search.haystack;
    let width = out.len().min(scratch.len());
    current.set.clear();
    next.set.clear();
    let mut matched = false;
    let mut at = search.start;
    loop {
        if current.set.is_empty() {
            if matched || (search.anchored && at > search.start) {
                break;
            }
            if !search.anchored {
                if let Some(skip) = skip {
                    match skip(haystack, at, search.end) {
                        Some(candidate) => at = candidate,
                        None => break,
                    }
                }
            }
        }
        if !matched && (!search.anchored || at == search.start) {
            scratch[..width].fill(NONE);
            closure(
                nfa,
                current,
                stack,
                &mut scratch[..width],
                haystack,
                at,
                nfa.start_anchored,
            );
        }
        for index in 0..current.set.len() {
            let id = current.set.get(index);
            let state = &nfa.states[id as usize];
            if matches!(state, State::Match) {
                let offset = id as usize * current.stride;
                out[..width].copy_from_slice(&current.slots[offset..offset + width]);
                matched = true;
                if search.earliest {
                    return true;
                }
                break;
            }
            if at >= search.end {
                continue;
            }
            if let Some(target) = byte_matches(state, haystack[at]) {
                let offset = id as usize * current.stride;
                scratch[..width].copy_from_slice(&current.slots[offset..offset + width]);
                closure(
                    nfa,
                    next,
                    stack,
                    &mut scratch[..width],
                    haystack,
                    at + 1,
                    target,
                );
            }
        }
        if at >= search.end {
            break;
        }
        at += 1;
        core::mem::swap(current, next);
        next.set.clear();
    }
    matched
}
