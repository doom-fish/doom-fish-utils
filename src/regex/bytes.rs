use super::meta::{GroupInfo, Meta, Pool, Settings};
use super::pikevm::NONE;
use super::{expand_bytes, Error, Stepper};
use std::borrow::Cow;
use std::fmt;
use std::iter::FusedIterator;
use std::ops::{Index, Range};
use std::str::FromStr;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct RegexBuilder {
    pattern: String,
    settings: Settings,
}

impl RegexBuilder {
    #[must_use]
    pub fn new(pattern: &str) -> Self {
        Self {
            pattern: pattern.to_owned(),
            settings: Settings::new(),
        }
    }

    pub fn build(&self) -> Result<Regex, Error> {
        let meta = Arc::new(Meta::new(&self.pattern, &self.settings, false)?);
        Ok(Regex {
            pool: Box::new(Pool::new(meta)),
        })
    }

    super::builder_options!();
}

pub struct Regex {
    pool: Box<Pool>,
}

impl Clone for Regex {
    fn clone(&self) -> Self {
        Self {
            pool: Box::new(Pool::new(Arc::clone(self.pool.meta_arc()))),
        }
    }
}

impl fmt::Debug for Regex {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("Regex").field(&self.as_str()).finish()
    }
}

impl fmt::Display for Regex {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Regex {
    type Err = Error;

    fn from_str(pattern: &str) -> Result<Self, Error> {
        Self::new(pattern)
    }
}

impl TryFrom<&str> for Regex {
    type Error = Error;

    fn try_from(pattern: &str) -> Result<Self, Error> {
        Self::new(pattern)
    }
}

impl TryFrom<String> for Regex {
    type Error = Error;

    fn try_from(pattern: String) -> Result<Self, Error> {
        Self::new(&pattern)
    }
}

impl Regex {
    pub fn new(pattern: &str) -> Result<Self, Error> {
        RegexBuilder::new(pattern).build()
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.pool.meta().pattern
    }

    #[must_use]
    pub fn is_match(&self, haystack: &[u8]) -> bool {
        self.is_match_at(haystack, 0)
    }

    #[must_use]
    pub fn is_match_at(&self, haystack: &[u8], start: usize) -> bool {
        let mut guard = self.pool.get();
        self.pool.meta().is_match(guard.cache(), haystack, start)
    }

