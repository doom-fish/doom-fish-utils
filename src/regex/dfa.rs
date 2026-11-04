#![allow(clippy::cast_possible_truncation)]

use super::hir::{Look, UNICODE_WORD_LOOKS, WORD_LOOKS};
use super::nfa::{Nfa, State, StateId};
use super::pikevm::{is_word_byte, SparseSet};

const MASK: u32 = (1 << 28) - 1;
const TAG_MATCH: u32 = 1 << 28;
const TAG_START: u32 = 1 << 29;
const DEAD: u32 = 1 << 30;
const QUIT: u32 = (1 << 30) | 1;
const UNKNOWN: u32 = 1 << 31;

const FROM_NON_WORD: u32 = 0;
const FROM_WORD: u32 = 1;
const FROM_NON_ASCII: u32 = 2;

#[derive(Debug)]
pub struct GaveUp;

pub type Skip<'a> = Option<&'a dyn Fn(&[u8], usize, usize) -> Option<usize>>;

#[derive(Clone, Debug)]
#[allow(clippy::struct_excessive_bools)]
pub struct Dfa {
    classes: [u8; 256],
    stride2: u32,
    eoi: usize,
    leftmost_first: bool,
    tag_starts: bool,
    looks: u32,
    unicode_word: bool,
    keep_unions: bool,
    capacity: usize,
}

#[derive(Clone, Debug)]
pub struct DfaCache {
    trans: Vec<u32>,
    states: Vec<Box<[u32]>>,
    ids: Vec<u32>,
    table: Vec<u32>,
    starts: [u32; 10],
    memory: usize,
    clears: usize,
    created: usize,
    set: SparseSet,
    next: SparseSet,
    stack: Vec<StateId>,
    key: Vec<u32>,
    current: Vec<u32>,
    pub stopped: usize,
}

fn hash(key: &[u32]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &word in key {
        hash = (hash ^ u64::from(word)).wrapping_mul(0x0000_0100_0000_01B3);
        hash ^= hash >> 29;
    }
    hash
}

#[derive(Clone, Copy)]
enum Unit {
    Byte(u8),
    Eoi,
}

impl Dfa {
    pub const fn new(nfa: &Nfa, leftmost_first: bool, tag_starts: bool, capacity: usize) -> Self {
        let count = nfa.classes.count;
        let stride2 = (count + 1).next_power_of_two().trailing_zeros();
        Self {
            classes: nfa.classes.map,
            stride2,
            eoi: count,
            leftmost_first,
            tag_starts,
            looks: nfa.looks,
            unicode_word: nfa.looks & UNICODE_WORD_LOOKS != 0,
            keep_unions: nfa.looks & (WORD_LOOKS | Look::End.bit() | Look::EndLine.bit()) != 0,
            capacity,
        }
    }

    const fn stride(&self) -> usize {
        1 << self.stride2
    }

    pub fn cache(nfa: &Nfa) -> DfaCache {
        DfaCache {
            trans: Vec::new(),
            states: Vec::new(),
            ids: Vec::new(),
            table: Vec::new(),
            starts: [UNKNOWN; 10],
            memory: 0,
            clears: 0,
            created: 0,
            set: SparseSet::new(nfa.states.len()),
            next: SparseSet::new(nfa.states.len()),
            stack: Vec::new(),
            key: Vec::new(),
            current: Vec::new(),
            stopped: 0,
        }
    }

    const fn classify(&self, byte: u8) -> u32 {
        if byte >= 0x80 {
            if self.unicode_word {
                FROM_NON_ASCII
            } else {
                FROM_NON_WORD
            }
        } else if is_word_byte(byte) {
            FROM_WORD
        } else {
            FROM_NON_WORD
        }
    }

    fn closure(nfa: &Nfa, set: &mut SparseSet, stack: &mut Vec<StateId>, start: StateId, have: u32) {
        stack.push(start);
        while let Some(mut id) = stack.pop() {
            loop {
                if !set.insert(id) {
                    break;
                }
                match &nfa.states[id as usize] {
                    State::Union(alternatives) => {
                        for &alternative in alternatives[1..].iter().rev() {
                            stack.push(alternative);
                        }
                        id = alternatives[0];
                    }
                    State::Capture { next, .. } => id = *next,
                    State::Look { look, next } if have & look.bit() != 0 => id = *next,
                    _ => break,
                }
            }
        }
    }

