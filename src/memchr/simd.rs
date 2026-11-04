#![allow(clippy::inline_always)]

use super::vector::{MoveMask, Vector};

pub trait Needles<V: Vector>: Copy {
    unsafe fn matches(self, chunk: V) -> V;
    fn matches_byte(self, byte: u8) -> bool;
}

#[derive(Clone, Copy)]
pub struct One<V> {
    vector: V,
    byte: u8,
}

#[derive(Clone, Copy)]
pub struct Two<V> {
    vectors: [V; 2],
    bytes: [u8; 2],
}

#[derive(Clone, Copy)]
pub struct Three<V> {
    vectors: [V; 3],
    bytes: [u8; 3],
}

impl<V: Vector> One<V> {
    #[inline(always)]
    pub unsafe fn new(byte: u8) -> Self {
        Self {
            vector: V::splat(byte),
            byte,
        }
    }
}

impl<V: Vector> Two<V> {
    #[inline(always)]
    pub unsafe fn new(first: u8, second: u8) -> Self {
        Self {
            vectors: [V::splat(first), V::splat(second)],
            bytes: [first, second],
        }
    }
}

impl<V: Vector> Three<V> {
    #[inline(always)]
    pub unsafe fn new(first: u8, second: u8, third: u8) -> Self {
        Self {
            vectors: [V::splat(first), V::splat(second), V::splat(third)],
            bytes: [first, second, third],
        }
    }
}

impl<V: Vector> Needles<V> for One<V> {
    #[inline(always)]
    unsafe fn matches(self, chunk: V) -> V {
        chunk.cmpeq(self.vector)
    }

    #[inline(always)]
    fn matches_byte(self, byte: u8) -> bool {
        byte == self.byte
    }
}

impl<V: Vector> Needles<V> for Two<V> {
    #[inline(always)]
    unsafe fn matches(self, chunk: V) -> V {
        chunk.cmpeq(self.vectors[0]).or(chunk.cmpeq(self.vectors[1]))
    }

    #[inline(always)]
    fn matches_byte(self, byte: u8) -> bool {
        byte == self.bytes[0] || byte == self.bytes[1]
    }
}

impl<V: Vector> Needles<V> for Three<V> {
    #[inline(always)]
    unsafe fn matches(self, chunk: V) -> V {
        chunk
            .cmpeq(self.vectors[0])
            .or(chunk.cmpeq(self.vectors[1]))
            .or(chunk.cmpeq(self.vectors[2]))
    }

    #[inline(always)]
    fn matches_byte(self, byte: u8) -> bool {
        byte == self.bytes[0] || byte == self.bytes[1] || byte == self.bytes[2]
    }
}

#[inline(always)]
pub unsafe fn find<V: Vector, N: Needles<V>>(haystack: &[u8], needles: N) -> Option<usize> {
    let len = haystack.len();
    let ptr = haystack.as_ptr();
    if len < V::BYTES {
        return haystack.iter().position(|&byte| needles.matches_byte(byte));
    }
    let mask = needles.matches(V::load_unaligned(ptr)).movemask();
    if mask.has_non_zero() {
        return Some(mask.first_offset());
    }
    let mut at = V::BYTES - (ptr as usize & V::ALIGN);
    if len >= 4 * V::BYTES {
        while at <= len - 4 * V::BYTES {
            let a = needles.matches(V::load_aligned(ptr.add(at)));
            let b = needles.matches(V::load_aligned(ptr.add(at + V::BYTES)));
            let c = needles.matches(V::load_aligned(ptr.add(at + 2 * V::BYTES)));
            let d = needles.matches(V::load_aligned(ptr.add(at + 3 * V::BYTES)));
            if a.or(b).or(c.or(d)).any() {
                let mask = a.movemask();
                if mask.has_non_zero() {
                    return Some(at + mask.first_offset());
                }
                let mask = b.movemask();
                if mask.has_non_zero() {
                    return Some(at + V::BYTES + mask.first_offset());
                }
                let mask = c.movemask();
                if mask.has_non_zero() {
                    return Some(at + 2 * V::BYTES + mask.first_offset());
                }
                return Some(at + 3 * V::BYTES + d.movemask().first_offset());
            }
            at += 4 * V::BYTES;
        }
    }
    while at + V::BYTES <= len {
        let mask = needles.matches(V::load_aligned(ptr.add(at))).movemask();
        if mask.has_non_zero() {
            return Some(at + mask.first_offset());
        }
        at += V::BYTES;
    }
    if at < len {
        let last = len - V::BYTES;
        let mask = needles.matches(V::load_unaligned(ptr.add(last))).movemask();
        if mask.has_non_zero() {
            return Some(last + mask.first_offset());
        }
    }
    None
}