    #[must_use]
    pub fn find<'h>(&self, haystack: &'h [u8]) -> Option<Match<'h>> {
        self.find_at(haystack, 0)
    }

    #[must_use]
    pub fn find_at<'h>(&self, haystack: &'h [u8], start: usize) -> Option<Match<'h>> {
        let mut guard = self.pool.get();
        self.pool
            .meta()
            .find(guard.cache(), haystack, start)
            .map(|(start, end)| Match::new(haystack, start, end))
    }

    #[must_use]
    pub fn shortest_match(&self, haystack: &[u8]) -> Option<usize> {
        self.shortest_match_at(haystack, 0)
    }

    #[must_use]
    pub fn shortest_match_at(&self, haystack: &[u8], start: usize) -> Option<usize> {
        let mut guard = self.pool.get();
        self.pool.meta().shortest(guard.cache(), haystack, start)
    }

    #[must_use]
    pub fn find_iter<'r, 'h>(&'r self, haystack: &'h [u8]) -> Matches<'r, 'h> {
        Matches {
            haystack,
            stepper: Stepper::new(&self.pool),
        }
    }

    #[must_use]
    pub fn captures<'h>(&self, haystack: &'h [u8]) -> Option<Captures<'h>> {
        self.captures_at(haystack, 0)
    }

    #[must_use]
    pub fn captures_at<'h>(&self, haystack: &'h [u8], start: usize) -> Option<Captures<'h>> {
        let meta = self.pool.meta();
        let mut slots = vec![NONE; meta.group_len() * 2];
        let mut guard = self.pool.get();
        meta.captures(guard.cache(), haystack, start, &mut slots)
            .then(|| Captures {
                haystack,
                slots,
                groups: Arc::clone(&meta.groups),
            })
    }

    #[must_use]
    pub fn captures_iter<'r, 'h>(&'r self, haystack: &'h [u8]) -> CaptureMatches<'r, 'h> {
        CaptureMatches {
            haystack,
            stepper: Stepper::new(&self.pool),
        }
    }

    #[must_use]
    pub fn split<'r, 'h>(&'r self, haystack: &'h [u8]) -> Split<'r, 'h> {
        Split {
            matches: self.find_iter(haystack),
            last: 0,
        }
    }

    #[must_use]
    pub fn splitn<'r, 'h>(&'r self, haystack: &'h [u8], limit: usize) -> SplitN<'r, 'h> {
        SplitN {
            split: self.split(haystack),
            limit,
        }
    }

    pub fn replace<'h, R: Replacer>(&self, haystack: &'h [u8], replacer: R) -> Cow<'h, [u8]> {
        self.replacen(haystack, 1, replacer)
    }

    pub fn replace_all<'h, R: Replacer>(&self, haystack: &'h [u8], replacer: R) -> Cow<'h, [u8]> {
        self.replacen(haystack, 0, replacer)
    }

    pub fn replacen<'h, R: Replacer>(
        &self,
        haystack: &'h [u8],
        limit: usize,
        mut replacer: R,
    ) -> Cow<'h, [u8]> {
        if let Some(replacement) = replacer.no_expansion() {
            let mut matches = self.find_iter(haystack).enumerate().peekable();
            if matches.peek().is_none() {
                return Cow::Borrowed(haystack);
            }
            let mut out = Vec::with_capacity(haystack.len());
            let mut last = 0;
            for (count, found) in matches {
                out.extend_from_slice(&haystack[last..found.start()]);
                out.extend_from_slice(&replacement);
                last = found.end();
                if limit > 0 && count + 1 >= limit {
                    break;
                }
            }
            out.extend_from_slice(&haystack[last..]);
            return Cow::Owned(out);
        }
        let mut captures = self.captures_iter(haystack).enumerate().peekable();
        if captures.peek().is_none() {
            return Cow::Borrowed(haystack);
        }
        let mut out = Vec::with_capacity(haystack.len());
        let mut last = 0;
        for (count, caps) in captures {
            let found = caps.get_match();
            out.extend_from_slice(&haystack[last..found.start()]);
            replacer.replace_append(&caps, &mut out);
            last = found.end();
            if limit > 0 && count + 1 >= limit {
                break;
            }
        }
        out.extend_from_slice(&haystack[last..]);
        Cow::Owned(out)
    }

    #[must_use]
    pub fn captures_len(&self) -> usize {
        self.pool.meta().group_len()
    }

    #[must_use]
    pub fn capture_names(&self) -> super::CaptureNames<'_> {
        super::CaptureNames(self.pool.meta().groups.names.iter())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Match<'h> {
    haystack: &'h [u8],
    start: usize,
    end: usize,
}

impl<'h> Match<'h> {
    const fn new(haystack: &'h [u8], start: usize, end: usize) -> Self {
        Self {
            haystack,
            start,
            end,
        }
    }

    #[must_use]
    pub const fn start(&self) -> usize {
        self.start
    }

    #[must_use]
    pub const fn end(&self) -> usize {
        self.end
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.start == self.end
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.end - self.start
    }

    #[must_use]
    pub const fn range(&self) -> Range<usize> {
        self.start..self.end
    }

    #[must_use]
    pub fn as_bytes(&self) -> &'h [u8] {
        &self.haystack[self.start..self.end]
    }
}

impl fmt::Debug for Match<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = self.as_bytes();
        let mut debug = formatter.debug_struct("Match");
        debug.field("start", &self.start).field("end", &self.end);
        match core::str::from_utf8(bytes) {
            Ok(text) => debug.field("bytes", &text),
            Err(_) => debug.field("bytes", &bytes),
        };
        debug.finish()
    }
}

impl<'h> From<Match<'h>> for &'h [u8] {
    fn from(found: Match<'h>) -> &'h [u8] {
        found.as_bytes()
    }
}

impl From<Match<'_>> for Range<usize> {
    fn from(found: Match<'_>) -> Self {
        found.range()
    }
}

pub struct Captures<'h> {
    haystack: &'h [u8],
    slots: Vec<usize>,
    groups: Arc<GroupInfo>,
}