    fn build_key(&self, nfa: &Nfa, cache: &mut DfaCache, is_match: bool, from: u32, have: u32) {
        let DfaCache { key, next, .. } = cache;
        key.clear();
        key.push(0);
        key.push(0);
        let mut need = 0u32;
        for &id in next.as_slice() {
            match &nfa.states[id as usize] {
                State::Range { .. } | State::Sparse(_) => key.push(id),
                State::Look { look, .. } => {
                    if have & look.bit() != 0 {
                        continue;
                    }
                    if matches!(look, Look::Start | Look::StartLine) {
                        continue;
                    }
                    need |= look.bit();
                    key.push(id);
                }
                State::Match => {
                    key.push(id);
                    if self.leftmost_first {
                        break;
                    }
                }
                State::Union(_) => {
                    if self.keep_unions {
                        key.push(id);
                    }
                }
                State::Capture { .. } | State::Fail => {}
            }
        }
        let from = if need == 0 || self.looks & WORD_LOOKS == 0 { 0 } else { from };
        let have = if need == 0 { 0 } else { have & self.looks };
        key[0] = u32::from(is_match) | (from << 1);
        key[1] = have | (need << 16);
    }

    fn start(
        &self,
        nfa: &Nfa,
        cache: &mut DfaCache,
        haystack: &[u8],
        at: usize,
        anchored: bool,
        reverse: bool,
    ) -> Result<u32, GaveUp> {
        let previous = if reverse {
            haystack.get(at).copied()
        } else if at == 0 {
            None
        } else {
            Some(haystack[at - 1])
        };
        let (context, have, from) = match previous {
            None => (0, Look::Start.bit() | Look::StartLine.bit(), FROM_NON_WORD),
            Some(b'\n') => (1, Look::StartLine.bit(), FROM_NON_WORD),
            Some(byte) => {
                let from = self.classify(byte);
                (2 + from as usize, 0, from)
            }
        };
        let slot = context * 2 + usize::from(anchored);
        let cached = cache.starts[slot];
        if cached != UNKNOWN {
            return Ok(cached);
        }
        let start = if anchored {
            nfa.start_anchored
        } else {
            nfa.start_unanchored
        };
        let have = have & self.looks;
        cache.next.clear();
        Self::closure(nfa, &mut cache.next, &mut cache.stack, start, have);
        self.build_key(nfa, cache, false, from, have);
        let tag = if self.tag_starts && !anchored {
            TAG_START
        } else {
            0
        };
        let id = self.intern(cache, tag)?;
        cache.starts[slot] = id;
        Ok(id)
    }

    fn intern(&self, cache: &mut DfaCache, tag: u32) -> Result<u32, GaveUp> {
        let key_hash = hash(&cache.key);
        if !cache.table.is_empty() {
            let mask = cache.table.len() - 1;
            let mut slot = key_hash as usize & mask;
            loop {
                let entry = cache.table[slot];
                if entry == 0 {
                    break;
                }
                let index = entry as usize - 1;
                if *cache.states[index] == *cache.key {
                    let id = cache.ids[index] | tag;
                    cache.ids[index] = id;
                    return Ok(id);
                }
                slot = (slot + 1) & mask;
            }
        }
        let stride = self.stride();
        let cost = stride * 4 + cache.key.len() * 4 + 48;
        if cache.memory + cost > self.capacity {
            if cache.states.is_empty() || cost * 4 > self.capacity {
                return Err(GaveUp);
            }
            Self::clear(cache);
        }
        let index = cache.states.len();
        let offset = (index as u32) << self.stride2;
        if offset > MASK - stride as u32 {
            return Err(GaveUp);
        }
        let is_match = cache.key[0] & 1 != 0;
        let id = offset | tag | if is_match { TAG_MATCH } else { 0 };
        cache.states.push(cache.key.clone().into_boxed_slice());
        cache.ids.push(id);
        cache.trans.resize(cache.trans.len() + stride, UNKNOWN);
        cache.memory += cost;
        cache.created += 1;
        if cache.states.len() * 2 > cache.table.len() {
            let capacity = (cache.states.len() * 4).next_power_of_two().max(64);
            cache.table = vec![0; capacity];
            cache.memory += capacity * 4;
            let mask = capacity - 1;
            for (position, state) in cache.states.iter().enumerate() {
                let mut slot = hash(state) as usize & mask;
                while cache.table[slot] != 0 {
                    slot = (slot + 1) & mask;
                }
                cache.table[slot] = position as u32 + 1;
            }
        } else {
            let mask = cache.table.len() - 1;
            let mut slot = key_hash as usize & mask;
            while cache.table[slot] != 0 {
                slot = (slot + 1) & mask;
            }
            cache.table[slot] = index as u32 + 1;
        }
        Ok(id)
    }

