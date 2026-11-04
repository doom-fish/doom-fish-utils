use super::unicode::{self, MAX_CHAR};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Look {
    Start,
    End,
    StartLine,
    EndLine,
    WordAscii,
    WordAsciiNegate,
    WordUnicode,
    WordUnicodeNegate,
    WordStartAscii,
    WordEndAscii,
    WordStartUnicode,
    WordEndUnicode,
    WordStartHalfAscii,
    WordEndHalfAscii,
    WordStartHalfUnicode,
    WordEndHalfUnicode,
}

impl Look {
    pub const fn bit(self) -> u32 {
        1 << self as u32
    }

    pub const fn reversed(self) -> Self {
        match self {
            Self::Start => Self::End,
            Self::End => Self::Start,
            Self::StartLine => Self::EndLine,
            Self::EndLine => Self::StartLine,
            Self::WordStartAscii => Self::WordEndAscii,
            Self::WordEndAscii => Self::WordStartAscii,
            Self::WordStartUnicode => Self::WordEndUnicode,
            Self::WordEndUnicode => Self::WordStartUnicode,
            Self::WordStartHalfAscii => Self::WordEndHalfAscii,
            Self::WordEndHalfAscii => Self::WordStartHalfAscii,
            Self::WordStartHalfUnicode => Self::WordEndHalfUnicode,
            Self::WordEndHalfUnicode => Self::WordStartHalfUnicode,
            other => other,
        }
    }

    pub const fn is_unicode_word(self) -> bool {
        matches!(
            self,
            Self::WordUnicode
                | Self::WordUnicodeNegate
                | Self::WordStartUnicode
                | Self::WordEndUnicode
                | Self::WordStartHalfUnicode
                | Self::WordEndHalfUnicode
        )
    }

    pub const fn is_word(self) -> bool {
        !matches!(
            self,
            Self::Start | Self::End | Self::StartLine | Self::EndLine
        )
    }

    pub const fn holds(self, before: bool, after: bool) -> bool {
        match self {
            Self::WordAscii | Self::WordUnicode => before != after,
            Self::WordAsciiNegate | Self::WordUnicodeNegate => before == after,
            Self::WordStartAscii | Self::WordStartUnicode => !before && after,
            Self::WordEndAscii | Self::WordEndUnicode => before && !after,
            Self::WordStartHalfAscii | Self::WordStartHalfUnicode => !before,
            Self::WordEndHalfAscii | Self::WordEndHalfUnicode => !after,
            Self::Start | Self::End | Self::StartLine | Self::EndLine => false,
        }
    }

    pub const ALL: [Self; 16] = [
        Self::Start,
        Self::End,
        Self::StartLine,
        Self::EndLine,
        Self::WordAscii,
        Self::WordAsciiNegate,
        Self::WordUnicode,
        Self::WordUnicodeNegate,
        Self::WordStartAscii,
        Self::WordEndAscii,
        Self::WordStartUnicode,
        Self::WordEndUnicode,
        Self::WordStartHalfAscii,
        Self::WordEndHalfAscii,
        Self::WordStartHalfUnicode,
        Self::WordEndHalfUnicode,
    ];
}

pub const WORD_LOOKS: u32 = !(Look::Start.bit()
    | Look::End.bit()
    | Look::StartLine.bit()
    | Look::EndLine.bit())
    & 0xFFFF;

pub const UNICODE_WORD_LOOKS: u32 = Look::WordUnicode.bit()
    | Look::WordUnicodeNegate.bit()
    | Look::WordStartUnicode.bit()
    | Look::WordEndUnicode.bit()
    | Look::WordStartHalfUnicode.bit()
    | Look::WordEndHalfUnicode.bit();

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Class {
    pub ranges: Vec<(u32, u32)>,
    pub unicode: bool,
}

impl Class {
    pub const fn empty(unicode: bool) -> Self {
        Self {
            ranges: Vec::new(),
            unicode,
        }
    }

    pub fn new(unicode: bool, ranges: impl IntoIterator<Item = (u32, u32)>) -> Self {
        let mut class = Self {
            ranges: ranges.into_iter().collect(),
            unicode,
        };
        class.canonicalize();
        class
    }

    pub const fn max(&self) -> u32 {
        if self.unicode {
            MAX_CHAR
        } else {
            0xFF
        }
    }

