#![allow(clippy::cast_possible_truncation)]

use super::hir::{Class, Hir};
use crate::memchr::memmem::{CaseInsensitiveFinder, Finder};
use crate::memchr::teddy::Teddy;
use crate::memchr::{memchr, memchr2, memchr3, RANK};

#[derive(Clone, Debug)]
pub enum Prefilter {
    Byte(u8),
    Bytes2(u8, u8),
    Bytes3(u8, u8, u8),
    Substring(Finder<'static>),
    CaseInsensitive(CaseInsensitiveFinder<'static>),
    Teddy(Teddy),
}

impl Prefilter {
    pub fn find(&self, haystack: &[u8], start: usize, end: usize) -> Option<usize> {
        let window = &haystack[start..end];
        let found = match self {
            Self::Byte(a) => memchr(*a, window),
            Self::Bytes2(a, b) => memchr2(*a, *b, window),
            Self::Bytes3(a, b, c) => memchr3(*a, *b, *c, window),
            Self::Substring(finder) => finder.find(window),
            Self::CaseInsensitive(finder) => finder.find(window),
            Self::Teddy(teddy) => return teddy.find(haystack, start, end).map(|found| found.0),
        };
        found.map(|offset| start + offset)
    }
}

#[derive(Clone, Debug)]
pub enum Exact {
    Literal(Finder<'static>, usize),
    CaseInsensitive(CaseInsensitiveFinder<'static>, usize),
    Teddy(Teddy),
}

#[derive(Clone, Debug)]
struct Lit {
    bytes: Vec<u8>,
    exact: bool,
}

const LIMIT: usize = 64;
const MAX_LEN: usize = 48;
const COMMON: u8 = 240;

fn class_literals(class: &Class) -> Option<Vec<Lit>> {
    if class.count() > 16 {
        return None;
    }
    let mut out = Vec::new();
    for &(lo, hi) in &class.ranges {
        for value in lo..=hi {
            if class.unicode {
                let Some(c) = char::from_u32(value) else {
                    continue;
                };
                let mut buffer = [0u8; 4];
                out.push(Lit {
                    bytes: c.encode_utf8(&mut buffer).as_bytes().to_vec(),
                    exact: true,
                });
            } else {
                out.push(Lit {
                    bytes: vec![value as u8],
                    exact: true,
                });
            }
        }
    }
    Some(out)
}

fn cross(left: Vec<Lit>, right: &[Lit]) -> Result<Vec<Lit>, Vec<Lit>> {
    let mut size = 0;
    for lit in &left {
        size += if lit.exact { right.len() } else { 1 };
    }
    if size > LIMIT || left.iter().any(|lit| lit.exact && lit.bytes.len() >= MAX_LEN) {
        return Err(left);
    }
    let mut out = Vec::with_capacity(size);
    for lit in left {
        if lit.exact {
            for suffix in right {
                let mut bytes = lit.bytes.clone();
                bytes.extend_from_slice(&suffix.bytes);
                out.push(Lit {
                    bytes,
                    exact: suffix.exact,
                });
            }
        } else {
            out.push(lit);
        }
    }
    Ok(out)
}

fn inexact(mut lits: Vec<Lit>) -> Vec<Lit> {
    for lit in &mut lits {
        lit.exact = false;
    }
    lits
}

fn concat<'h>(items: impl Iterator<Item = &'h Hir>) -> Vec<Lit> {
    let mut acc = vec![Lit {
        bytes: Vec::new(),
        exact: true,
    }];
    for item in items {
        if acc.iter().all(|lit| !lit.exact) {
            break;
        }
        let Some(next) = prefixes(item) else {
            return inexact(acc);
        };
        match cross(acc, &next) {
            Ok(result) => acc = result,
            Err(previous) => return inexact(previous),
        }
    }
    acc
}

fn prefixes(hir: &Hir) -> Option<Vec<Lit>> {
    match hir {
        Hir::Empty | Hir::Look(_) => Some(vec![Lit {
            bytes: Vec::new(),
            exact: true,
        }]),
        Hir::Literal(bytes) => Some(vec![Lit {
            bytes: bytes.clone(),
            exact: true,
        }]),
        Hir::Class(class) => class_literals(class),
        Hir::Capture { sub, .. } => prefixes(sub),
        Hir::Repeat {
            min,
            max,
            greedy,
            sub,
        } => {
            let inner = prefixes(sub)?;
            if *min == 0 {
                let mut out = if *max == Some(1) { inner } else { inexact(inner) };
                let empty = Lit {
                    bytes: Vec::new(),
                    exact: true,
                };
                if *greedy {
                    out.push(empty);
                } else {
                    out.insert(0, empty);
                }
                Some(out)
            } else if *max == Some(*min) && *min <= 8 {
                Some(concat(core::iter::repeat_n(sub.as_ref(), *min as usize)))
            } else {
                Some(inexact(concat(core::iter::repeat_n(sub.as_ref(), *min as usize))))
            }
        }
        Hir::Concat(items) => Some(concat(items.iter())),
        Hir::Alternation(items) => {
            let mut out = Vec::new();
            for item in items {
                out.extend(prefixes(item)?);
                if out.len() > LIMIT {
                    return None;
                }
            }
            Some(out)
        }
    }
}

fn ascii_pair(class: &Class) -> Option<u8> {
    match class.ranges.as_slice() {
        [(a, a_end), (b, b_end)] if a == a_end && b == b_end && *b < 0x80 => {
            let (upper, lower) = (*a as u8, *b as u8);
            (upper.is_ascii_uppercase() && lower == upper.to_ascii_lowercase()).then_some(lower)
        }
        _ => None,
    }
}

fn case_insensitive_prefix(hir: &Hir, out: &mut Vec<u8>, sensitive: &mut bool) -> bool {
    match hir {
        Hir::Empty => true,
        Hir::Literal(bytes) => {
            for &byte in bytes {
                if byte.is_ascii_alphabetic() {
                    *sensitive = true;
                }
                out.push(byte.to_ascii_lowercase());
            }
            true
        }
        Hir::Class(class) => ascii_pair(class).is_some_and(|byte| {
            out.push(byte);
            true
        }),
        Hir::Capture { sub, .. } => case_insensitive_prefix(sub, out, sensitive),
        Hir::Concat(items) => {
            for item in items {
                if !case_insensitive_prefix(item, out, sensitive) {
                    return false;
                }
            }
            true
        }
        _ => false,
    }
}

fn common_prefix(lits: &[Lit]) -> &[u8] {
    let first = &lits[0].bytes;
    let mut length = first.len();
    for lit in &lits[1..] {
        length = length.min(
            first
                .iter()
                .zip(&lit.bytes)
                .take_while(|(a, b)| a == b)
                .count(),
        );
    }
    &first[..length]
}

fn byte_prefilter(bytes: &[u8]) -> Option<Prefilter> {
    if bytes.iter().any(|&byte| RANK[usize::from(byte)] > COMMON) {
        return None;
    }
    match *bytes {
        [a] => Some(Prefilter::Byte(a)),
        [a, b] => Some(Prefilter::Bytes2(a, b)),
        [a, b, c] => Some(Prefilter::Bytes3(a, b, c)),
        _ => None,
    }
}

fn substring(bytes: &[u8]) -> Option<Prefilter> {
    match bytes.len() {
        0 => None,
        1 => byte_prefilter(bytes),
        _ => Some(Prefilter::Substring(Finder::new(bytes).into_owned())),
    }
}

pub fn exact(hir: &Hir) -> Option<Exact> {
    if hir.looks() != 0 {
        return None;
    }
    let mut multiple = None;
    if let Some(lits) = prefixes(hir) {
        if let [lit] = lits.as_slice() {
            if lit.exact && !lit.bytes.is_empty() {
                return Some(Exact::Literal(
                    Finder::new(&lit.bytes).into_owned(),
                    lit.bytes.len(),
                ));
            }
        }
        if lits.len() > 1 && lits.iter().all(|lit| lit.exact && !lit.bytes.is_empty()) {
            multiple = Some(lits);
        }
    }
    let mut folded = Vec::new();
    let mut sensitive = false;
    if case_insensitive_prefix(hir, &mut folded, &mut sensitive) && !sensitive && !folded.is_empty()
    {
        let length = folded.len();
        return Some(Exact::CaseInsensitive(
            CaseInsensitiveFinder::new(&folded).into_owned(),
            length,
        ));
    }
    let lits = multiple?;
    let bytes: Vec<Vec<u8>> = lits.into_iter().map(|lit| lit.bytes).collect();
    Teddy::new(&bytes).map(Exact::Teddy)
}

pub fn prefilter(hir: &Hir) -> Option<Prefilter> {
    if let Some(mut lits) = prefixes(hir) {
        let mut seen = std::collections::HashSet::new();
        lits.retain(|lit| seen.insert(lit.bytes.clone()));
        if !lits.is_empty() && lits.iter().all(|lit| !lit.bytes.is_empty()) {
            if lits.len() == 1 {
                return substring(&lits[0].bytes);
            }
            let common = common_prefix(&lits);
            if common.len() >= 2 {
                return substring(common);
            }
            let folded = lits[0].bytes.to_ascii_lowercase();
            if folded.len() >= 2
                && lits
                    .iter()
                    .all(|lit| lit.bytes.len() == folded.len() && lit.bytes.eq_ignore_ascii_case(&folded))
            {
                return Some(Prefilter::CaseInsensitive(
                    CaseInsensitiveFinder::new(&folded).into_owned(),
                ));
            }
            let bytes: Vec<Vec<u8>> = lits.iter().map(|lit| lit.bytes.clone()).collect();
            let shortest = bytes.iter().map(Vec::len).min().unwrap_or(0);
            if shortest >= 2 {
                if let Some(teddy) = Teddy::new(&bytes) {
                    return Some(Prefilter::Teddy(teddy));
                }
            }
            let mut firsts: Vec<u8> = lits.iter().map(|lit| lit.bytes[0]).collect();
            firsts.sort_unstable();
            firsts.dedup();
            if let Some(prefilter) = byte_prefilter(&firsts) {
                return Some(prefilter);
            }
            if firsts.iter().all(|&byte| RANK[usize::from(byte)] <= COMMON) {
                if let Some(teddy) = Teddy::new(&bytes) {
                    return Some(Prefilter::Teddy(teddy));
                }
            }
        }
    }
    let mut folded = Vec::new();
    let mut sensitive = false;
    case_insensitive_prefix(hir, &mut folded, &mut sensitive);
    if folded.len() >= 2 {
        return Some(Prefilter::CaseInsensitive(
            CaseInsensitiveFinder::new(&folded).into_owned(),
        ));
    }
    None
}

pub struct Inner {
    pub prefix: Hir,
    pub literal: Vec<u8>,
}

fn alphabet(hir: &Hir, out: &mut [bool; 256]) {
    match hir {
        Hir::Empty | Hir::Look(_) => {}
        Hir::Literal(bytes) => {
            for &byte in bytes {
                out[usize::from(byte)] = true;
            }
        }
        Hir::Class(class) => {
            for &(lo, hi) in &class.ranges {
                if lo > 0xFF {
                    continue;
                }
                let top = if class.unicode && hi > 0x7F { 0xFF } else { hi.min(0xFF) };
                for byte in lo..=top {
                    out[byte as usize] = true;
                }
            }
            if class.unicode && class.ranges.last().is_some_and(|&(_, hi)| hi > 0x7F) {
                for slot in &mut out[0x80..] {
                    *slot = true;
                }
            }
        }
        Hir::Repeat { sub, .. } | Hir::Capture { sub, .. } => alphabet(sub, out),
        Hir::Concat(items) | Hir::Alternation(items) => {
            for item in items {
                alphabet(item, out);
            }
        }
    }
}

pub fn inner(hir: &Hir) -> Option<Inner> {
    let mut hir = hir;
    while let Hir::Capture { sub, .. } = hir {
        hir = sub;
    }
    let Hir::Concat(items) = hir else {
        return None;
    };
    let mut best: Option<(usize, usize, Vec<u8>)> = None;
    for index in 1..items.len() {
        let lits = concat(items[index..].iter());
        let [lit] = lits.as_slice() else {
            continue;
        };
        if lit.bytes.is_empty() {
            continue;
        }
        let mut seen = [false; 256];
        for item in &items[..index] {
            alphabet(item, &mut seen);
        }
        if seen[usize::from(lit.bytes[0])] {
            continue;
        }
        let score = if lit.bytes.len() >= 2 {
            1000 + lit.bytes.len()
        } else {
            255 - usize::from(RANK[usize::from(lit.bytes[0])])
        };
        if best.as_ref().is_none_or(|(current, _, _)| score > *current) {
            best = Some((score, index, lit.bytes.clone()));
        }
    }
    let (_, index, literal) = best?;
    Some(Inner {
        prefix: Hir::concat(items[..index].to_vec()),
        literal,
    })
}
