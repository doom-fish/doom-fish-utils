use super::dfa::{Dfa, DfaCache, GaveUp};
use super::hir::Hir;
use super::literal::{self, Exact, Prefilter};
use super::nfa::{self, Nfa, Options};
use super::parse::{self, Config, Flags};
use super::pikevm::{self, PikeCache, Search, NONE};
use super::Error;
use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

#[derive(Clone, Debug)]
pub struct Settings {
    pub flags: Flags,
    pub size_limit: usize,
    pub dfa_size_limit: usize,
    pub nest_limit: u32,
}

impl Settings {
    pub const fn new() -> Self {
        Self {
            flags: Flags {
                case_insensitive: false,
                multi_line: false,
                dot_matches_new_line: false,
                swap_greed: false,
                ignore_whitespace: false,
                unicode: true,
            },
            size_limit: 10 * (1 << 20),
            dfa_size_limit: 2 * (1 << 20),
            nest_limit: 250,
        }
    }
}

#[derive(Debug)]
pub struct GroupInfo {
    pub names: Vec<Option<Box<str>>>,
    pub map: HashMap<Box<str>, usize>,
}

impl GroupInfo {
    pub fn index(&self, name: &str) -> Option<usize> {
        self.map.get(name).copied()
    }
}

#[derive(Debug)]
struct Reverse {
    nfa: Nfa,
    dfa: Dfa,
}

#[derive(Debug)]
struct ReverseInner {
    literal: Prefilter,
    nfa: Nfa,
    dfa: Dfa,
}

#[derive(Debug)]
pub struct Meta {
    pub pattern: Box<str>,
    pub groups: Arc<GroupInfo>,
    hir: Hir,
    size_limit: usize,
    dfa_size_limit: usize,
    forward: Nfa,
    reverse: OnceLock<Option<Reverse>>,
    forward_dfa: Dfa,
    exact: Option<Exact>,
    prefilter: Option<Prefilter>,
    inner: Option<ReverseInner>,
    start_anchored: bool,
    utf8: bool,
    min_len: usize,
}

#[derive(Debug)]
pub struct Cache {
    forward: DfaCache,
    reverse: Option<DfaCache>,
    inner: Option<DfaCache>,
    pike: Option<PikeCache>,
}

fn syntax_error(pattern: &str, error: &parse::ParseError) -> Error {
    let mut text = String::from("regex parse error:\n");
    if pattern.contains('\n') {
        text.push_str("    ");
        text.push_str(pattern);
        text.push('\n');
    } else {
        let column = pattern[..error.offset.min(pattern.len())].chars().count();
        text.push_str("    ");
        text.push_str(pattern);
        text.push_str("\n    ");
        text.push_str(&" ".repeat(column));
        text.push_str("^\n");
    }
    text.push_str("error: ");
    text.push_str(&error.message);
    Error::Syntax(text)
}

impl Meta {
    pub fn new(pattern: &str, settings: &Settings, utf8: bool) -> Result<Self, Error> {
        let config = Config {
            flags: settings.flags,
            utf8,
            nest_limit: settings.nest_limit,
        };
        let parsed = parse::parse(pattern, &config).map_err(|error| syntax_error(pattern, &error))?;
        let hir: Hir = parsed.hir;
        let groups = parsed.names.len();
        let too_big = |_| Error::CompiledTooBig(settings.size_limit);
        let forward = nfa::build(
            &hir,
            &Options {
                reverse: false,
                captures: true,
                size_limit: settings.size_limit,
                groups,
            },
        )
        .map_err(too_big)?;
        let exact = literal::exact(&hir);
        let prefilter = if exact.is_none() {
            literal::prefilter(&hir)
        } else {
            None
        };
        let start_anchored = hir.is_start_anchored();
        let inner = if exact.is_none() && prefilter.is_none() && !start_anchored {
            literal::inner(&hir).and_then(|inner| {
                let nfa = nfa::build(
                    &inner.prefix,
                    &Options {
                        reverse: true,
                        captures: false,
                        size_limit: settings.size_limit,
                        groups,
                    },
                )
                .ok()?;
                let literal = if inner.literal.len() == 1 {
                    Prefilter::Byte(inner.literal[0])
                } else {
                    Prefilter::Substring(crate::memchr::memmem::Finder::new(&inner.literal).into_owned())
                };
                Some(ReverseInner {
                    literal,
                    dfa: Dfa::new(&nfa, false, false, settings.dfa_size_limit),
                    nfa,
                })
            })
        } else {
            None
        };
        let map = parsed
            .names
            .iter()
            .enumerate()
            .filter_map(|(index, name)| name.clone().map(|name| (name, index)))
            .collect();
        Ok(Self {
            pattern: pattern.into(),
            groups: Arc::new(GroupInfo {
                names: parsed.names,
                map,
            }),
            min_len: hir.min_len(),
            forward_dfa: Dfa::new(&forward, true, prefilter.is_some(), settings.dfa_size_limit),
            forward,
            reverse: OnceLock::new(),
            hir,
            size_limit: settings.size_limit,
            dfa_size_limit: settings.dfa_size_limit,
            exact,
            prefilter,
            inner,
            start_anchored,
            utf8,
        })
    }