    pub fn canonicalize(&mut self) {
        self.ranges.sort_unstable();
        let mut merged: Vec<(u32, u32)> = Vec::with_capacity(self.ranges.len());
        for &(lo, hi) in &self.ranges {
            let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
            match merged.last_mut() {
                Some(last) if lo <= last.1.saturating_add(1) => last.1 = last.1.max(hi),
                _ => merged.push((lo, hi)),
            }
        }
        self.ranges = merged;
    }

    pub fn push(&mut self, lo: u32, hi: u32) {
        self.ranges.push((lo, hi));
        self.canonicalize();
    }

    pub fn union(&mut self, other: &Self) {
        self.ranges.extend_from_slice(&other.ranges);
        self.canonicalize();
    }

    pub fn negate(&mut self) {
        let max = self.max();
        let mut out = Vec::with_capacity(self.ranges.len() + 1);
        let mut next = 0u32;
        for &(lo, hi) in &self.ranges {
            if lo > next {
                out.push((next, lo - 1));
            }
            next = hi.saturating_add(1);
        }
        if next <= max {
            out.push((next, max));
        }
        if self.unicode {
            let mut cleaned = Self {
                ranges: out,
                unicode: true,
            };
            cleaned.difference(&Self {
                ranges: vec![(0xD800, 0xDFFF)],
                unicode: true,
            });
            self.ranges = cleaned.ranges;
        } else {
            self.ranges = out;
        }
    }

    pub fn intersect(&mut self, other: &Self) {
        let mut out = Vec::new();
        let (mut i, mut j) = (0, 0);
        while i < self.ranges.len() && j < other.ranges.len() {
            let (a_lo, a_hi) = self.ranges[i];
            let (b_lo, b_hi) = other.ranges[j];
            let lo = a_lo.max(b_lo);
            let hi = a_hi.min(b_hi);
            if lo <= hi {
                out.push((lo, hi));
            }
            if a_hi < b_hi {
                i += 1;
            } else {
                j += 1;
            }
        }
        self.ranges = out;
    }

    pub fn difference(&mut self, other: &Self) {
        let mut negated = other.clone();
        negated.unicode = self.unicode;
        let max = self.max();
        let mut complement = Vec::new();
        let mut next = 0u32;
        for &(lo, hi) in &negated.ranges {
            if lo > next {
                complement.push((next, lo - 1));
            }
            next = hi.saturating_add(1);
        }
        if next <= max {
            complement.push((next, max));
        }
        negated.ranges = complement;
        self.intersect(&negated);
    }

    pub fn symmetric_difference(&mut self, other: &Self) {
        let mut both = self.clone();
        both.intersect(other);
        self.union(other);
        self.difference(&both);
    }

    pub fn case_fold(&mut self) {
        let mut extra = Vec::new();
        if self.unicode {
            for &(lo, hi) in &self.ranges {
                unicode::fold_into(lo, hi.min(MAX_CHAR), &mut extra);
            }
        } else {
            for &(lo, hi) in &self.ranges {
                for (start, end, delta) in [(b'a', b'z', -32i32), (b'A', b'Z', 32)] {
                    let lo2 = lo.max(u32::from(start));
                    let hi2 = hi.min(u32::from(end));
                    if lo2 <= hi2 {
                        extra.push((
                            lo2.wrapping_add_signed(delta),
                            hi2.wrapping_add_signed(delta),
                        ));
                    }
                }
            }
        }
        if !extra.is_empty() {
            self.ranges.extend(extra);
            self.canonicalize();
        }
    }

    pub fn is_ascii(&self) -> bool {
        self.ranges.last().is_none_or(|&(_, hi)| hi <= 0x7F)
    }

    pub fn single_value(&self) -> Option<u32> {
        match self.ranges.as_slice() {
            [(lo, hi)] if lo == hi => Some(*lo),
            _ => None,
        }
    }

