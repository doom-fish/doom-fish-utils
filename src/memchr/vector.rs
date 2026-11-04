#![allow(clippy::inline_always, dead_code)]

pub trait MoveMask: Copy {
    fn has_non_zero(self) -> bool;
    fn first_offset(self) -> usize;
    fn last_offset(self) -> usize;
    fn clear_least_significant_bit(self) -> Self;
    fn clear_most_significant_bit(self) -> Self;
}

pub trait Vector: Copy {
    const BYTES: usize;
    const ALIGN: usize;
    type Mask: MoveMask;

    unsafe fn splat(byte: u8) -> Self;
    unsafe fn load_aligned(data: *const u8) -> Self;
    unsafe fn load_unaligned(data: *const u8) -> Self;
    unsafe fn movemask(self) -> Self::Mask;
    unsafe fn cmpeq(self, other: Self) -> Self;
    unsafe fn and(self, other: Self) -> Self;
    unsafe fn or(self, other: Self) -> Self;
    unsafe fn any(self) -> bool;
    unsafe fn table(bytes: &[u8; 16]) -> Self;
    unsafe fn shuffle(self, indices: Self) -> Self;
    unsafe fn high_nibbles(self) -> Self;
    unsafe fn nonzero(self) -> Self::Mask;
    unsafe fn store(self, out: &mut [u8; 32]);
}

#[derive(Clone, Copy)]
pub struct BitMask(pub u32);

impl MoveMask for BitMask {
    #[inline(always)]
    fn has_non_zero(self) -> bool {
        self.0 != 0
    }

    #[inline(always)]
    fn first_offset(self) -> usize {
        self.0.trailing_zeros() as usize
    }

    #[inline(always)]
    fn last_offset(self) -> usize {
        31 - self.0.leading_zeros() as usize
    }

    #[inline(always)]
    fn clear_least_significant_bit(self) -> Self {
        Self(self.0 & self.0.wrapping_sub(1))
    }

    #[inline(always)]
    fn clear_most_significant_bit(self) -> Self {
        Self(self.0 & !(1u32 << self.0.ilog2()))
    }
}

#[cfg(target_arch = "aarch64")]
pub mod neon {
    use super::{MoveMask, Vector};
    use core::arch::aarch64::{
        uint8x16_t, vandq_u8, vceqq_u8, vdupq_n_u8, vget_lane_u64, vgetq_lane_u64, vld1q_u8,
        vorrq_u8, vpmaxq_u8, vqtbl1q_u8, vreinterpret_u64_u8, vreinterpretq_u16_u8,
        vreinterpretq_u64_u8, vshrn_n_u16, vshrq_n_u8, vst1q_u8, vtstq_u8,
    };

    #[derive(Clone, Copy)]
    pub struct NibbleMask(u64);

    impl MoveMask for NibbleMask {
        #[inline(always)]
        fn has_non_zero(self) -> bool {
            self.0 != 0
        }

        #[inline(always)]
        fn first_offset(self) -> usize {
            (self.0.trailing_zeros() >> 2) as usize
        }

        #[inline(always)]
        fn last_offset(self) -> usize {
            15 - (self.0.leading_zeros() >> 2) as usize
        }

        #[inline(always)]
        fn clear_least_significant_bit(self) -> Self {
            Self(self.0 & self.0.wrapping_sub(1))
        }

        #[inline(always)]
        fn clear_most_significant_bit(self) -> Self {
            Self(self.0 & !(1u64 << self.0.ilog2()))
        }
    }

    #[derive(Clone, Copy)]
    pub struct Neon(uint8x16_t);

    impl Vector for Neon {
        const BYTES: usize = 16;
        const ALIGN: usize = 15;
        type Mask = NibbleMask;

        #[inline(always)]
        unsafe fn splat(byte: u8) -> Self {
            Self(vdupq_n_u8(byte))
        }

        #[inline(always)]
        unsafe fn load_aligned(data: *const u8) -> Self {
            Self(vld1q_u8(data))
        }

