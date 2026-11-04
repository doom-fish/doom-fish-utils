pub mod bytes;
mod dfa;
mod hir;
mod literal;
mod meta;
mod nfa;
mod parse;
mod pikevm;
mod unicode;
mod utf8;

use meta::{GroupInfo, Meta, Pool, Settings};
use pikevm::NONE;
use std::borrow::Cow;
use std::fmt;
use std::iter::FusedIterator;
use std::ops::{Index, Range};
use std::str::FromStr;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    Syntax(String),
    CompiledTooBig(usize),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax(message) => formatter.write_str(message),
            Self::CompiledTooBig(limit) => {
                write!(formatter, "Compiled regex exceeds size limit of {limit} bytes.")
            }
        }
    }
}

impl std::error::Error for Error {}

#[must_use]
pub fn escape(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    parse::escape_into(pattern, &mut out);
    out
}

#[derive(Clone, Debug)]
pub struct RegexBuilder {
    pattern: String,
    settings: Settings,
}

macro_rules! builder_options {
    () => {
        pub fn case_insensitive(&mut self, yes: bool) -> &mut Self {
            self.settings.flags.case_insensitive = yes;
            self
        }

        pub fn multi_line(&mut self, yes: bool) -> &mut Self {
            self.settings.flags.multi_line = yes;
            self
        }

        pub fn dot_matches_new_line(&mut self, yes: bool) -> &mut Self {
            self.settings.flags.dot_matches_new_line = yes;
            self
        }

        pub fn swap_greed(&mut self, yes: bool) -> &mut Self {
            self.settings.flags.swap_greed = yes;
            self
        }

        pub fn ignore_whitespace(&mut self, yes: bool) -> &mut Self {
            self.settings.flags.ignore_whitespace = yes;
            self
        }

        pub fn unicode(&mut self, yes: bool) -> &mut Self {
            self.settings.flags.unicode = yes;
            self
        }

        pub fn size_limit(&mut self, bytes: usize) -> &mut Self {
            self.settings.size_limit = bytes;
            self
        }

        pub fn dfa_size_limit(&mut self, bytes: usize) -> &mut Self {
            self.settings.dfa_size_limit = bytes;
            self
        }

        pub fn nest_limit(&mut self, limit: u32) -> &mut Self {
            self.settings.nest_limit = limit;
            self
        }
    };
}

pub(crate) use builder_options;

impl RegexBuilder {
    #[must_use]
    pub fn new(pattern: &str) -> Self {
        Self {
            pattern: pattern.to_owned(),
            settings: Settings::new(),
        }
    }

    pub fn build(&self) -> Result<Regex, Error> {
        let meta = Arc::new(Meta::new(&self.pattern, &self.settings, true)?);
        Ok(Regex {
            pool: Box::new(Pool::new(meta)),
        })
    }

    builder_options!();
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
    pub fn is_match(&self, haystack: &str) -> bool {
        self.is_match_at(haystack, 0)
    }

    #[must_use]
    pub fn is_match_at(&self, haystack: &str, start: usize) -> bool {
        let mut guard = self.pool.get();
        self.pool
            .meta()
            .is_match(guard.cache(), haystack.as_bytes(), start)
    }