    pub fn cache(&self) -> Cache {
        Cache {
            forward: Dfa::cache(&self.forward),
            reverse: None,
            inner: self.inner.as_ref().map(|inner| Dfa::cache(&inner.nfa)),
            pike: None,
        }
    }

    fn reverse(&self) -> Option<&Reverse> {
        self.reverse
            .get_or_init(|| {
                let nfa = nfa::build(
                    &self.hir,
                    &Options {
                        reverse: true,
                        captures: false,
                        size_limit: self.size_limit,
                        groups: 0,
                    },
                )
                .ok()?;
                Some(Reverse {
                    dfa: Dfa::new(&nfa, false, false, self.dfa_size_limit),
                    nfa,
                })
            })
            .as_ref()
    }

    pub fn group_len(&self) -> usize {
        self.groups.names.len()
    }

    fn pike(&self, cache: &mut Cache, search: &Search<'_>, slots: &mut [usize]) -> bool {
        let pike = cache.pike.get_or_insert_with(|| PikeCache::new(&self.forward));
        match &self.prefilter {
            Some(prefilter) => {
                let skip = |haystack: &[u8], start: usize, end: usize| prefilter.find(haystack, start, end);
                pikevm::search(&self.forward, pike, search, Some(&skip), slots)
            }
            None => pikevm::search(&self.forward, pike, search, None, slots),
        }
    }

    fn forward(
        &self,
        cache: &mut Cache,
        haystack: &[u8],
        start: usize,
        earliest: bool,
    ) -> Result<Option<usize>, GaveUp> {
        match &self.prefilter {
            Some(prefilter) => {
                let skip = |haystack: &[u8], start: usize, end: usize| prefilter.find(haystack, start, end);
                self.forward_dfa.find_fwd(
                    &self.forward,
                    &mut cache.forward,
                    haystack,
                    start,
                    haystack.len(),
                    self.start_anchored,
                    earliest,
                    Some(&skip),
                )
            }
            None => self.forward_dfa.find_fwd(
                &self.forward,
                &mut cache.forward,
                haystack,
                start,
                haystack.len(),
                self.start_anchored,
                earliest,
                None,
            ),
        }
    }

    fn pike_find(&self, cache: &mut Cache, haystack: &[u8], start: usize, earliest: bool) -> Option<(usize, usize)> {
        let mut slots = [NONE; 2];
        let search = Search {
            haystack,
            start,
            end: haystack.len(),
            anchored: self.start_anchored,
            earliest,
        };
        let found = self.pike(cache, &search, &mut slots);
        found.then_some(slots.into())
    }

    const fn rejects(&self, haystack: &[u8], start: usize) -> bool {
        start > haystack.len()
            || haystack.len() - start < self.min_len
            || (self.start_anchored && start > 0)
    }