        #[inline(always)]
        unsafe fn load_unaligned(data: *const u8) -> Self {
            Self(vld1q_u8(data))
        }

        #[inline(always)]
        unsafe fn movemask(self) -> NibbleMask {
            let narrowed = vshrn_n_u16::<4>(vreinterpretq_u16_u8(self.0));
            NibbleMask(vget_lane_u64::<0>(vreinterpret_u64_u8(narrowed)) & 0x8888_8888_8888_8888)
        }

        #[inline(always)]
        unsafe fn cmpeq(self, other: Self) -> Self {
            Self(vceqq_u8(self.0, other.0))
        }

        #[inline(always)]
        unsafe fn and(self, other: Self) -> Self {
            Self(vandq_u8(self.0, other.0))
        }

        #[inline(always)]
        unsafe fn or(self, other: Self) -> Self {
            Self(vorrq_u8(self.0, other.0))
        }

        #[inline(always)]
        unsafe fn any(self) -> bool {
            vgetq_lane_u64::<0>(vreinterpretq_u64_u8(vpmaxq_u8(self.0, self.0))) != 0
        }

        #[inline(always)]
        unsafe fn table(bytes: &[u8; 16]) -> Self {
            Self(vld1q_u8(bytes.as_ptr()))
        }

        #[inline(always)]
        unsafe fn shuffle(self, indices: Self) -> Self {
            Self(vqtbl1q_u8(self.0, indices.0))
        }

        #[inline(always)]
        unsafe fn high_nibbles(self) -> Self {
            Self(vshrq_n_u8::<4>(self.0))
        }

        #[inline(always)]
        unsafe fn nonzero(self) -> NibbleMask {
            Self(vtstq_u8(self.0, self.0)).movemask()
        }

