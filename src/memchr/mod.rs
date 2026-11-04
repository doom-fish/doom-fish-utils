pub mod memmem;
mod rank;
#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
mod simd;
#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
mod swar;
#[cfg(feature = "regex")]
pub(crate) mod teddy;
mod twoway;
#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
mod vector;

use core::iter::{FusedIterator, Rev};

pub(crate) use rank::RANK;

#[cfg(target_arch = "aarch64")]
mod arch {
    use super::simd::{self, One, Pair, Three, Two};
    use super::vector::neon::Neon;

    pub const PAIR_VECTOR: Option<usize> = Some(16);

    pub fn find1(haystack: &[u8], a: u8) -> Option<usize> {
        unsafe { simd::find::<Neon, _>(haystack, One::new(a)) }
    }

    pub fn find2(haystack: &[u8], a: u8, b: u8) -> Option<usize> {
        unsafe { simd::find::<Neon, _>(haystack, Two::new(a, b)) }
    }

    pub fn find3(haystack: &[u8], a: u8, b: u8, c: u8) -> Option<usize> {
        unsafe { simd::find::<Neon, _>(haystack, Three::new(a, b, c)) }
    }

    pub fn rfind1(haystack: &[u8], a: u8) -> Option<usize> {
        unsafe { simd::rfind::<Neon, _>(haystack, One::new(a)) }
    }

    pub fn rfind2(haystack: &[u8], a: u8, b: u8) -> Option<usize> {
        unsafe { simd::rfind::<Neon, _>(haystack, Two::new(a, b)) }
    }

    pub fn rfind3(haystack: &[u8], a: u8, b: u8, c: u8) -> Option<usize> {
        unsafe { simd::rfind::<Neon, _>(haystack, Three::new(a, b, c)) }
    }

    pub fn find_pair<F: FnMut(usize) -> Option<bool>>(
        haystack: &[u8],
        first: usize,
        last: usize,
        pair: &Pair,
        confirm: F,
    ) -> Result<Option<usize>, usize> {
        assert!(haystack.len() >= pair.reach() + 16 && last + pair.reach() < haystack.len());
        unsafe { simd::find_pair::<Neon, F>(haystack, first, last, pair, confirm) }
    }

    pub fn rfind_pair<F: FnMut(usize) -> Option<bool>>(
        haystack: &[u8],
        first: usize,
        last: usize,
        pair: &Pair,
        confirm: F,
    ) -> Result<Option<usize>, usize> {
        assert!(haystack.len() >= pair.reach() + 16 && last + pair.reach() < haystack.len());
        unsafe { simd::rfind_pair::<Neon, F>(haystack, first, last, pair, confirm) }
    }
}

#[cfg(target_arch = "x86_64")]
mod arch {
    use super::simd::{self, One, Pair, Three, Two};
    use super::vector::x86::{Avx2, Sse2};

    pub const PAIR_VECTOR: Option<usize> = Some(16);

    #[inline]
    fn avx2(length: usize) -> bool {
        length >= 32 && std::is_x86_feature_detected!("avx2")
    }

    #[target_feature(enable = "avx2")]
    unsafe fn find1_avx2(haystack: &[u8], a: u8) -> Option<usize> {
        simd::find::<Avx2, _>(haystack, One::new(a))
    }

    #[target_feature(enable = "avx2")]
    unsafe fn find2_avx2(haystack: &[u8], a: u8, b: u8) -> Option<usize> {
        simd::find::<Avx2, _>(haystack, Two::new(a, b))
    }

    #[target_feature(enable = "avx2")]
    unsafe fn find3_avx2(haystack: &[u8], a: u8, b: u8, c: u8) -> Option<usize> {
        simd::find::<Avx2, _>(haystack, Three::new(a, b, c))
    }

    #[target_feature(enable = "avx2")]
    unsafe fn rfind1_avx2(haystack: &[u8], a: u8) -> Option<usize> {
        simd::rfind::<Avx2, _>(haystack, One::new(a))
    }

    #[target_feature(enable = "avx2")]
    unsafe fn rfind2_avx2(haystack: &[u8], a: u8, b: u8) -> Option<usize> {
        simd::rfind::<Avx2, _>(haystack, Two::new(a, b))
    }

