use super::array_vec::{self, ArrayVec};
use core::cmp::Ordering;
use core::fmt;
use core::hash::{Hash, Hasher};
use core::ops::{Deref, DerefMut, RangeBounds};
use std::vec;

pub struct SmallVec<T, const N: usize>(Storage<T, N>);

enum Storage<T, const N: usize> {
    Inline(ArrayVec<T, N>),
    Heap(Vec<T>),
}

impl<T, const N: usize> SmallVec<T, N> {
    #[must_use]
    pub const fn new() -> Self {
        Self(Storage::Inline(ArrayVec::new()))
    }

    #[must_use]
    pub const fn inline_size(&self) -> usize {
        N
    }

    #[must_use]
    pub fn as_slice(&self) -> &[T] {
        match &self.0 {
            Storage::Inline(array) => array,
            Storage::Heap(vec) => vec,
        }
    }

    fn as_mut_slice(&mut self) -> &mut [T] {
        match &mut self.0 {
            Storage::Inline(array) => array,
            Storage::Heap(vec) => vec,
        }
    }

    pub fn push(&mut self, value: T) {
        match &mut self.0 {
            Storage::Inline(array) if !array.is_full() => array.push(value),
            _ => self.heap().push(value),
        }
    }

    pub fn pop(&mut self) -> Option<T> {
        match &mut self.0 {
            Storage::Inline(array) => array.pop(),
            Storage::Heap(vec) => vec.pop(),
        }
    }

    pub fn remove(&mut self, index: usize) -> T {
        match &mut self.0 {
            Storage::Inline(array) => array.remove(index),
            Storage::Heap(vec) => vec.remove(index),
        }
    }

    pub fn truncate(&mut self, len: usize) {
        match &mut self.0 {
            Storage::Inline(array) => array.truncate(len),
            Storage::Heap(vec) => vec.truncate(len),
        }
    }

    pub fn clear(&mut self) {
        self.truncate(0);
    }

    pub fn reserve(&mut self, additional: usize) {
        match &mut self.0 {
            Storage::Inline(array) if array.len().saturating_add(additional) <= N => {}
            _ => self.heap().reserve(additional),
        }
    }

    pub fn extend_from_slice(&mut self, slice: &[T])
    where
        T: Clone,
    {
        self.extend(slice.iter().cloned());
    }

    pub fn dedup(&mut self)
    where
        T: PartialEq,
    {
        let slice = self.as_mut_slice();
        let mut kept = 1;
        for index in 1..slice.len() {
            if slice[index] != slice[kept - 1] {
                slice.swap(kept, index);
                kept += 1;
            }
        }
        self.truncate(kept);
    }

    pub fn retain(&mut self, keep: impl FnMut(&mut T) -> bool) {
        match &mut self.0 {
            Storage::Inline(array) => array.retain(keep),
            Storage::Heap(vec) => vec.retain_mut(keep),
        }
    }

    pub fn drain(&mut self, range: impl RangeBounds<usize>) -> Drain<'_, T, N> {
        match &mut self.0 {
            Storage::Inline(array) => Drain::Inline(array.drain(range)),
            Storage::Heap(vec) => Drain::Heap(vec.drain(range)),
        }
    }

    fn heap(&mut self) -> &mut Vec<T> {
        if let Storage::Inline(array) = &mut self.0 {
            self.0 = Storage::Heap(array.drain(..).collect());
        }
        match &mut self.0 {
            Storage::Heap(vec) => vec,
            Storage::Inline(_) => unreachable!(),
        }
    }
}

impl<T, const N: usize> Default for SmallVec<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const N: usize> Deref for SmallVec<T, N> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T, const N: usize> DerefMut for SmallVec<T, N> {
    fn deref_mut(&mut self) -> &mut [T] {
        self.as_mut_slice()
    }
}

