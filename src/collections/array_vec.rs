use core::fmt;
use core::mem::{ManuallyDrop, MaybeUninit};
use core::ops::{Bound, Deref, DerefMut, RangeBounds};
use core::ptr;
use core::slice;

pub struct ArrayVec<T, const CAP: usize> {
    len: usize,
    items: [MaybeUninit<T>; CAP],
}

impl<T, const CAP: usize> ArrayVec<T, CAP> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            len: 0,
            items: [const { MaybeUninit::uninit() }; CAP],
        }
    }

    #[must_use]
    pub const fn as_slice(&self) -> &[T] {
        unsafe { slice::from_raw_parts(self.items.as_ptr().cast(), self.len) }
    }

    fn as_mut_slice(&mut self) -> &mut [T] {
        unsafe { slice::from_raw_parts_mut(self.items.as_mut_ptr().cast(), self.len) }
    }

    #[must_use]
    pub(super) const fn is_full(&self) -> bool {
        self.len == CAP
    }

    pub fn push(&mut self, value: T) {
        assert!(self.len < CAP, "ArrayVec is full");
        self.items[self.len].write(value);
        self.len += 1;
    }

    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        Some(unsafe { self.items[self.len].assume_init_read() })
    }

    pub fn insert(&mut self, index: usize, value: T) {
        assert!(index <= self.len, "insertion index is out of bounds");
        assert!(self.len < CAP, "ArrayVec is full");
        unsafe {
            let at = self.items.as_mut_ptr().add(index);
            ptr::copy(at, at.add(1), self.len - index);
            at.write(MaybeUninit::new(value));
        }
        self.len += 1;
    }

    pub(super) fn remove(&mut self, index: usize) -> T {
        assert!(index < self.len, "removal index is out of bounds");
        self.len -= 1;
        unsafe {
            let at = self.items.as_mut_ptr().add(index);
            let value = at.read().assume_init();
            ptr::copy(at.add(1), at, self.len - index);
            value
        }
    }

    pub(super) fn truncate(&mut self, len: usize) {
        while self.len > len {
            self.pop();
        }
    }

    pub fn clear(&mut self) {
        self.truncate(0);
    }

    pub(super) fn retain(&mut self, mut keep: impl FnMut(&mut T) -> bool) {
        let len = self.len;
        self.len = 0;
        for index in 0..len {
            let mut value = unsafe { self.items[index].assume_init_read() };
            if keep(&mut value) {
                self.items[self.len].write(value);
                self.len += 1;
            }
        }
    }

    pub fn drain(&mut self, range: impl RangeBounds<usize>) -> IntoIter<T, CAP> {
        let start = match range.start_bound() {
            Bound::Included(&start) => Some(start),
            Bound::Excluded(&start) => start.checked_add(1),
            Bound::Unbounded => Some(0),
        };
        let end = match range.end_bound() {
            Bound::Included(&end) => end.checked_add(1),
            Bound::Excluded(&end) => Some(end),
            Bound::Unbounded => Some(self.len),
        };
        let (Some(start), Some(end)) = (start, end) else {
            panic!("drain range is out of bounds");
        };
        assert!(
            start <= end && end <= self.len,
            "drain range is out of bounds"
        );
        let mut drained = Self::new();
        unsafe {
            let at = self.items.as_mut_ptr().add(start);
            ptr::copy_nonoverlapping(at, drained.items.as_mut_ptr(), end - start);
            ptr::copy(at.add(end - start), at, self.len - end);
        }
        drained.len = end - start;
        self.len -= end - start;
        drained.into_iter()
    }
}

impl<T, const CAP: usize> Drop for ArrayVec<T, CAP> {
    fn drop(&mut self) {
        unsafe { ptr::drop_in_place(self.as_mut_slice()) }
    }
}

impl<T, const CAP: usize> Default for ArrayVec<T, CAP> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const CAP: usize> Deref for ArrayVec<T, CAP> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T, const CAP: usize> DerefMut for ArrayVec<T, CAP> {
    fn deref_mut(&mut self) -> &mut [T] {
        self.as_mut_slice()
    }
}

impl<T: Clone, const CAP: usize> Clone for ArrayVec<T, CAP> {
    fn clone(&self) -> Self {
        self.iter().cloned().collect()
    }
}

impl<T: fmt::Debug, const CAP: usize> fmt::Debug for ArrayVec<T, CAP> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_slice().fmt(formatter)
    }
}

impl<T, const CAP: usize> Extend<T> for ArrayVec<T, CAP> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        for value in iter {
            self.push(value);
        }
    }
}

impl<T, const CAP: usize> FromIterator<T> for ArrayVec<T, CAP> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut array = Self::new();
        array.extend(iter);
        array
    }
}

impl<T, const CAP: usize> IntoIterator for ArrayVec<T, CAP> {
    type Item = T;
    type IntoIter = IntoIter<T, CAP>;

    fn into_iter(self) -> IntoIter<T, CAP> {
        let array = ManuallyDrop::new(self);
        IntoIter {
            items: unsafe { ptr::read(&raw const array.items) },
            start: 0,
            end: array.len,
        }
    }
}

impl<'a, T, const CAP: usize> IntoIterator for &'a ArrayVec<T, CAP> {
    type Item = &'a T;
    type IntoIter = slice::Iter<'a, T>;

    fn into_iter(self) -> slice::Iter<'a, T> {
        self.iter()
    }
}

pub struct IntoIter<T, const CAP: usize> {
    items: [MaybeUninit<T>; CAP],
    start: usize,
    end: usize,
}

impl<T, const CAP: usize> Iterator for IntoIter<T, CAP> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        if self.start == self.end {
            return None;
        }
        self.start += 1;
        Some(unsafe { self.items[self.start - 1].assume_init_read() })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.end - self.start, Some(self.end - self.start))
    }
}

impl<T, const CAP: usize> DoubleEndedIterator for IntoIter<T, CAP> {
    fn next_back(&mut self) -> Option<T> {
        if self.start == self.end {
            return None;
        }
        self.end -= 1;
        Some(unsafe { self.items[self.end].assume_init_read() })
    }
}

impl<T, const CAP: usize> Drop for IntoIter<T, CAP> {
    fn drop(&mut self) {
        for item in &mut self.items[self.start..self.end] {
            unsafe { item.assume_init_drop() }
        }
    }
}