    fn clear(cache: &mut DfaCache) {
        cache.trans.clear();
        cache.states.clear();
        cache.ids.clear();
        cache.table.clear();
        cache.starts = [UNKNOWN; 10];
        cache.memory = 0;
        cache.clears += 1;
    }

    fn next(&self, nfa: &Nfa, cache: &mut DfaCache, from: u32, unit: Unit) -> Result<u32, GaveUp> {
        let clears = cache.clears;
        let result = self.compute(nfa, cache, from, unit)?;
        if let Unit::Byte(byte) = unit {
            if cache.clears == clears {
                let class = usize::from(self.classes[usize::from(byte)]);
                cache.trans[(from & MASK) as usize + class] = result;
            }
        }
        Ok(result)
    }

    #[allow(clippy::too_many_lines)]
    fn compute(&self, nfa: &Nfa, cache: &mut DfaCache, from: u32, unit: Unit) -> Result<u32, GaveUp> {
        let index = (from & MASK) as usize >> self.stride2;
        cache.current.clear();
        cache.current.extend_from_slice(&cache.states[index]);
        let header = cache.current[0];
        let from_word = header >> 1;
        let mut have = cache.current[1] & 0xFFFF;
        let need = cache.current[1] >> 16;
        let mut unicode_unknown = false;
        if need != 0 {
            let after = match unit {
                Unit::Eoi => {
                    have |= Look::End.bit() | Look::EndLine.bit();
                    Some(false)
                }
                Unit::Byte(byte) => {
                    if byte == b'\n' {
                        have |= Look::EndLine.bit();
                    }
                    (byte < 0x80).then(|| is_word_byte(byte))
                }
            };
            let before = from_word == FROM_WORD;
            unicode_unknown = from_word == FROM_NON_ASCII || after.is_none();
            if unicode_unknown && need & UNICODE_WORD_LOOKS != 0 {
                return Ok(QUIT);
            }
            for look in Look::ALL {
                if look.is_word()
                    && !(unicode_unknown && look.is_unicode_word())
                    && look.holds(before, after.unwrap_or(false))
                {
                    have |= look.bit();
                }
            }
        }
        cache.set.clear();
        let resolved = need & have != 0;
        for position in 2..cache.current.len() {
            let id = cache.current[position];
            if resolved {
                Self::closure(nfa, &mut cache.set, &mut cache.stack, id, have);
            } else {
                cache.set.insert(id);
            }
        }
        if resolved
            && unicode_unknown
            && self.unicode_word
            && cache.set.as_slice().iter().any(|&id| {
                matches!(&nfa.states[id as usize], State::Look { look, .. } if look.is_unicode_word())
            })
        {
            return Ok(QUIT);
        }
        let mut is_match = false;
        cache.next.clear();
        let (next_have, next_from) = match unit {
            Unit::Eoi => (0, FROM_NON_WORD),
            Unit::Byte(byte) => (
                if byte == b'\n' {
                    Look::StartLine.bit()
                } else {
                    0
                },
                self.classify(byte),
            ),
        };
        let next_have = next_have & self.looks;
        for position in 0..cache.set.len() {
            let id = cache.set.get(position);
            match &nfa.states[id as usize] {
                State::Match => {
                    is_match = true;
                    if self.leftmost_first {
                        break;
                    }
                }
                state => {
                    if let Unit::Byte(byte) = unit {
                        let target = match state {
                            State::Range { lo, hi, next } => {
                                (*lo <= byte && byte <= *hi).then_some(*next)
                            }
                            State::Sparse(ranges) => ranges
                                .iter()
                                .find(|&&(lo, hi, _)| lo <= byte && byte <= hi)
                                .map(|&(_, _, next)| next),
                            _ => None,
                        };
                        if let Some(target) = target {
                            Self::closure(nfa, &mut cache.next, &mut cache.stack, target, next_have);
                        }
                    }
                }
            }
        }
        if matches!(unit, Unit::Eoi) {
            return Ok(if is_match { TAG_MATCH } else { DEAD });
        }
        self.build_key(nfa, cache, is_match, next_from, next_have);
        if cache.key.len() == 2 && !is_match {
            return Ok(DEAD);
        }
        self.intern(cache, 0)
    }

