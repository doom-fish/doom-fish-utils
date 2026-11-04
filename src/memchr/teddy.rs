#![allow(clippy::inline_always)]

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
use super::vector::{MoveMask, Vector};

#[derive(Clone, Debug)]
#[cfg_attr(
    not(any(target_arch = "aarch64", target_arch = "x86_64")),
    allow(dead_code)
)]
pub struct Teddy {
    literals: Vec<Vec<u8>>,
    buckets: [Vec<usize>; 8],
    low: [[u8; 16]; 3],
    high: [[u8; 16]; 3],
    fingerprint: usize,
    shortest: usize,
}

impl Teddy {
    pub fn new(literals: &[Vec<u8>]) -> Option<Self> {
        if !available() || literals.is_empty() || literals.len() > 64 {
            return None;
        }
        let shortest = literals.iter().map(Vec::len).min()?;
        if shortest == 0 {
            return None;
        }
        let fingerprint = shortest.min(3);
        let mut order: Vec<usize> = (0..literals.len()).collect();
        order.sort_by(|&a, &b| literals[a][..fingerprint].cmp(&literals[b][..fingerprint]));
        let mut buckets: [Vec<usize>; 8] = Default::default();
        let mut bucket = 0;
        for (position, &index) in order.iter().enumerate() {
            if position > 0
                && literals[order[position - 1]][..fingerprint] != literals[index][..fingerprint]
            {
                bucket = (bucket + 1) % 8;
            }
            buckets[bucket].push(index);
        }
        let mut low = [[0u8; 16]; 3];
        let mut high = [[0u8; 16]; 3];
        for (bucket, members) in buckets.iter().enumerate() {
            for &index in members {
                for offset in 0..fingerprint {
                    let byte = literals[index][offset];
                    low[offset][usize::from(byte & 0x0F)] |= 1 << bucket;
                    high[offset][usize::from(byte >> 4)] |= 1 << bucket;
                }
            }
        }
        Some(Self {
            literals: literals.to_vec(),
            buckets,
            low,
            high,
            fingerprint,
            shortest,
        })
    }

    #[inline]
    fn verify(&self, haystack: &[u8], at: usize, end: usize, bits: u8) -> Option<usize> {
        let window = &haystack[at..end];
        let mut best: Option<usize> = None;
        let mut bits = bits;
        while bits != 0 {
            let bucket = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            for &index in &self.buckets[bucket] {
                if best.is_some_and(|best| best < index) {
                    continue;
                }
                if window.starts_with(&self.literals[index]) {
                    best = Some(index);
                }
            }
        }
        best
    }

    #[inline]
    fn bits_at(&self, haystack: &[u8], at: usize) -> u8 {
        let mut bits = u8::MAX;
        for (offset, &byte) in haystack[at..at + self.fingerprint].iter().enumerate() {
            bits &= self.low[offset][usize::from(byte & 0x0F)] & self.high[offset][usize::from(byte >> 4)];
        }
        bits
    }

    pub fn literal_len(&self, index: usize) -> usize {
        self.literals[index].len()
    }