impl<'h> Captures<'h> {
    #[must_use]
    pub fn get(&self, index: usize) -> Option<Match<'h>> {
        let start = *self.slots.get(index * 2)?;
        let end = *self.slots.get(index * 2 + 1)?;
        (start != NONE && end != NONE).then(|| Match::new(self.haystack, start, end))
    }

    #[must_use]
    pub fn get_match(&self) -> Match<'h> {
        Match::new(self.haystack, self.slots[0], self.slots[1])
    }

    #[must_use]
    pub fn name(&self, name: &str) -> Option<Match<'h>> {
        self.get(self.groups.index(name)?)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.len() / 2
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    #[must_use]
    pub const fn iter<'c>(&'c self) -> SubCaptureMatches<'c, 'h> {
        SubCaptureMatches {
            captures: self,
            index: 0,
        }
    }

    pub fn expand(&self, replacement: &[u8], destination: &mut Vec<u8>) {
        expand_bytes(
            replacement,
            destination,
            |index| self.get(index).map(|found| found.as_bytes()),
            |name| self.name(name).map(|found| found.as_bytes()),
        );
    }
}

impl fmt::Debug for Captures<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut map = formatter.debug_map();
        for index in 0..self.len() {
            let key = self.groups.names[index]
                .as_ref()
                .map_or_else(|| index.to_string(), |name| format!("{index}/{name}"));
            map.entry(&key, &self.get(index));
        }
        map.finish()
    }
}

impl Index<usize> for Captures<'_> {
    type Output = [u8];

    fn index(&self, index: usize) -> &[u8] {
        self.get(index).map_or_else(
            || panic!("no group at index '{index}'"),
            |found| found.as_bytes(),
        )
    }
}

impl<'n> Index<&'n str> for Captures<'_> {
    type Output = [u8];

    fn index(&self, name: &'n str) -> &[u8] {
        self.name(name).map_or_else(
            || panic!("no group named '{name}'"),
            |found| found.as_bytes(),
        )
    }
}

impl<'c, 'h> IntoIterator for &'c Captures<'h> {
    type Item = Option<Match<'h>>;
    type IntoIter = SubCaptureMatches<'c, 'h>;

    fn into_iter(self) -> SubCaptureMatches<'c, 'h> {
        self.iter()
    }
}

pub struct SubCaptureMatches<'c, 'h> {
    captures: &'c Captures<'h>,
    index: usize,
}

impl<'h> Iterator for SubCaptureMatches<'_, 'h> {
    type Item = Option<Match<'h>>;

    fn next(&mut self) -> Option<Option<Match<'h>>> {
        if self.index >= self.captures.len() {
            return None;
        }
        let found = self.captures.get(self.index);
        self.index += 1;
        Some(found)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.captures.len() - self.index;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for SubCaptureMatches<'_, '_> {}

impl FusedIterator for SubCaptureMatches<'_, '_> {}

pub struct Matches<'r, 'h> {
    haystack: &'h [u8],
    stepper: Stepper<'r>,
}

impl<'h> Iterator for Matches<'_, 'h> {
    type Item = Match<'h>;

    fn next(&mut self) -> Option<Match<'h>> {
        let (start, end) = self.stepper.advance(self.haystack, None)?;
        Some(Match::new(self.haystack, start, end))
    }
}

impl FusedIterator for Matches<'_, '_> {}

impl fmt::Debug for Matches<'_, '_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Matches").finish_non_exhaustive()
    }
}

pub struct CaptureMatches<'r, 'h> {
    haystack: &'h [u8],
    stepper: Stepper<'r>,
}

impl<'h> Iterator for CaptureMatches<'_, 'h> {
    type Item = Captures<'h>;

    fn next(&mut self) -> Option<Captures<'h>> {
        let groups = Arc::clone(&self.stepper.pool.meta().groups);
        let mut slots = vec![NONE; groups.names.len() * 2];
        self.stepper.advance(self.haystack, Some(&mut slots))?;
        Some(Captures {
            haystack: self.haystack,
            slots,
            groups,
        })
    }
}

impl FusedIterator for CaptureMatches<'_, '_> {}

impl fmt::Debug for CaptureMatches<'_, '_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("CaptureMatches").finish_non_exhaustive()
    }
}

pub struct Split<'r, 'h> {
    matches: Matches<'r, 'h>,
    last: usize,
}

impl<'h> Iterator for Split<'_, 'h> {
    type Item = &'h [u8];

    fn next(&mut self) -> Option<&'h [u8]> {
        let haystack = self.matches.haystack;
        match self.matches.next() {
            None => {
                if self.last > haystack.len() {
                    None
                } else {
                    let piece = &haystack[self.last..];
                    self.last = haystack.len() + 1;
                    Some(piece)
                }
            }
            Some(found) => {
                let piece = &haystack[self.last..found.start()];
                self.last = found.end();
                Some(piece)
            }
        }
    }
}

