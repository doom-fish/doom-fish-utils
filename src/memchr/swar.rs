use core::mem::size_of;

const WORD: usize = size_of::<usize>();
const LOW: usize = usize::from_ne_bytes([0x01; WORD]);
const HIGH: usize = usize::from_ne_bytes([0x80; WORD]);

#[inline]
const fn has_zero_byte(word: usize) -> bool {
    word.wrapping_sub(LOW) & !word & HIGH != 0
}

#[inline]
const fn splat(byte: u8) -> usize {
    LOW * byte as usize
}

#[inline]
fn read(haystack: &[u8], at: usize) -> usize {
    let mut bytes = [0u8; WORD];
    bytes.copy_from_slice(&haystack[at..at + WORD]);
    usize::from_ne_bytes(bytes)
}

#[inline]
fn any_needle(word: usize, needles: &[usize]) -> bool {
    needles.iter().any(|&needle| has_zero_byte(word ^ needle))
}

pub fn find(haystack: &[u8], bytes: &[u8]) -> Option<usize> {
    let mut needles = [0usize; 3];
    for (slot, &byte) in needles.iter_mut().zip(bytes) {
        *slot = splat(byte);
    }
    let needles = &needles[..bytes.len()];
    let mut at = 0;
    while at + 2 * WORD <= haystack.len() {
        if any_needle(read(haystack, at), needles) || any_needle(read(haystack, at + WORD), needles)
        {
            break;
        }
        at += 2 * WORD;
    }
    haystack[at..]
        .iter()
        .position(|byte| bytes.contains(byte))
        .map(|offset| at + offset)
}

pub fn rfind(haystack: &[u8], bytes: &[u8]) -> Option<usize> {
    let mut needles = [0usize; 3];
    for (slot, &byte) in needles.iter_mut().zip(bytes) {
        *slot = splat(byte);
    }
    let needles = &needles[..bytes.len()];
    let mut end = haystack.len();
    while end >= 2 * WORD {
        if any_needle(read(haystack, end - WORD), needles)
            || any_needle(read(haystack, end - 2 * WORD), needles)
        {
            break;
        }
        end -= 2 * WORD;
    }
    haystack[..end].iter().rposition(|byte| bytes.contains(byte))
}