    pub fn find(&self, haystack: &[u8], start: usize, end: usize) -> Option<(usize, usize)> {
        if end < start + self.shortest {
            return None;
        }
        let at = imp::find(self, haystack, start, end);
        let last = end - self.shortest;
        let mut at = match at {
            Ok(found) => return found,
            Err(at) => at,
        };
        while at <= last {
            let bits = self.bits_at(haystack, at);
            if bits != 0 {
                if let Some(index) = self.verify(haystack, at, end, bits) {
                    return Some((at, index));
                }
            }
            at += 1;
        }
        None
    }
}

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
#[inline(always)]
unsafe fn classify<V: Vector>(chunk: V, low: V, high: V, nibble: V) -> V {
    low.shuffle(chunk.and(nibble)).and(high.shuffle(chunk.high_nibbles()))
}

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
#[inline(always)]
unsafe fn search<V: Vector, const F: usize>(
    teddy: &Teddy,
    haystack: &[u8],
    start: usize,
    end: usize,
) -> Result<Option<(usize, usize)>, usize> {
    let pointer = haystack.as_ptr();
    let nibble = V::splat(0x0F);
    let low = [
        V::table(&teddy.low[0]),
        V::table(&teddy.low[1]),
        V::table(&teddy.low[2]),
    ];
    let high = [
        V::table(&teddy.high[0]),
        V::table(&teddy.high[1]),
        V::table(&teddy.high[2]),
    ];
    let last = end - teddy.shortest;
    let mut at = start;
    let mut lanes = [0u8; 32];
    while at + F - 1 + V::BYTES <= end {
        let mut result = classify(V::load_unaligned(pointer.add(at)), low[0], high[0], nibble);
        if F > 1 {
            result = result.and(classify(
                V::load_unaligned(pointer.add(at + 1)),
                low[1],
                high[1],
                nibble,
            ));
        }
        if F > 2 {
            result = result.and(classify(
                V::load_unaligned(pointer.add(at + 2)),
                low[2],
                high[2],
                nibble,
            ));
        }
        let mut mask = result.nonzero();
        if mask.has_non_zero() {
            result.store(&mut lanes);
            while mask.has_non_zero() {
                let lane = mask.first_offset();
                let position = at + lane;
                if position > last {
                    return Ok(None);
                }
                if let Some(index) = teddy.verify(haystack, position, end, lanes[lane]) {
                    return Ok(Some((position, index)));
                }
                mask = mask.clear_least_significant_bit();
            }
        }
        at += V::BYTES;
    }
    Err(at)
}

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
#[inline(always)]
unsafe fn dispatch<V: Vector>(
    teddy: &Teddy,
    haystack: &[u8],
    start: usize,
    end: usize,
) -> Result<Option<(usize, usize)>, usize> {
    match teddy.fingerprint {
        1 => search::<V, 1>(teddy, haystack, start, end),
        2 => search::<V, 2>(teddy, haystack, start, end),
        _ => search::<V, 3>(teddy, haystack, start, end),
    }
}

#[cfg(target_arch = "aarch64")]
const fn available() -> bool {
    true
}

#[cfg(target_arch = "aarch64")]
mod imp {
    use super::super::vector::neon::Neon;
    use super::{dispatch, Teddy};

    pub fn find(
        teddy: &Teddy,
        haystack: &[u8],
        start: usize,
        end: usize,
    ) -> Result<Option<(usize, usize)>, usize> {
        assert!(end <= haystack.len() && start + teddy.shortest <= end);
        unsafe { dispatch::<Neon>(teddy, haystack, start, end) }
    }
}

#[cfg(target_arch = "x86_64")]
fn available() -> bool {
    std::is_x86_feature_detected!("ssse3")
}

#[cfg(target_arch = "x86_64")]
mod imp {
    use super::super::vector::x86::{Avx2, Sse2};
    use super::{dispatch, Teddy};

    #[target_feature(enable = "ssse3")]
    unsafe fn find_ssse3(
        teddy: &Teddy,
        haystack: &[u8],
        start: usize,
        end: usize,
    ) -> Result<Option<(usize, usize)>, usize> {
        dispatch::<Sse2>(teddy, haystack, start, end)
    }

    #[target_feature(enable = "avx2")]
    unsafe fn find_avx2(
        teddy: &Teddy,
        haystack: &[u8],
        start: usize,
        end: usize,
    ) -> Result<Option<(usize, usize)>, usize> {
        dispatch::<Avx2>(teddy, haystack, start, end)
    }

    pub fn find(
        teddy: &Teddy,
        haystack: &[u8],
        start: usize,
        end: usize,
    ) -> Result<Option<(usize, usize)>, usize> {
        assert!(end <= haystack.len() && start + teddy.shortest <= end);
        unsafe {
            if end - start >= 64 && std::is_x86_feature_detected!("avx2") {
                find_avx2(teddy, haystack, start, end)
            } else {
                find_ssse3(teddy, haystack, start, end)
            }
        }
    }
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
const fn available() -> bool {
    false
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
mod imp {
    use super::Teddy;

    pub const fn find(
        _teddy: &Teddy,
        _haystack: &[u8],
        start: usize,
        _end: usize,
    ) -> Result<Option<(usize, usize)>, usize> {
        Err(start)
    }
}