    #[target_feature(enable = "avx2")]
    unsafe fn rfind3_avx2(haystack: &[u8], a: u8, b: u8, c: u8) -> Option<usize> {
        simd::rfind::<Avx2, _>(haystack, Three::new(a, b, c))
    }

    #[target_feature(enable = "avx2")]
    unsafe fn find_pair_avx2<F: FnMut(usize) -> Option<bool>>(
        haystack: &[u8],
        first: usize,
        last: usize,
        pair: &Pair,
        confirm: F,
    ) -> Result<Option<usize>, usize> {
        simd::find_pair::<Avx2, F>(haystack, first, last, pair, confirm)
    }

    #[target_feature(enable = "avx2")]
    unsafe fn rfind_pair_avx2<F: FnMut(usize) -> Option<bool>>(
        haystack: &[u8],
        first: usize,
        last: usize,
        pair: &Pair,
        confirm: F,
    ) -> Result<Option<usize>, usize> {
        simd::rfind_pair::<Avx2, F>(haystack, first, last, pair, confirm)
    }

    pub fn find1(haystack: &[u8], a: u8) -> Option<usize> {
        unsafe {
            if avx2(haystack.len()) {
                find1_avx2(haystack, a)
            } else {
                simd::find::<Sse2, _>(haystack, One::new(a))
            }
        }
    }

    pub fn find2(haystack: &[u8], a: u8, b: u8) -> Option<usize> {
        unsafe {
            if avx2(haystack.len()) {
                find2_avx2(haystack, a, b)
            } else {
                simd::find::<Sse2, _>(haystack, Two::new(a, b))
            }
        }
    }

    pub fn find3(haystack: &[u8], a: u8, b: u8, c: u8) -> Option<usize> {
        unsafe {
            if avx2(haystack.len()) {
                find3_avx2(haystack, a, b, c)
            } else {
                simd::find::<Sse2, _>(haystack, Three::new(a, b, c))
            }
        }
    }

    pub fn rfind1(haystack: &[u8], a: u8) -> Option<usize> {
        unsafe {
            if avx2(haystack.len()) {
                rfind1_avx2(haystack, a)
            } else {
                simd::rfind::<Sse2, _>(haystack, One::new(a))
            }
        }
    }

    pub fn rfind2(haystack: &[u8], a: u8, b: u8) -> Option<usize> {
        unsafe {
            if avx2(haystack.len()) {
                rfind2_avx2(haystack, a, b)
            } else {
                simd::rfind::<Sse2, _>(haystack, Two::new(a, b))
            }
        }
    }

    pub fn rfind3(haystack: &[u8], a: u8, b: u8, c: u8) -> Option<usize> {
        unsafe {
            if avx2(haystack.len()) {
                rfind3_avx2(haystack, a, b, c)
            } else {
                simd::rfind::<Sse2, _>(haystack, Three::new(a, b, c))
            }
        }
    }

    pub fn find_pair<F: FnMut(usize) -> Option<bool>>(
        haystack: &[u8],
        first: usize,
        last: usize,
        pair: &Pair,
        confirm: F,
    ) -> Result<Option<usize>, usize> {
        assert!(haystack.len() >= pair.reach() + 16 && last + pair.reach() < haystack.len());
        unsafe {
            if haystack.len() >= pair.reach() + 32 && avx2(haystack.len()) {
                find_pair_avx2(haystack, first, last, pair, confirm)
            } else {
                simd::find_pair::<Sse2, F>(haystack, first, last, pair, confirm)
            }
        }
    }

    pub fn rfind_pair<F: FnMut(usize) -> Option<bool>>(
        haystack: &[u8],
        first: usize,
        last: usize,
        pair: &Pair,
        confirm: F,
    ) -> Result<Option<usize>, usize> {
        assert!(haystack.len() >= pair.reach() + 16 && last + pair.reach() < haystack.len());
        unsafe {
            if haystack.len() >= pair.reach() + 32 && avx2(haystack.len()) {
                rfind_pair_avx2(haystack, first, last, pair, confirm)
            } else {
                simd::rfind_pair::<Sse2, F>(haystack, first, last, pair, confirm)
            }
        }
    }
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
mod arch {
    use super::swar;

    pub const PAIR_VECTOR: Option<usize> = Some(1);

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
        pub const fn reach(&self) -> usize {
            if self.index1 > self.index2 {
                self.index1
            } else {
                self.index2
            }
        }
    }