        #[inline(always)]
        unsafe fn store(self, out: &mut [u8; 32]) {
            vst1q_u8(out.as_mut_ptr(), self.0);
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[allow(clippy::cast_ptr_alignment, clippy::cast_sign_loss, clippy::cast_possible_wrap)]
pub mod x86 {
    use super::{BitMask, Vector};
    use core::arch::x86_64::{
        __m128i, __m256i, _mm256_and_si256, _mm256_broadcastsi128_si256, _mm256_cmpeq_epi8,
        _mm256_load_si256, _mm256_loadu_si256, _mm256_movemask_epi8, _mm256_or_si256,
        _mm256_set1_epi8, _mm256_setzero_si256, _mm256_shuffle_epi8, _mm256_srli_epi16,
        _mm256_storeu_si256, _mm_and_si128, _mm_cmpeq_epi8, _mm_load_si128, _mm_loadu_si128,
        _mm_movemask_epi8, _mm_or_si128, _mm_set1_epi8, _mm_setzero_si128, _mm_shuffle_epi8,
        _mm_srli_epi16, _mm_storeu_si128,
    };

    #[derive(Clone, Copy)]
    pub struct Sse2(__m128i);

    impl Vector for Sse2 {
        const BYTES: usize = 16;
        const ALIGN: usize = 15;
        type Mask = BitMask;

        #[inline(always)]
        unsafe fn splat(byte: u8) -> Self {
            Self(_mm_set1_epi8(byte as i8))
        }

        #[inline(always)]
        unsafe fn load_aligned(data: *const u8) -> Self {
            Self(_mm_load_si128(data.cast::<__m128i>()))
        }

        #[inline(always)]
        unsafe fn load_unaligned(data: *const u8) -> Self {
            Self(_mm_loadu_si128(data.cast::<__m128i>()))
        }

        #[inline(always)]
        unsafe fn movemask(self) -> BitMask {
            BitMask(_mm_movemask_epi8(self.0) as u32)
        }

        #[inline(always)]
        unsafe fn cmpeq(self, other: Self) -> Self {
            Self(_mm_cmpeq_epi8(self.0, other.0))
        }

        #[inline(always)]
        unsafe fn and(self, other: Self) -> Self {
            Self(_mm_and_si128(self.0, other.0))
        }

        #[inline(always)]
        unsafe fn or(self, other: Self) -> Self {
            Self(_mm_or_si128(self.0, other.0))
        }

        #[inline(always)]
        unsafe fn any(self) -> bool {
            _mm_movemask_epi8(self.0) != 0
        }

        #[inline(always)]
        unsafe fn table(bytes: &[u8; 16]) -> Self {
            Self(_mm_loadu_si128(bytes.as_ptr().cast::<__m128i>()))
        }

        #[inline(always)]
        unsafe fn shuffle(self, indices: Self) -> Self {
            Self(_mm_shuffle_epi8(self.0, indices.0))
        }

        #[inline(always)]
        unsafe fn high_nibbles(self) -> Self {
            Self(_mm_and_si128(_mm_srli_epi16(self.0, 4), _mm_set1_epi8(0x0F)))
        }

        #[inline(always)]
        unsafe fn nonzero(self) -> BitMask {
            BitMask(!(_mm_movemask_epi8(_mm_cmpeq_epi8(self.0, _mm_setzero_si128())) as u32) & 0xFFFF)
        }

        #[inline(always)]
        unsafe fn store(self, out: &mut [u8; 32]) {
            _mm_storeu_si128(out.as_mut_ptr().cast::<__m128i>(), self.0);
        }
    }

    #[derive(Clone, Copy)]
    pub struct Avx2(__m256i);

    impl Vector for Avx2 {
        const BYTES: usize = 32;
        const ALIGN: usize = 31;
        type Mask = BitMask;

        #[inline(always)]
        unsafe fn splat(byte: u8) -> Self {
            Self(_mm256_set1_epi8(byte as i8))
        }

        #[inline(always)]
        unsafe fn load_aligned(data: *const u8) -> Self {
            Self(_mm256_load_si256(data.cast::<__m256i>()))
        }

        #[inline(always)]
        unsafe fn load_unaligned(data: *const u8) -> Self {
            Self(_mm256_loadu_si256(data.cast::<__m256i>()))
        }

        #[inline(always)]
        unsafe fn movemask(self) -> BitMask {
            BitMask(_mm256_movemask_epi8(self.0) as u32)
        }

        #[inline(always)]
        unsafe fn cmpeq(self, other: Self) -> Self {
            Self(_mm256_cmpeq_epi8(self.0, other.0))
        }

        #[inline(always)]
        unsafe fn and(self, other: Self) -> Self {
            Self(_mm256_and_si256(self.0, other.0))
        }

        #[inline(always)]
        unsafe fn or(self, other: Self) -> Self {
            Self(_mm256_or_si256(self.0, other.0))
        }

        #[inline(always)]
        unsafe fn any(self) -> bool {
            _mm256_movemask_epi8(self.0) != 0
        }

        #[inline(always)]
        unsafe fn table(bytes: &[u8; 16]) -> Self {
            Self(_mm256_broadcastsi128_si256(_mm_loadu_si128(
                bytes.as_ptr().cast::<__m128i>()
            )))
        }

        #[inline(always)]
        unsafe fn shuffle(self, indices: Self) -> Self {
            Self(_mm256_shuffle_epi8(self.0, indices.0))
        }

        #[inline(always)]
        unsafe fn high_nibbles(self) -> Self {
            Self(_mm256_and_si256(_mm256_srli_epi16(self.0, 4), _mm256_set1_epi8(0x0F)))
        }

        #[inline(always)]
        unsafe fn nonzero(self) -> BitMask {
            BitMask(!(_mm256_movemask_epi8(_mm256_cmpeq_epi8(self.0, _mm256_setzero_si256())) as u32))
        }

        #[inline(always)]
        unsafe fn store(self, out: &mut [u8; 32]) {
            _mm256_storeu_si256(out.as_mut_ptr().cast::<__m256i>(), self.0);
        }
    }
}