impl FusedIterator for Split<'_, '_> {}

pub struct SplitN<'r, 'h> {
    split: Split<'r, 'h>,
    limit: usize,
}

impl<'h> Iterator for SplitN<'_, 'h> {
    type Item = &'h [u8];

    fn next(&mut self) -> Option<&'h [u8]> {
        if self.limit == 0 {
            return None;
        }
        self.limit -= 1;
        if self.limit > 0 {
            return self.split.next();
        }
        let haystack = self.split.matches.haystack;
        if self.split.last > haystack.len() {
            None
        } else {
            let piece = &haystack[self.split.last..];
            self.split.last = haystack.len() + 1;
            Some(piece)
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.limit))
    }
}

impl FusedIterator for SplitN<'_, '_> {}

pub trait Replacer {
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut Vec<u8>);

    fn no_expansion(&mut self) -> Option<Cow<'_, [u8]>> {
        None
    }

    fn by_ref(&mut self) -> ReplacerRef<'_, Self> {
        ReplacerRef(self)
    }
}

#[derive(Debug)]
pub struct ReplacerRef<'a, R: ?Sized>(&'a mut R);

impl<R: Replacer + ?Sized> Replacer for ReplacerRef<'_, R> {
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut Vec<u8>) {
        self.0.replace_append(caps, dst);
    }

    fn no_expansion(&mut self) -> Option<Cow<'_, [u8]>> {
        self.0.no_expansion()
    }
}

#[derive(Clone, Debug)]
pub struct NoExpand<'s>(pub &'s [u8]);

impl Replacer for NoExpand<'_> {
    fn replace_append(&mut self, _: &Captures<'_>, dst: &mut Vec<u8>) {
        dst.extend_from_slice(self.0);
    }

    fn no_expansion(&mut self) -> Option<Cow<'_, [u8]>> {
        Some(Cow::Borrowed(self.0))
    }
}

fn literal_replacement(replacement: &[u8]) -> Option<Cow<'_, [u8]>> {
    (!replacement.contains(&b'$')).then_some(Cow::Borrowed(replacement))
}

impl<const N: usize> Replacer for &[u8; N] {
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut Vec<u8>) {
        caps.expand(&self[..], dst);
    }

    fn no_expansion(&mut self) -> Option<Cow<'_, [u8]>> {
        literal_replacement(&self[..])
    }
}

impl<const N: usize> Replacer for [u8; N] {
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut Vec<u8>) {
        caps.expand(&self[..], dst);
    }

    fn no_expansion(&mut self) -> Option<Cow<'_, [u8]>> {
        literal_replacement(&self[..])
    }
}

impl Replacer for &[u8] {
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut Vec<u8>) {
        caps.expand(self, dst);
    }

    fn no_expansion(&mut self) -> Option<Cow<'_, [u8]>> {
        literal_replacement(self)
    }
}

impl Replacer for &Vec<u8> {
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut Vec<u8>) {
        caps.expand(self, dst);
    }

    fn no_expansion(&mut self) -> Option<Cow<'_, [u8]>> {
        literal_replacement(self)
    }
}

impl Replacer for Vec<u8> {
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut Vec<u8>) {
        caps.expand(self, dst);
    }

    fn no_expansion(&mut self) -> Option<Cow<'_, [u8]>> {
        literal_replacement(self)
    }
}

impl Replacer for Cow<'_, [u8]> {
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut Vec<u8>) {
        caps.expand(&self[..], dst);
    }

    fn no_expansion(&mut self) -> Option<Cow<'_, [u8]>> {
        literal_replacement(&self[..])
    }
}

impl Replacer for &Cow<'_, [u8]> {
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut Vec<u8>) {
        caps.expand(&self[..], dst);
    }

    fn no_expansion(&mut self) -> Option<Cow<'_, [u8]>> {
        literal_replacement(&self[..])
    }
}

impl<F, T> Replacer for F
where
    F: FnMut(&Captures<'_>) -> T,
    T: AsRef<[u8]>,
{
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut Vec<u8>) {
        dst.extend_from_slice((*self)(caps).as_ref());
    }
}