    #[must_use]
    pub fn find<'h>(&self, haystack: &'h str) -> Option<Match<'h>> {
        self.find_at(haystack, 0)
    }

    #[must_use]
    pub fn find_at<'h>(&self, haystack: &'h str, start: usize) -> Option<Match<'h>> {
        let mut guard = self.pool.get();
        self.pool
            .meta()
            .find(guard.cache(), haystack.as_bytes(), start)
            .map(|(start, end)| Match::new(haystack, start, end))
    }

    #[must_use]
    pub fn shortest_match(&self, haystack: &str) -> Option<usize> {
        self.shortest_match_at(haystack, 0)
    }

    #[must_use]
    pub fn shortest_match_at(&self, haystack: &str, start: usize) -> Option<usize> {
        let mut guard = self.pool.get();
        self.pool
            .meta()
            .shortest(guard.cache(), haystack.as_bytes(), start)
    }

    #[must_use]
    pub fn find_iter<'r, 'h>(&'r self, haystack: &'h str) -> Matches<'r, 'h> {
        Matches {
            haystack,
            stepper: Stepper::new(&self.pool),
        }
    }

    #[must_use]
    pub fn captures<'h>(&self, haystack: &'h str) -> Option<Captures<'h>> {
        self.captures_at(haystack, 0)
    }

    #[must_use]
    pub fn captures_at<'h>(&self, haystack: &'h str, start: usize) -> Option<Captures<'h>> {
        let meta = self.pool.meta();
        let mut slots = vec![NONE; meta.group_len() * 2];
        let mut guard = self.pool.get();
        meta.captures(guard.cache(), haystack.as_bytes(), start, &mut slots)
            .then(|| Captures {
                haystack,
                slots,
                groups: Arc::clone(&meta.groups),
            })
    }

    #[must_use]
    pub fn captures_iter<'r, 'h>(&'r self, haystack: &'h str) -> CaptureMatches<'r, 'h> {
        CaptureMatches {
            haystack,
            stepper: Stepper::new(&self.pool),
        }
    }

    #[must_use]
    pub fn split<'r, 'h>(&'r self, haystack: &'h str) -> Split<'r, 'h> {
        Split {
            matches: self.find_iter(haystack),
            last: 0,
        }
    }

    #[must_use]
    pub fn splitn<'r, 'h>(&'r self, haystack: &'h str, limit: usize) -> SplitN<'r, 'h> {
        SplitN {
            split: self.split(haystack),
            limit,
        }
    }

    pub fn replace<'h, R: Replacer>(&self, haystack: &'h str, replacer: R) -> Cow<'h, str> {
        self.replacen(haystack, 1, replacer)
    }

    pub fn replace_all<'h, R: Replacer>(&self, haystack: &'h str, replacer: R) -> Cow<'h, str> {
        self.replacen(haystack, 0, replacer)
    }

    pub fn replacen<'h, R: Replacer>(
        &self,
        haystack: &'h str,
        limit: usize,
        mut replacer: R,
    ) -> Cow<'h, str> {
        if let Some(replacement) = replacer.no_expansion() {
            let mut matches = self.find_iter(haystack).enumerate().peekable();
            if matches.peek().is_none() {
                return Cow::Borrowed(haystack);
            }
            let mut out = String::with_capacity(haystack.len());
            let mut last = 0;
            for (count, found) in matches {
                out.push_str(&haystack[last..found.start()]);
                out.push_str(&replacement);
                last = found.end();
                if limit > 0 && count + 1 >= limit {
                    break;
                }
            }
            out.push_str(&haystack[last..]);
            return Cow::Owned(out);
        }
        let mut captures = self.captures_iter(haystack).enumerate().peekable();
        if captures.peek().is_none() {
            return Cow::Borrowed(haystack);
        }
        let mut out = String::with_capacity(haystack.len());
        let mut last = 0;
        for (count, caps) in captures {
            let found = caps.get_match();
            out.push_str(&haystack[last..found.start()]);
            replacer.replace_append(&caps, &mut out);
            last = found.end();
            if limit > 0 && count + 1 >= limit {
                break;
            }
        }
        out.push_str(&haystack[last..]);
        Cow::Owned(out)
    }

    #[must_use]
    pub fn captures_len(&self) -> usize {
        self.pool.meta().group_len()
    }

    #[must_use]
    pub fn capture_names(&self) -> CaptureNames<'_> {
        CaptureNames(self.pool.meta().groups.names.iter())
    }
}

pub(crate) struct Stepper<'r> {
    pool: &'r Pool,
    guard: meta::Guard<'r>,
    position: usize,
    last_end: Option<usize>,
}

impl<'r> Stepper<'r> {
    fn new(pool: &'r Pool) -> Self {
        Self {
            pool,
            guard: pool.get(),
            position: 0,
            last_end: None,
        }
    }

