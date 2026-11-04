use super::twoway::TwoWay;
use super::{find_pair, memchr, memchr2, memrchr, memrchr2, rfind_pair, Pair, PAIR_VECTOR, RANK};
use std::borrow::Cow;
use std::iter::FusedIterator;

#[derive(Clone, Debug)]
struct Searcher {
    pair: Option<Pair>,
    forward: Option<TwoWay>,
    backward: Option<TwoWay>,
    fold: bool,
}

const fn fold_mask(byte: u8, fold: bool) -> u8 {
    if fold && byte.is_ascii_alphabetic() {
        0x20
    } else {
        0
    }
}

fn rank(byte: u8, fold: bool) -> u8 {
    if fold && byte.is_ascii_alphabetic() {
        RANK[usize::from(byte.to_ascii_lowercase())].max(RANK[usize::from(byte.to_ascii_uppercase())])
    } else {
        RANK[usize::from(byte)]
    }
}

fn choose_pair(needle: &[u8], fold: bool) -> Pair {
    let mut index1 = 0;
    for (index, &byte) in needle.iter().enumerate() {
        if rank(byte, fold) < rank(needle[index1], fold) {
            index1 = index;
        }
    }
    let mut index2 = usize::from(index1 == 0);
    for (index, &byte) in needle.iter().enumerate() {
        if index != index1 && rank(byte, fold) < rank(needle[index2], fold) {
            index2 = index;
        }
    }
    let fold1 = fold_mask(needle[index1], fold);
    let fold2 = fold_mask(needle[index2], fold);
    Pair {
        index1,
        index2,
        byte1: needle[index1] | fold1,
        byte2: needle[index2] | fold2,
        fold1,
        fold2,
    }
}

fn mismatch(needle: &[u8], window: &[u8]) -> usize {
    let mut at = 0;
    while at + 8 <= needle.len() {
        let mut left = [0u8; 8];
        let mut right = [0u8; 8];
        left.copy_from_slice(&needle[at..at + 8]);
        right.copy_from_slice(&window[at..at + 8]);
        let difference = u64::from_le_bytes(left) ^ u64::from_le_bytes(right);
        if difference != 0 {
            return at + (difference.trailing_zeros() / 8) as usize;
        }
        at += 8;
    }
    while at < needle.len() && needle[at] == window[at] {
        at += 1;
    }
    at
}

fn mismatch_folded(needle: &[u8], window: &[u8]) -> usize {
    needle
        .iter()
        .zip(window)
        .position(|(&left, &right)| left != right.to_ascii_lowercase())
        .unwrap_or(needle.len())
}

impl Searcher {
    fn new(needle: &[u8], fold: bool, forward: bool, backward: bool) -> Self {
        Self {
            pair: (needle.len() >= 2).then(|| choose_pair(needle, fold)),
            forward: forward.then(|| TwoWay::forward(needle)),
            backward: backward.then(|| TwoWay::backward(needle)),
            fold,
        }
    }

    fn compared(&self, needle: &[u8], haystack: &[u8], at: usize) -> usize {
        let window = &haystack[at..at + needle.len()];
        if self.fold {
            mismatch_folded(needle, window)
        } else {
            mismatch(needle, window)
        }
    }

    fn find(&self, needle: &[u8], haystack: &[u8]) -> Option<usize> {
        let length = needle.len();
        if length == 0 {
            return Some(0);
        }
        if haystack.len() < length {
            return None;
        }
        if length == 1 {
            let byte = needle[0];
            return if self.fold && byte.is_ascii_alphabetic() {
                memchr2(byte.to_ascii_lowercase(), byte.to_ascii_uppercase(), haystack)
            } else {
                memchr(byte, haystack)
            };
        }
        let two_way = self.forward.as_ref()?;
        let last = haystack.len() - length;
        if let (Some(pair), Some(vector)) = (self.pair.as_ref(), PAIR_VECTOR) {
            if haystack.len() >= pair.reach() + vector && haystack.len() >= 64 {
                let mut seen = 0usize;
                let mut work = 0usize;
                let result = find_pair(haystack, 0, last, pair, |candidate| {
                    seen += 1;
                    if seen > 32 && (seen * 16 > candidate + 64 || work > 8 * candidate + 4096) {
                        return None;
                    }
                    let compared = self.compared(needle, haystack, candidate);
                    work += compared + 1;
                    Some(compared == length)
                });
                return match result {
                    Ok(found) => found,
                    Err(resume) => two_way.find(needle, haystack, resume, self.fold),
                };
            }
        }
        two_way.find(needle, haystack, 0, self.fold)
    }