    pub fn count(&self) -> u64 {
        self.ranges
            .iter()
            .map(|&(lo, hi)| u64::from(hi - lo) + 1)
            .sum()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hir {
    Empty,
    Literal(Vec<u8>),
    Class(Class),
    Look(Look),
    Repeat {
        min: u32,
        max: Option<u32>,
        greedy: bool,
        sub: Box<Self>,
    },
    Capture {
        index: u32,
        sub: Box<Self>,
    },
    Concat(Vec<Self>),
    Alternation(Vec<Self>),
}

impl Hir {
    pub fn class(class: Class) -> Self {
        if !class.unicode {
            if let Some(byte) = class.single_value() {
                return Self::Literal(vec![u8::try_from(byte).unwrap_or(0)]);
            }
        } else if let Some(value) = class.single_value() {
            if let Some(c) = char::from_u32(value) {
                let mut buffer = [0u8; 4];
                return Self::Literal(c.encode_utf8(&mut buffer).as_bytes().to_vec());
            }
        }
        Self::Class(class)
    }

    pub fn repeat(sub: Self, min: u32, max: Option<u32>, greedy: bool) -> Self {
        if min == 1 && max == Some(1) {
            return sub;
        }
        if max == Some(0) {
            return Self::Empty;
        }
        Self::Repeat {
            min,
            max,
            greedy,
            sub: Box::new(sub),
        }
    }

    pub fn concat(items: Vec<Self>) -> Self {
        let mut out: Vec<Self> = Vec::with_capacity(items.len());
        for item in items {
            match item {
                Self::Empty => {}
                Self::Concat(inner) => {
                    for sub in inner {
                        push_concat(&mut out, sub);
                    }
                }
                other => push_concat(&mut out, other),
            }
        }
        match out.len() {
            0 => Self::Empty,
            1 => out.pop().unwrap_or(Self::Empty),
            _ => Self::Concat(out),
        }
    }

    pub fn alternation(items: Vec<Self>) -> Self {
        let mut out: Vec<Self> = Vec::with_capacity(items.len());
        for item in items {
            match item {
                Self::Alternation(inner) => out.extend(inner),
                other => out.push(other),
            }
        }
        if out.len() == 1 {
            return out.pop().unwrap_or(Self::Empty);
        }
        Self::Alternation(out)
    }

    pub fn min_len(&self) -> usize {
        match self {
            Self::Empty | Self::Look(_) => 0,
            Self::Literal(bytes) => bytes.len(),
            Self::Class(class) => usize::from(!class.ranges.is_empty()),
            Self::Repeat { min, sub, .. } => sub.min_len().saturating_mul(*min as usize),
            Self::Capture { sub, .. } => sub.min_len(),
            Self::Concat(items) => items
                .iter()
                .fold(0usize, |total, item| total.saturating_add(item.min_len())),
            Self::Alternation(items) => items.iter().map(Self::min_len).min().unwrap_or(0),
        }
    }

    pub fn looks(&self) -> u32 {
        match self {
            Self::Empty | Self::Literal(_) | Self::Class(_) => 0,
            Self::Look(look) => look.bit(),
            Self::Repeat { sub, .. } | Self::Capture { sub, .. } => sub.looks(),
            Self::Concat(items) | Self::Alternation(items) => {
                items.iter().fold(0, |bits, item| bits | item.looks())
            }
        }
    }

    pub fn capture_indices(&self, out: &mut Vec<u32>) {
        match self {
            Self::Empty | Self::Literal(_) | Self::Class(_) | Self::Look(_) => {}
            Self::Capture { index, sub } => {
                out.push(*index);
                sub.capture_indices(out);
            }
            Self::Repeat { sub, .. } => sub.capture_indices(out),
            Self::Concat(items) | Self::Alternation(items) => {
                for item in items {
                    item.capture_indices(out);
                }
            }
        }
    }

    pub fn is_start_anchored(&self) -> bool {
        match self {
            Self::Look(Look::Start) => true,
            Self::Repeat { min, sub, .. } => *min > 0 && sub.is_start_anchored(),
            Self::Capture { sub, .. } => sub.is_start_anchored(),
            Self::Concat(items) => items.first().is_some_and(Self::is_start_anchored),
            Self::Alternation(items) => items.iter().all(Self::is_start_anchored),
            _ => false,
        }
    }
}

fn push_concat(out: &mut Vec<Hir>, item: Hir) {
    if let Hir::Literal(bytes) = &item {
        if let Some(Hir::Literal(last)) = out.last_mut() {
            last.extend_from_slice(bytes);
            return;
        }
    }
    out.push(item);
}