    fn advance(&mut self, haystack: &[u8], slots: Option<&mut [usize]>) -> Option<(usize, usize)> {
        let meta = self.pool.meta();
        if self.position > haystack.len() {
            return None;
        }
        let mut found = meta.find(self.guard.cache(), haystack, self.position)?;
        if found.0 == found.1 && Some(found.1) == self.last_end {
            self.position += 1;
            if self.position > haystack.len() {
                return None;
            }
            found = meta.find(self.guard.cache(), haystack, self.position)?;
        }
        if let Some(slots) = slots {
            meta.captures_span(self.guard.cache(), haystack, found, slots);
        }
        self.position = found.1;
        self.last_end = Some(found.1);
        Some(found)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Match<'h> {
    haystack: &'h str,
    start: usize,
    end: usize,
}

impl<'h> Match<'h> {
    const fn new(haystack: &'h str, start: usize, end: usize) -> Self {
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
    pub fn as_str(&self) -> &'h str {
        &self.haystack[self.start..self.end]
    }
}

impl fmt::Debug for Match<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Match")
            .field("start", &self.start)
            .field("end", &self.end)
            .field("string", &self.as_str())
            .finish()
    }
}

impl<'h> From<Match<'h>> for &'h str {
    fn from(found: Match<'h>) -> &'h str {
        found.as_str()
    }
}

impl From<Match<'_>> for Range<usize> {
    fn from(found: Match<'_>) -> Self {
        found.range()
    }
}

pub struct Captures<'h> {
    haystack: &'h str,
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

    pub fn expand(&self, replacement: &str, destination: &mut String) {
        let mut bytes = Vec::new();
        expand_bytes(
            replacement.as_bytes(),
            &mut bytes,
            |index| self.get(index).map(|found| found.as_str().as_bytes()),
            |name| self.name(name).map(|found| found.as_str().as_bytes()),
        );
        destination.push_str(&String::from_utf8_lossy(&bytes));
    }
}

impl fmt::Debug for Captures<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut map = formatter.debug_map();
        for index in 0..self.len() {
            let key = self.groups.names[index]
                .as_ref()
                .map_or_else(|| index.to_string(), |name| format!("{index}/{name}"));
            map.entry(&key, &self.get(index).map(|found| found.as_str()));
        }
        map.finish()
    }
}

impl Index<usize> for Captures<'_> {
    type Output = str;

    fn index(&self, index: usize) -> &str {
        self.get(index).map_or_else(
            || panic!("no group at index '{index}'"),
            |found| found.as_str(),
        )
    }
}

impl<'n> Index<&'n str> for Captures<'_> {
    type Output = str;

    fn index(&self, name: &'n str) -> &str {
        self.name(name).map_or_else(
            || panic!("no group named '{name}'"),
            |found| found.as_str(),
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
    haystack: &'h str,
    stepper: Stepper<'r>,
}

impl<'h> Iterator for Matches<'_, 'h> {
    type Item = Match<'h>;

    fn next(&mut self) -> Option<Match<'h>> {
        let (start, end) = self.stepper.advance(self.haystack.as_bytes(), None)?;
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
    haystack: &'h str,
    stepper: Stepper<'r>,
}

impl<'h> Iterator for CaptureMatches<'_, 'h> {
    type Item = Captures<'h>;

    fn next(&mut self) -> Option<Captures<'h>> {
        let groups = Arc::clone(&self.stepper.pool.meta().groups);
        let mut slots = vec![NONE; groups.names.len() * 2];
        self.stepper
            .advance(self.haystack.as_bytes(), Some(&mut slots))?;
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
    type Item = &'h str;

    fn next(&mut self) -> Option<&'h str> {
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
    type Item = &'h str;

    fn next(&mut self) -> Option<&'h str> {
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

#[derive(Clone, Debug)]
pub struct CaptureNames<'r>(std::slice::Iter<'r, Option<Box<str>>>);

impl<'r> Iterator for CaptureNames<'r> {
    type Item = Option<&'r str>;