    pub fn find1(haystack: &[u8], a: u8) -> Option<usize> {
        swar::find(haystack, &[a])
    }

    pub fn find2(haystack: &[u8], a: u8, b: u8) -> Option<usize> {
        swar::find(haystack, &[a, b])
    }

    pub fn find3(haystack: &[u8], a: u8, b: u8, c: u8) -> Option<usize> {
        swar::find(haystack, &[a, b, c])
    }

    pub fn rfind1(haystack: &[u8], a: u8) -> Option<usize> {
        swar::rfind(haystack, &[a])
    }

    pub fn rfind2(haystack: &[u8], a: u8, b: u8) -> Option<usize> {
        swar::rfind(haystack, &[a, b])
    }

    pub fn rfind3(haystack: &[u8], a: u8, b: u8, c: u8) -> Option<usize> {
        swar::rfind(haystack, &[a, b, c])
    }

    const fn byte_set(pair: &Pair) -> ([u8; 2], usize) {
        if pair.fold1 == 0 {
            ([pair.byte1, pair.byte1], 1)
        } else {
            ([pair.byte1, pair.byte1 & !0x20], 2)
        }
    }

    pub fn find_pair<F: FnMut(usize) -> Option<bool>>(
        haystack: &[u8],
        first: usize,
        last: usize,
        pair: &Pair,
        mut confirm: F,
    ) -> Result<Option<usize>, usize> {
        let (bytes, count) = byte_set(pair);
        let mut at = first;
        while at <= last {
            let window = &haystack[at + pair.index1..=last + pair.index1];
            let Some(offset) = swar::find(window, &bytes[..count]) else {
                return Ok(None);
            };
            let candidate = at + offset;
            if (haystack[candidate + pair.index2] | pair.fold2) == pair.byte2 {
                match confirm(candidate) {
                    Some(true) => return Ok(Some(candidate)),
                    Some(false) => {}
                    None => return Err(candidate),
                }
            }
            at = candidate + 1;
        }
        Ok(None)
    }

    pub fn rfind_pair<F: FnMut(usize) -> Option<bool>>(
        haystack: &[u8],
        first: usize,
        last: usize,
        pair: &Pair,
        mut confirm: F,
    ) -> Result<Option<usize>, usize> {
        let (bytes, count) = byte_set(pair);
        let mut end = last + 1;
        while end > first {
            let window = &haystack[first + pair.index1..end + pair.index1];
            let Some(offset) = swar::rfind(window, &bytes[..count]) else {
                return Ok(None);
            };
            let candidate = first + offset;
            if (haystack[candidate + pair.index2] | pair.fold2) == pair.byte2 {
                match confirm(candidate) {
                    Some(true) => return Ok(Some(candidate)),
                    Some(false) => {}
                    None => return Err(candidate),
                }
            }
            end = candidate;
        }
        Ok(None)
    }
}

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
pub(crate) use simd::Pair;

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
pub(crate) use arch::Pair;

pub(crate) use arch::{find_pair, rfind_pair, PAIR_VECTOR};

#[inline]
#[must_use]
pub fn memchr(needle: u8, haystack: &[u8]) -> Option<usize> {
    arch::find1(haystack, needle)
}

#[inline]
#[must_use]
pub fn memchr2(needle1: u8, needle2: u8, haystack: &[u8]) -> Option<usize> {
    arch::find2(haystack, needle1, needle2)
}

#[inline]
#[must_use]
pub fn memchr3(needle1: u8, needle2: u8, needle3: u8, haystack: &[u8]) -> Option<usize> {
    arch::find3(haystack, needle1, needle2, needle3)
}

#[inline]
#[must_use]
pub fn memrchr(needle: u8, haystack: &[u8]) -> Option<usize> {
    arch::rfind1(haystack, needle)
}

#[inline]
#[must_use]
pub fn memrchr2(needle1: u8, needle2: u8, haystack: &[u8]) -> Option<usize> {
    arch::rfind2(haystack, needle1, needle2)
}

#[inline]
#[must_use]
pub fn memrchr3(needle1: u8, needle2: u8, needle3: u8, haystack: &[u8]) -> Option<usize> {
    arch::rfind3(haystack, needle1, needle2, needle3)
}