impl<T: Clone, const N: usize> Clone for SmallVec<T, N> {
    fn clone(&self) -> Self {
        self.iter().cloned().collect()
    }
}

impl<T: fmt::Debug, const N: usize> fmt::Debug for SmallVec<T, N> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_slice().fmt(formatter)
    }
}

impl<T: PartialEq, const N: usize> PartialEq for SmallVec<T, N> {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl<T: Eq, const N: usize> Eq for SmallVec<T, N> {}

impl<T: PartialOrd, const N: usize> PartialOrd for SmallVec<T, N> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.as_slice().partial_cmp(other.as_slice())
    }
}

impl<T: Ord, const N: usize> Ord for SmallVec<T, N> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_slice().cmp(other.as_slice())
    }
}

impl<T: Hash, const N: usize> Hash for SmallVec<T, N> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_slice().hash(state);
    }
}

impl<T, const N: usize> Extend<T> for SmallVec<T, N> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        let iter = iter.into_iter();
        self.reserve(iter.size_hint().0);
        for value in iter {
            self.push(value);
        }
    }
}

impl<T, const N: usize> FromIterator<T> for SmallVec<T, N> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut vec = Self::new();
        vec.extend(iter);
        vec
    }
}

impl<T, const N: usize> From<Vec<T>> for SmallVec<T, N> {
    fn from(vec: Vec<T>) -> Self {
        if vec.capacity() <= N {
            vec.into_iter().collect()
        } else {
            Self(Storage::Heap(vec))
        }
    }
}

impl<T: Clone, const N: usize> From<&[T]> for SmallVec<T, N> {
    fn from(slice: &[T]) -> Self {
        slice.iter().cloned().collect()
    }
}

impl<T, const N: usize> IntoIterator for SmallVec<T, N> {
    type Item = T;
    type IntoIter = IntoIter<T, N>;

    fn into_iter(self) -> IntoIter<T, N> {
        match self.0 {
            Storage::Inline(array) => IntoIter::Inline(array.into_iter()),
            Storage::Heap(vec) => IntoIter::Heap(vec.into_iter()),
        }
    }
}

impl<'a, T, const N: usize> IntoIterator for &'a SmallVec<T, N> {
    type Item = &'a T;
    type IntoIter = core::slice::Iter<'a, T>;

    fn into_iter(self) -> core::slice::Iter<'a, T> {
        self.iter()
    }
}

impl<'a, T, const N: usize> IntoIterator for &'a mut SmallVec<T, N> {
    type Item = &'a mut T;
    type IntoIter = core::slice::IterMut<'a, T>;

    fn into_iter(self) -> core::slice::IterMut<'a, T> {
        self.iter_mut()
    }
}

pub enum IntoIter<T, const N: usize> {
    Inline(array_vec::IntoIter<T, N>),
    Heap(vec::IntoIter<T>),
}

impl<T, const N: usize> Iterator for IntoIter<T, N> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        match self {
            Self::Inline(iter) => iter.next(),
            Self::Heap(iter) => iter.next(),
        }
    }
}

impl<T, const N: usize> DoubleEndedIterator for IntoIter<T, N> {
    fn next_back(&mut self) -> Option<T> {
        match self {
            Self::Inline(iter) => iter.next_back(),
            Self::Heap(iter) => iter.next_back(),
        }
    }
}

pub enum Drain<'a, T, const N: usize> {
    Inline(array_vec::IntoIter<T, N>),
    Heap(vec::Drain<'a, T>),
}

impl<T, const N: usize> Iterator for Drain<'_, T, N> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        match self {
            Self::Inline(iter) => iter.next(),
            Self::Heap(iter) => iter.next(),
        }
    }
}

impl<T, const N: usize> DoubleEndedIterator for Drain<'_, T, N> {
    fn next_back(&mut self) -> Option<T> {
        match self {
            Self::Inline(iter) => iter.next_back(),
            Self::Heap(iter) => iter.next_back(),
        }
    }
}