    fn rfind(&self, needle: &[u8], haystack: &[u8]) -> Option<usize> {
        let length = needle.len();
        if length == 0 {
            return Some(haystack.len());
        }
        if haystack.len() < length {
            return None;
        }
        if length == 1 {
            let byte = needle[0];
            return if self.fold && byte.is_ascii_alphabetic() {
                memrchr2(byte.to_ascii_lowercase(), byte.to_ascii_uppercase(), haystack)
            } else {
                memrchr(byte, haystack)
            };
        }
        let two_way = self.backward.as_ref()?;
        let last = haystack.len() - length;
        if let (Some(pair), Some(vector)) = (self.pair.as_ref(), PAIR_VECTOR) {
            if haystack.len() >= pair.reach() + vector && haystack.len() >= 64 {
                let mut seen = 0usize;
                let mut work = 0usize;
                let result = rfind_pair(haystack, 0, last, pair, |candidate| {
                    seen += 1;
                    let distance = last - candidate;
                    if seen > 32 && (seen * 16 > distance + 64 || work > 8 * distance + 4096) {
                        return None;
                    }
                    let compared = self.compared(needle, haystack, candidate);
                    work += compared + 1;
                    Some(compared == length)
                });
                return match result {
                    Ok(found) => found,
                    Err(resume) => two_way.rfind(needle, haystack, resume + length, self.fold),
                };
            }
        }
        two_way.rfind(needle, haystack, haystack.len(), self.fold)
    }
}

#[inline]
#[must_use]
pub fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.len() <= 1 {
        return match needle.first() {
            None => Some(0),
            Some(&byte) => memchr(byte, haystack),
        };
    }
    Finder::new(needle).find(haystack)
}

#[inline]
#[must_use]
pub fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.len() <= 1 {
        return match needle.first() {
            None => Some(haystack.len()),
            Some(&byte) => memrchr(byte, haystack),
        };
    }
    FinderRev::new(needle).rfind(haystack)
}

#[inline]
pub fn find_iter<'h, 'n, N: 'n + ?Sized + AsRef<[u8]>>(
    haystack: &'h [u8],
    needle: &'n N,
) -> FindIter<'h, 'n> {
    FindIter::new(haystack, Finder::new(needle))
}

#[inline]
pub fn rfind_iter<'h, 'n, N: 'n + ?Sized + AsRef<[u8]>>(
    haystack: &'h [u8],
    needle: &'n N,
) -> FindRevIter<'h, 'n> {
    FindRevIter::new(haystack, FinderRev::new(needle))
}

#[derive(Clone, Debug)]
pub struct Finder<'n> {
    needle: Cow<'n, [u8]>,
    searcher: Searcher,
}

impl<'n> Finder<'n> {
    #[must_use]
    pub fn new<B: ?Sized + AsRef<[u8]>>(needle: &'n B) -> Self {
        let needle = needle.as_ref();
        Self {
            searcher: Searcher::new(needle, false, true, false),
            needle: Cow::Borrowed(needle),
        }
    }

    #[inline]
    #[must_use]
    pub fn find(&self, haystack: &[u8]) -> Option<usize> {
        self.searcher.find(&self.needle, haystack)
    }

    #[inline]
    #[must_use]
    pub fn find_iter<'a, 'h>(&'a self, haystack: &'h [u8]) -> FindIter<'h, 'a> {
        FindIter::new(haystack, self.as_ref())
    }

    #[must_use]
    pub fn into_owned(self) -> Finder<'static> {
        Finder {
            needle: Cow::Owned(self.needle.into_owned()),
            searcher: self.searcher,
        }
    }

    #[must_use]
    pub fn as_ref(&self) -> Finder<'_> {
        Finder {
            needle: Cow::Borrowed(&self.needle),
            searcher: self.searcher.clone(),
        }
    }

    #[must_use]
    pub fn needle(&self) -> &[u8] {
        &self.needle
    }
}

#[derive(Clone, Debug)]
pub struct FinderRev<'n> {
    needle: Cow<'n, [u8]>,
    searcher: Searcher,
}

impl<'n> FinderRev<'n> {
    #[must_use]
    pub fn new<B: ?Sized + AsRef<[u8]>>(needle: &'n B) -> Self {
        let needle = needle.as_ref();
        Self {
            searcher: Searcher::new(needle, false, false, true),
            needle: Cow::Borrowed(needle),
        }
    }

    #[inline]
    #[must_use]
    pub fn rfind(&self, haystack: &[u8]) -> Option<usize> {
        self.searcher.rfind(&self.needle, haystack)
    }

    #[inline]
    #[must_use]
    pub fn rfind_iter<'a, 'h>(&'a self, haystack: &'h [u8]) -> FindRevIter<'h, 'a> {
        FindRevIter::new(haystack, self.as_ref())
    }