    fn eoi(&self, nfa: &Nfa, cache: &mut DfaCache, from: u32) -> Result<bool, GaveUp> {
        let slot = (from & MASK) as usize + self.eoi;
        let cached = cache.trans[slot];
        let result = if cached == UNKNOWN {
            let result = self.next(nfa, cache, from, Unit::Eoi)?;
            cache.trans[slot] = result;
            result
        } else {
            cached
        };
        if result == QUIT {
            return Err(GaveUp);
        }
        Ok(result & TAG_MATCH != 0)
    }

    fn step(&self, nfa: &Nfa, cache: &mut DfaCache, from: u32, byte: u8) -> Result<u32, GaveUp> {
        let class = usize::from(self.classes[usize::from(byte)]);
        let cached = cache.trans[(from & MASK) as usize + class];
        if cached != UNKNOWN {
            return Ok(cached);
        }
        self.next(nfa, cache, from, Unit::Byte(byte))
    }

    fn final_match(
        &self,
        nfa: &Nfa,
        cache: &mut DfaCache,
        from: u32,
        next: Option<u8>,
    ) -> Result<bool, GaveUp> {
        match next {
            None => self.eoi(nfa, cache, from),
            Some(byte) => {
                let id = self.step(nfa, cache, from, byte)?;
                if id == QUIT {
                    return Err(GaveUp);
                }
                Ok(id != DEAD && id & TAG_MATCH != 0)
            }
        }
    }