    fn find_raw(&self, cache: &mut Cache, haystack: &[u8], start: usize) -> Option<(usize, usize)> {
        if start > haystack.len() {
            return None;
        }
        match &self.exact {
            Some(Exact::Literal(finder, length)) => {
                return finder
                    .find(&haystack[start..])
                    .map(|offset| (start + offset, start + offset + length));
            }
            Some(Exact::CaseInsensitive(finder, length)) => {
                return finder
                    .find(&haystack[start..])
                    .map(|offset| (start + offset, start + offset + length));
            }
            Some(Exact::Teddy(teddy)) => {
                return teddy
                    .find(haystack, start, haystack.len())
                    .map(|(at, index)| (at, at + teddy.literal_len(index)));
            }
            None => {}
        }
        if self.rejects(haystack, start) {
            return None;
        }
        if let Some(inner) = &self.inner {
            if let Ok(found) = self.reverse_inner(inner, cache, haystack, start) {
                return found;
            }
        }
        match self.forward(cache, haystack, start, false) {
            Ok(None) => None,
            Ok(Some(end)) => {
                if self.start_anchored {
                    return Some((start, end));
                }
                if let Some(reverse) = self.reverse() {
                    let rev = cache.reverse.get_or_insert_with(|| Dfa::cache(&reverse.nfa));
                    if let Ok(Some(begin)) = reverse.dfa.find_rev(&reverse.nfa, rev, haystack, start, end) {
                        return Some((begin, end));
                    }
                }
                self.pike_find(cache, haystack, start, false)
            }
            Err(GaveUp) => self.pike_find(cache, haystack, start, false),
        }
    }

    fn reverse_inner(
        &self,
        inner: &ReverseInner,
        cache: &mut Cache,
        haystack: &[u8],
        start: usize,
    ) -> Result<Option<(usize, usize)>, GaveUp> {
        let mut at = start;
        let mut scanned = 0;
        loop {
            let Some(position) = inner.literal.find(haystack, at, haystack.len()) else {
                return Ok(None);
            };
            if position < scanned {
                return Err(GaveUp);
            }
            let reverse = cache.inner.as_mut().ok_or(GaveUp)?;
            if let Some(begin) = inner.dfa.find_rev(&inner.nfa, reverse, haystack, start, position)? {
                let end = self.forward_dfa.find_fwd(
                    &self.forward,
                    &mut cache.forward,
                    haystack,
                    begin,
                    haystack.len(),
                    true,
                    false,
                    None,
                )?;
                if let Some(end) = end {
                    return Ok(Some((begin, end)));
                }
                scanned = cache.forward.stopped;
            }
            at = position + 1;
        }
    }

    pub fn find(&self, cache: &mut Cache, haystack: &[u8], mut start: usize) -> Option<(usize, usize)> {
        loop {
            let found = self.find_raw(cache, haystack, start)?;
            if !self.utf8 || found.0 != found.1 || is_boundary(haystack, found.0) {
                return Some(found);
            }
            start = found.0 + 1;
        }
    }

    pub fn shortest(&self, cache: &mut Cache, haystack: &[u8], mut start: usize) -> Option<usize> {
        loop {
            let end = self.shortest_raw(cache, haystack, start)?;
            if !self.utf8 || is_boundary(haystack, end) {
                return Some(end);
            }
            start += 1;
        }
    }

    fn shortest_raw(&self, cache: &mut Cache, haystack: &[u8], start: usize) -> Option<usize> {
        if self.exact.is_some() {
            return self.find_raw(cache, haystack, start).map(|found| found.1);
        }
        if self.rejects(haystack, start) {
            return None;
        }
        match self.forward(cache, haystack, start, true) {
            Ok(found) => found,
            Err(GaveUp) => self.pike_find(cache, haystack, start, true).map(|found| found.1),
        }
    }

    pub fn is_match(&self, cache: &mut Cache, haystack: &[u8], start: usize) -> bool {
        if self.utf8 && self.min_len == 0 {
            return self.find(cache, haystack, start).is_some();
        }
        self.shortest_raw(cache, haystack, start).is_some()
    }