    #[must_use]
    pub fn into_owned(self) -> FinderRev<'static> {
        FinderRev {
            needle: Cow::Owned(self.needle.into_owned()),
            searcher: self.searcher,
        }
    }

    #[must_use]
    pub fn as_ref(&self) -> FinderRev<'_> {
        FinderRev {
            needle: Cow::Borrowed(&self.needle),
            searcher: self.searcher.clone(),
        }
    }

    #[must_use]
    pub fn needle(&self) -> &[u8] {
        &self.needle
    }
}

#[derive(Clone, Debug)]
pub struct CaseInsensitiveFinder<'n> {
    needle: Cow<'n, [u8]>,
    folded: Option<Box<[u8]>>,
    searcher: Searcher,
}

impl<'n> CaseInsensitiveFinder<'n> {
    #[must_use]
    pub fn new<B: ?Sized + AsRef<[u8]>>(needle: &'n B) -> Self {
        let needle = needle.as_ref();
        let folded = needle
            .iter()
            .any(u8::is_ascii_uppercase)
            .then(|| needle.to_ascii_lowercase().into_boxed_slice());
        let searcher = Searcher::new(folded.as_deref().unwrap_or(needle), true, true, true);
        Self {
            needle: Cow::Borrowed(needle),
            folded,
            searcher,
        }
    }

    fn folded(&self) -> &[u8] {
        self.folded.as_deref().unwrap_or(&self.needle)
    }

    #[inline]
    #[must_use]
    pub fn find(&self, haystack: &[u8]) -> Option<usize> {
        self.searcher.find(self.folded(), haystack)
    }

    #[inline]
    #[must_use]
    pub fn rfind(&self, haystack: &[u8]) -> Option<usize> {
        self.searcher.rfind(self.folded(), haystack)
    }

    #[inline]
    #[must_use]
    pub const fn find_iter<'a, 'h>(&'a self, haystack: &'h [u8]) -> CaseInsensitiveFindIter<'h, 'a> {
        CaseInsensitiveFindIter {
            haystack,
            finder: self,
            position: 0,
        }
    }

    #[must_use]
    pub fn into_owned(self) -> CaseInsensitiveFinder<'static> {
        CaseInsensitiveFinder {
            needle: Cow::Owned(self.needle.into_owned()),
            folded: self.folded,
            searcher: self.searcher,
        }
    }

    #[must_use]
    pub fn needle(&self) -> &[u8] {
        &self.needle
    }
}

#[derive(Clone, Debug)]
pub struct FindIter<'h, 'n> {
    haystack: &'h [u8],
    finder: Finder<'n>,
    position: usize,
}

impl<'h, 'n> FindIter<'h, 'n> {
    const fn new(haystack: &'h [u8], finder: Finder<'n>) -> Self {
        Self {
            haystack,
            finder,
            position: 0,
        }
    }

    #[must_use]
    pub fn into_owned(self) -> FindIter<'h, 'static> {
        FindIter {
            haystack: self.haystack,
            finder: self.finder.into_owned(),
            position: self.position,
        }
    }
}

impl Iterator for FindIter<'_, '_> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        let window = self.haystack.get(self.position..)?;
        let at = self.position + self.finder.find(window)?;
        self.position = at + self.finder.needle().len().max(1);
        Some(at)
    }
}

impl FusedIterator for FindIter<'_, '_> {}

#[derive(Clone, Debug)]
pub struct FindRevIter<'h, 'n> {
    haystack: &'h [u8],
    finder: FinderRev<'n>,
    position: Option<usize>,
}

impl<'h, 'n> FindRevIter<'h, 'n> {
    const fn new(haystack: &'h [u8], finder: FinderRev<'n>) -> Self {
        Self {
            haystack,
            finder,
            position: Some(haystack.len()),
        }
    }

    #[must_use]
    pub fn into_owned(self) -> FindRevIter<'h, 'static> {
        FindRevIter {
            haystack: self.haystack,
            finder: self.finder.into_owned(),
            position: self.position,
        }
    }
}

impl Iterator for FindRevIter<'_, '_> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        let position = self.position?;
        let Some(at) = self.finder.rfind(&self.haystack[..position]) else {
            self.position = None;
            return None;
        };
        self.position = if at == position {
            position.checked_sub(1)
        } else {
            Some(at)
        };
        Some(at)
    }
}

impl FusedIterator for FindRevIter<'_, '_> {}

#[derive(Clone, Debug)]
pub struct CaseInsensitiveFindIter<'h, 'n> {
    haystack: &'h [u8],
    finder: &'n CaseInsensitiveFinder<'n>,
    position: usize,
}

impl Iterator for CaseInsensitiveFindIter<'_, '_> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        let window = self.haystack.get(self.position..)?;
        let at = self.position + self.finder.find(window)?;
        self.position = at + self.finder.needle().len().max(1);
        Some(at)
    }
}

impl FusedIterator for CaseInsensitiveFindIter<'_, '_> {}