#[inline]
#[must_use]
pub const fn memchr_iter(needle: u8, haystack: &[u8]) -> Memchr<'_> {
    Memchr::new(needle, haystack)
}

#[inline]
pub fn memrchr_iter(needle: u8, haystack: &[u8]) -> Rev<Memchr<'_>> {
    Memchr::new(needle, haystack).rev()
}

#[inline]
#[must_use]
pub const fn memchr2_iter(needle1: u8, needle2: u8, haystack: &[u8]) -> Memchr2<'_> {
    Memchr2::new(needle1, needle2, haystack)
}

#[inline]
pub fn memrchr2_iter(needle1: u8, needle2: u8, haystack: &[u8]) -> Rev<Memchr2<'_>> {
    Memchr2::new(needle1, needle2, haystack).rev()
}

#[inline]
#[must_use]
pub const fn memchr3_iter(needle1: u8, needle2: u8, needle3: u8, haystack: &[u8]) -> Memchr3<'_> {
    Memchr3::new(needle1, needle2, needle3, haystack)
}

#[inline]
pub fn memrchr3_iter(
    needle1: u8,
    needle2: u8,
    needle3: u8,
    haystack: &[u8],
) -> Rev<Memchr3<'_>> {
    Memchr3::new(needle1, needle2, needle3, haystack).rev()
}

#[derive(Clone, Debug)]
pub struct Memchr<'h> {
    needle: u8,
    haystack: &'h [u8],
    start: usize,
    end: usize,
}

#[derive(Clone, Debug)]
pub struct Memchr2<'h> {
    needles: [u8; 2],
    haystack: &'h [u8],
    start: usize,
    end: usize,
}

#[derive(Clone, Debug)]
pub struct Memchr3<'h> {
    needles: [u8; 3],
    haystack: &'h [u8],
    start: usize,
    end: usize,
}

impl<'h> Memchr<'h> {
    #[inline]
    #[must_use]
    pub const fn new(needle: u8, haystack: &'h [u8]) -> Self {
        Self {
            needle,
            haystack,
            start: 0,
            end: haystack.len(),
        }
    }
}

impl<'h> Memchr2<'h> {
    #[inline]
    #[must_use]
    pub const fn new(needle1: u8, needle2: u8, haystack: &'h [u8]) -> Self {
        Self {
            needles: [needle1, needle2],
            haystack,
            start: 0,
            end: haystack.len(),
        }
    }
}

impl<'h> Memchr3<'h> {
    #[inline]
    #[must_use]
    pub const fn new(needle1: u8, needle2: u8, needle3: u8, haystack: &'h [u8]) -> Self {
        Self {
            needles: [needle1, needle2, needle3],
            haystack,
            start: 0,
            end: haystack.len(),
        }
    }
}

macro_rules! byte_iterator {
    ($name:ident, $forward:expr, $backward:expr) => {
        impl Iterator for $name<'_> {
            type Item = usize;

            #[inline]
            fn next(&mut self) -> Option<usize> {
                let window = &self.haystack[self.start..self.end];
                let at = self.start + ($forward)(&*self, window)?;
                self.start = at + 1;
                Some(at)
            }

            #[inline]
            fn size_hint(&self) -> (usize, Option<usize>) {
                (0, Some(self.end - self.start))
            }
        }

        impl DoubleEndedIterator for $name<'_> {
            #[inline]
            fn next_back(&mut self) -> Option<usize> {
                let window = &self.haystack[self.start..self.end];
                let at = self.start + ($backward)(&*self, window)?;
                self.end = at;
                Some(at)
            }
        }

        impl FusedIterator for $name<'_> {}
    };
}

byte_iterator!(
    Memchr,
    |this: &Memchr<'_>, window| memchr(this.needle, window),
    |this: &Memchr<'_>, window| memrchr(this.needle, window)
);

byte_iterator!(
    Memchr2,
    |this: &Memchr2<'_>, window| memchr2(this.needles[0], this.needles[1], window),
    |this: &Memchr2<'_>, window| memrchr2(this.needles[0], this.needles[1], window)
);

byte_iterator!(
    Memchr3,
    |this: &Memchr3<'_>, window| memchr3(this.needles[0], this.needles[1], this.needles[2], window),
    |this: &Memchr3<'_>, window| memrchr3(this.needles[0], this.needles[1], this.needles[2], window)
);