    fn give_up(cache: &DfaCache, searched: usize, created_before: usize) -> bool {
        cache.clears >= 3 && searched < 10 * (cache.created - created_before).max(1)
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub fn find_fwd(
        &self,
        nfa: &Nfa,
        cache: &mut DfaCache,
        haystack: &[u8],
        start: usize,
        end: usize,
        anchored: bool,
        earliest: bool,
        prefilter: Skip<'_>,
    ) -> Result<Option<usize>, GaveUp> {
        cache.clears = 0;
        let created_before = cache.created;
        let mut prefilter = prefilter;
        let mut calls = 0usize;
        let mut skipped = 0usize;
        let mut sid = self.start(nfa, cache, haystack, start, anchored, false)?;
        let mut last = None;
        let mut at = start;
        if sid & TAG_START != 0 {
            if let Some(prefilter) = prefilter {
                match prefilter(haystack, at, end) {
                    None => return Ok(None),
                    Some(candidate) => {
                        if candidate != at {
                            at = candidate;
                            sid = self.start(nfa, cache, haystack, at, anchored, false)?;
                        }
                    }
                }
            }
        }
        let classes = &self.classes;
        loop {
            if sid <= MASK {
                let trans = cache.trans.as_slice();
                while at + 3 < end {
                    let a = unsafe {
                        *trans.get_unchecked(
                            sid as usize + usize::from(classes[usize::from(*haystack.get_unchecked(at))]),
                        )
                    };
                    if a > MASK {
                        break;
                    }
                    let b = unsafe {
                        *trans.get_unchecked(
                            a as usize + usize::from(classes[usize::from(*haystack.get_unchecked(at + 1))]),
                        )
                    };
                    if b > MASK {
                        sid = a;
                        at += 1;
                        break;
                    }
                    let c = unsafe {
                        *trans.get_unchecked(
                            b as usize + usize::from(classes[usize::from(*haystack.get_unchecked(at + 2))]),
                        )
                    };
                    if c > MASK {
                        sid = b;
                        at += 2;
                        break;
                    }
                    let d = unsafe {
                        *trans.get_unchecked(
                            c as usize + usize::from(classes[usize::from(*haystack.get_unchecked(at + 3))]),
                        )
                    };
                    if d > MASK {
                        sid = c;
                        at += 3;
                        break;
                    }
                    sid = d;
                    at += 4;
                }
                while at < end {
                    let next = unsafe {
                        *trans.get_unchecked(
                            sid as usize + usize::from(classes[usize::from(*haystack.get_unchecked(at))]),
                        )
                    };
                    if next > MASK {
                        break;
                    }
                    sid = next;
                    at += 1;
                }
            }
            if at >= end {
                break;
            }
            let next = self.step(nfa, cache, sid, haystack[at])?;
            if next > MASK {
                if next == DEAD {
                    cache.stopped = at;
                    return Ok(last);
                }
                if next == QUIT {
                    return Err(GaveUp);
                }
                if next & TAG_MATCH != 0 {
                    last = Some(at);
                    if earliest {
                        return Ok(last);
                    }
                }
            }
            sid = next;
            at += 1;
            if sid & TAG_START != 0 && last.is_none() {
                if let Some(find) = prefilter {
                    match find(haystack, at, end) {
                        None => return Ok(None),
                        Some(candidate) => {
                            calls += 1;
                            skipped += candidate - at;
                            if candidate != at {
                                at = candidate;
                                sid = self.start(nfa, cache, haystack, at, anchored, false)?;
                            }
                            if calls >= 16 && skipped < 8 * calls {
                                prefilter = None;
                            }
                        }
                    }
                }
            }
            if cache.clears >= 3 && Self::give_up(cache, at - start, created_before) {
                return Err(GaveUp);
            }
        }
        if self.final_match(nfa, cache, sid, haystack.get(end).copied())? {
            last = Some(end);
        }
        cache.stopped = end;
        Ok(last)
    }

    pub fn find_rev(
        &self,
        nfa: &Nfa,
        cache: &mut DfaCache,
        haystack: &[u8],
        start: usize,
        end: usize,
    ) -> Result<Option<usize>, GaveUp> {
        cache.clears = 0;
        let created_before = cache.created;
        let mut sid = self.start(nfa, cache, haystack, end, true, true)?;
        let mut last = None;
        let mut at = end;
        let classes = &self.classes;
        loop {
            if sid <= MASK {
                let trans = cache.trans.as_slice();
                while at > start {
                    let next = unsafe {
                        *trans.get_unchecked(
                            sid as usize + usize::from(classes[usize::from(*haystack.get_unchecked(at - 1))]),
                        )
                    };
                    if next > MASK {
                        break;
                    }
                    sid = next;
                    at -= 1;
                }
            }
            if at <= start {
                break;
            }
            let next = self.step(nfa, cache, sid, haystack[at - 1])?;
            if next > MASK {
                if next == DEAD {
                    return Ok(last);
                }
                if next == QUIT {
                    return Err(GaveUp);
                }
                if next & TAG_MATCH != 0 {
                    last = Some(at);
                }
            }
            sid = next;
            at -= 1;
            if cache.clears >= 3 && Self::give_up(cache, end - at, created_before) {
                return Err(GaveUp);
            }
        }
        let previous = if start == 0 {
            None
        } else {
            Some(haystack[start - 1])
        };
        if self.final_match(nfa, cache, sid, previous)? {
            last = Some(start);
        }
        Ok(last)
    }
}