    pub fn captures(&self, cache: &mut Cache, haystack: &[u8], start: usize, slots: &mut [usize]) -> bool {
        slots.fill(NONE);
        let Some(found) = self.find(cache, haystack, start) else {
            return false;
        };
        self.captures_span(cache, haystack, found, slots);
        true
    }

    pub fn captures_span(&self, cache: &mut Cache, haystack: &[u8], found: (usize, usize), slots: &mut [usize]) {
        slots.fill(NONE);
        if slots.len() > 2 && self.group_len() > 1 {
            let search = Search {
                haystack,
                start: found.0,
                end: found.1,
                anchored: true,
                earliest: false,
            };
            let pike = cache.pike.get_or_insert_with(|| PikeCache::new(&self.forward));
            if pikevm::search(&self.forward, pike, &search, None, slots)
                && slots[0] == found.0
                && slots[1] == found.1
            {
                return;
            }
            slots.fill(NONE);
        }
        slots[0] = found.0;
        slots[1] = found.1;
    }
}

fn is_boundary(haystack: &[u8], at: usize) -> bool {
    haystack.get(at).is_none_or(|&byte| byte & 0xC0 != 0x80)
}

static NEXT_THREAD: AtomicUsize = AtomicUsize::new(3);

thread_local! {
    static THREAD: usize = NEXT_THREAD.fetch_add(1, Ordering::Relaxed);
}

const UNOWNED: usize = 0;
const IN_USE: usize = 1;

pub struct Pool {
    meta: Arc<Meta>,
    stack: Mutex<Vec<Cache>>,
    owner: AtomicUsize,
    owned: UnsafeCell<Option<Cache>>,
}

unsafe impl Sync for Pool {}

impl UnwindSafe for Pool {}

impl RefUnwindSafe for Pool {}

impl core::fmt::Debug for Pool {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_struct("Pool").finish_non_exhaustive()
    }
}

pub enum Guard<'a> {
    Owned(&'a Pool, usize),
    Shared(&'a Pool, Option<Box<Cache>>),
}

impl Pool {
    pub const fn new(meta: Arc<Meta>) -> Self {
        Self {
            meta,
            stack: Mutex::new(Vec::new()),
            owner: AtomicUsize::new(UNOWNED),
            owned: UnsafeCell::new(None),
        }
    }

    pub fn meta(&self) -> &Meta {
        &self.meta
    }

    pub const fn meta_arc(&self) -> &Arc<Meta> {
        &self.meta
    }

    pub fn get(&self) -> Guard<'_> {
        let Ok(caller) = THREAD.try_with(|id| *id) else {
            return self.shared();
        };
        let owner = self.owner.load(Ordering::Acquire);
        if caller == owner {
            self.owner.store(IN_USE, Ordering::Release);
            return Guard::Owned(self, caller);
        }
        if owner == UNOWNED
            && self
                .owner
                .compare_exchange(UNOWNED, IN_USE, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            let cache = self.meta.cache();
            unsafe {
                *self.owned.get() = Some(cache);
            }
            return Guard::Owned(self, caller);
        }
        self.shared()
    }

    fn shared(&self) -> Guard<'_> {
        let cache = self
            .stack
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop()
            .unwrap_or_else(|| self.meta.cache());
        Guard::Shared(self, Some(Box::new(cache)))
    }
}

impl Guard<'_> {
    pub fn cache(&mut self) -> &mut Cache {
        match self {
            Self::Owned(pool, _) => unsafe {
                (*pool.owned.get())
                    .as_mut()
                    .unwrap_or_else(|| unreachable!())
            },
            Self::Shared(_, cache) => cache.as_mut().unwrap_or_else(|| unreachable!()),
        }
    }
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        match self {
            Self::Owned(pool, caller) => pool.owner.store(*caller, Ordering::Release),
            Self::Shared(pool, cache) => {
                if let Some(cache) = cache.take() {
                    let mut stack = pool.stack.lock().unwrap_or_else(PoisonError::into_inner);
                    if stack.len() < 64 {
                        stack.push(*cache);
                    }
                }
            }
        }
    }
}