#[inline(always)]
pub unsafe fn rfind<V: Vector, N: Needles<V>>(haystack: &[u8], needles: N) -> Option<usize> {
    let len = haystack.len();
    let ptr = haystack.as_ptr();
    if len < V::BYTES {
        return haystack.iter().rposition(|&byte| needles.matches_byte(byte));
    }
    let mask = needles
        .matches(V::load_unaligned(ptr.add(len - V::BYTES)))
        .movemask();
    if mask.has_non_zero() {
        return Some(len - V::BYTES + mask.last_offset());
    }
    let mut at = len - ((ptr as usize + len) & V::ALIGN);
    while at >= 4 * V::BYTES {
        at -= 4 * V::BYTES;
        let a = needles.matches(V::load_aligned(ptr.add(at)));
        let b = needles.matches(V::load_aligned(ptr.add(at + V::BYTES)));
        let c = needles.matches(V::load_aligned(ptr.add(at + 2 * V::BYTES)));
        let d = needles.matches(V::load_aligned(ptr.add(at + 3 * V::BYTES)));
        if a.or(b).or(c.or(d)).any() {
            let mask = d.movemask();
            if mask.has_non_zero() {
                return Some(at + 3 * V::BYTES + mask.last_offset());
            }
            let mask = c.movemask();
            if mask.has_non_zero() {
                return Some(at + 2 * V::BYTES + mask.last_offset());
            }
            let mask = b.movemask();
            if mask.has_non_zero() {
                return Some(at + V::BYTES + mask.last_offset());
            }
            return Some(at + a.movemask().last_offset());
        }
    }
    while at >= V::BYTES {
        at -= V::BYTES;
        let mask = needles.matches(V::load_aligned(ptr.add(at))).movemask();
        if mask.has_non_zero() {
            return Some(at + mask.last_offset());
        }
    }
    if at > 0 {
        let mask = needles.matches(V::load_unaligned(ptr)).movemask();
        if mask.has_non_zero() {
            return Some(mask.last_offset());
        }
    }
    None
}

#[derive(Clone, Copy, Debug)]
pub struct Pair {
    pub index1: usize,
    pub index2: usize,
    pub byte1: u8,
    pub byte2: u8,
    pub fold1: u8,
    pub fold2: u8,
}

impl Pair {
    #[inline(always)]
    pub const fn reach(&self) -> usize {
        if self.index1 > self.index2 {
            self.index1
        } else {
            self.index2
        }
    }
}

#[inline(always)]
unsafe fn pair_mask<V: Vector>(ptr: *const u8, at: usize, pair: &Pair, vectors: [V; 4]) -> V {
    let first = V::load_unaligned(ptr.add(at + pair.index1)).or(vectors[2]);
    let second = V::load_unaligned(ptr.add(at + pair.index2)).or(vectors[3]);
    first.cmpeq(vectors[0]).and(second.cmpeq(vectors[1]))
}

#[inline(always)]
unsafe fn pair_vectors<V: Vector>(pair: &Pair) -> [V; 4] {
    [
        V::splat(pair.byte1),
        V::splat(pair.byte2),
        V::splat(pair.fold1),
        V::splat(pair.fold2),
    ]
}

#[inline(always)]
pub unsafe fn find_pair<V: Vector, F: FnMut(usize) -> Option<bool>>(
    haystack: &[u8],
    first: usize,
    last: usize,
    pair: &Pair,
    mut confirm: F,
) -> Result<Option<usize>, usize> {
    let ptr = haystack.as_ptr();
    let vectors = pair_vectors::<V>(pair);
    let limit = haystack.len() - pair.reach() - V::BYTES;
    let mut at = first;
    while at + V::BYTES <= limit {
        let low = pair_mask(ptr, at, pair, vectors);
        let high = pair_mask(ptr, at + V::BYTES, pair, vectors);
        if low.or(high).any() {
            for (base, chunk) in [(at, low), (at + V::BYTES, high)] {
                let mut mask = chunk.movemask();
                while mask.has_non_zero() {
                    let candidate = base + mask.first_offset();
                    if candidate > last {
                        return Ok(None);
                    }
                    match confirm(candidate) {
                        Some(true) => return Ok(Some(candidate)),
                        Some(false) => {}
                        None => return Err(candidate),
                    }
                    mask = mask.clear_least_significant_bit();
                }
            }
        }
        at += 2 * V::BYTES;
    }
    while at <= last {
        let chunk = at.min(limit);
        let mut mask = pair_mask(ptr, chunk, pair, vectors).movemask();
        while mask.has_non_zero() {
            let candidate = chunk + mask.first_offset();
            mask = mask.clear_least_significant_bit();
            if candidate < at {
                continue;
            }
            if candidate > last {
                return Ok(None);
            }
            match confirm(candidate) {
                Some(true) => return Ok(Some(candidate)),
                Some(false) => {}
                None => return Err(candidate),
            }
        }
        at = chunk + V::BYTES;
    }
    Ok(None)
}

#[inline(always)]
pub unsafe fn rfind_pair<V: Vector, F: FnMut(usize) -> Option<bool>>(
    haystack: &[u8],
    first: usize,
    last: usize,
    pair: &Pair,
    mut confirm: F,
) -> Result<Option<usize>, usize> {
    let ptr = haystack.as_ptr();
    let vectors = pair_vectors::<V>(pair);
    let mut end = last + 1;
    while end >= first + 2 * V::BYTES {
        let low = end - 2 * V::BYTES;
        let lower = pair_mask(ptr, low, pair, vectors);
        let upper = pair_mask(ptr, low + V::BYTES, pair, vectors);
        if lower.or(upper).any() {
            for (base, chunk) in [(low + V::BYTES, upper), (low, lower)] {
                let mut mask = chunk.movemask();
                while mask.has_non_zero() {
                    let candidate = base + mask.last_offset();
                    match confirm(candidate) {
                        Some(true) => return Ok(Some(candidate)),
                        Some(false) => {}
                        None => return Err(candidate),
                    }
                    mask = mask.clear_most_significant_bit();
                }
            }
        }
        end = low;
    }
    while end > first {
        let chunk = end.saturating_sub(V::BYTES);
        let mut mask = pair_mask(ptr, chunk, pair, vectors).movemask();
        while mask.has_non_zero() {
            let candidate = chunk + mask.last_offset();
            mask = mask.clear_most_significant_bit();
            if candidate >= end {
                continue;
            }
            if candidate < first {
                return Ok(None);
            }
            match confirm(candidate) {
                Some(true) => return Ok(Some(candidate)),
                Some(false) => {}
                None => return Err(candidate),
            }
        }
        end = chunk;
    }
    Ok(None)
}