    fn next(&mut self) -> Option<Option<&'r str>> {
        self.0.next().map(|name| name.as_deref())
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl ExactSizeIterator for CaptureNames<'_> {}

impl FusedIterator for CaptureNames<'_> {}

enum Reference<'a> {
    Number(usize),
    Name(&'a str),
}

const fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn reference(replacement: &[u8]) -> Option<(Reference<'_>, usize)> {
    if replacement.len() <= 1 || replacement[0] != b'$' {
        return None;
    }
    let (name, end) = if replacement[1] == b'{' {
        let close = replacement[2..].iter().position(|&byte| byte == b'}')? + 2;
        (core::str::from_utf8(&replacement[2..close]).ok()?, close + 1)
    } else {
        let length = replacement[1..]
            .iter()
            .take_while(|&&byte| is_name_byte(byte))
            .count();
        if length == 0 {
            return None;
        }
        (core::str::from_utf8(&replacement[1..=length]).ok()?, length + 1)
    };
    Some((
        name.parse::<usize>()
            .map_or(Reference::Name(name), Reference::Number),
        end,
    ))
}

pub(crate) fn expand_bytes<'a>(
    mut replacement: &[u8],
    destination: &mut Vec<u8>,
    by_index: impl Fn(usize) -> Option<&'a [u8]>,
    by_name: impl Fn(&str) -> Option<&'a [u8]>,
) {
    while let Some(at) = crate::memchr::memchr(b'$', replacement) {
        destination.extend_from_slice(&replacement[..at]);
        replacement = &replacement[at..];
        if replacement.get(1) == Some(&b'$') {
            destination.push(b'$');
            replacement = &replacement[2..];
            continue;
        }
        match reference(replacement) {
            None => {
                destination.push(b'$');
                replacement = &replacement[1..];
            }
            Some((target, end)) => {
                let value = match target {
                    Reference::Number(index) => by_index(index),
                    Reference::Name(name) => by_name(name),
                };
                destination.extend_from_slice(value.unwrap_or_default());
                replacement = &replacement[end..];
            }
        }
    }
    destination.extend_from_slice(replacement);
}

pub trait Replacer {
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut String);

    fn no_expansion(&mut self) -> Option<Cow<'_, str>> {
        None
    }

    fn by_ref(&mut self) -> ReplacerRef<'_, Self> {
        ReplacerRef(self)
    }
}

#[derive(Debug)]
pub struct ReplacerRef<'a, R: ?Sized>(&'a mut R);

impl<R: Replacer + ?Sized> Replacer for ReplacerRef<'_, R> {
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut String) {
        self.0.replace_append(caps, dst);
    }

    fn no_expansion(&mut self) -> Option<Cow<'_, str>> {
        self.0.no_expansion()
    }
}

#[derive(Clone, Debug)]
pub struct NoExpand<'s>(pub &'s str);

impl Replacer for NoExpand<'_> {
    fn replace_append(&mut self, _: &Captures<'_>, dst: &mut String) {
        dst.push_str(self.0);
    }

    fn no_expansion(&mut self) -> Option<Cow<'_, str>> {
        Some(Cow::Borrowed(self.0))
    }
}

fn literal_replacement(replacement: &str) -> Option<Cow<'_, str>> {
    (!replacement.contains('$')).then_some(Cow::Borrowed(replacement))
}

impl Replacer for &str {
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut String) {
        caps.expand(self, dst);
    }

    fn no_expansion(&mut self) -> Option<Cow<'_, str>> {
        literal_replacement(self)
    }
}

impl Replacer for &String {
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut String) {
        caps.expand(self, dst);
    }

    fn no_expansion(&mut self) -> Option<Cow<'_, str>> {
        literal_replacement(self)
    }
}

impl Replacer for String {
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut String) {
        caps.expand(self, dst);
    }

    fn no_expansion(&mut self) -> Option<Cow<'_, str>> {
        literal_replacement(self)
    }
}

impl Replacer for Cow<'_, str> {
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut String) {
        caps.expand(self, dst);
    }

    fn no_expansion(&mut self) -> Option<Cow<'_, str>> {
        literal_replacement(self)
    }
}

impl Replacer for &Cow<'_, str> {
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut String) {
        caps.expand(self, dst);
    }

    fn no_expansion(&mut self) -> Option<Cow<'_, str>> {
        literal_replacement(self)
    }
}

impl<F, T> Replacer for F
where
    F: FnMut(&Captures<'_>) -> T,
    T: AsRef<str>,
{
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut String) {
        dst.push_str((*self)(caps).as_ref());
    }
}
