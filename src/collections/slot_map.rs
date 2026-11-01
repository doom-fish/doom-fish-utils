use core::fmt;
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;
use core::mem;
use core::num::NonZeroU32;
use core::ops::{Index, IndexMut};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct KeyData {
    idx: u32,
    version: NonZeroU32,
}

impl KeyData {
    const fn new(idx: u32, version: u32) -> Self {
        let Some(version) = NonZeroU32::new(version | 1) else {
            unreachable!()
        };
        Self { idx, version }
    }

    #[must_use]
    pub const fn as_ffi(self) -> u64 {
        ((self.version.get() as u64) << 32) | self.idx as u64
    }

    #[must_use]
    pub const fn from_ffi(value: u64) -> Self {
        Self::new((value & 0xffff_ffff) as u32, (value >> 32) as u32)
    }

    const fn is_null(self) -> bool {
        self.idx == u32::MAX
    }
}

impl Default for KeyData {
    fn default() -> Self {
        Self::new(u32::MAX, 1)
    }
}

impl fmt::Debug for KeyData {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_null() {
            formatter.write_str("null")
        } else {
            write!(formatter, "{}v{}", self.idx, self.version)
        }
    }
}

impl Hash for KeyData {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(self.as_ffi());
    }
}

pub trait Key: From<KeyData> + Copy {
    fn data(&self) -> KeyData;
}

#[macro_export]
macro_rules! new_key_type {
    ($(#[$attribute:meta])* $visibility:vis struct $name:ident;) => {
        $(#[$attribute])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[repr(transparent)]
        $visibility struct $name($crate::collections::KeyData);

        impl ::core::convert::From<$crate::collections::KeyData> for $name {
            fn from(data: $crate::collections::KeyData) -> Self {
                Self(data)
            }
        }

        impl $crate::collections::Key for $name {
            fn data(&self) -> $crate::collections::KeyData {
                self.0
            }
        }
    };
}

new_key_type! {
    pub struct DefaultKey;
}

#[derive(Clone, Debug)]
struct Slot<V> {
    version: u32,
    state: State<V>,
}

#[derive(Clone, Debug)]
enum State<V> {
    Occupied(V),
    Vacant(u32),
}

#[derive(Clone, Debug)]
pub struct SlotMap<K: Key, V> {
    slots: Vec<Slot<V>>,
    free_head: u32,
    len: usize,
    key: PhantomData<fn(K) -> K>,
}

impl<K: Key, V> SlotMap<K, V> {
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        let mut slots = Vec::with_capacity(capacity + 1);
        slots.push(Slot {
            version: 0,
            state: State::Vacant(0),
        });
        Self {
            slots,
            free_head: 1,
            len: 0,
            key: PhantomData,
        }
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn insert(&mut self, value: V) -> K {
        self.len += 1;
        if let Some(slot) = self.slots.get_mut(self.free_head as usize) {
            let index = self.free_head;
            if let State::Vacant(next_free) = mem::replace(&mut slot.state, State::Occupied(value))
            {
                self.free_head = next_free;
            }
            slot.version |= 1;
            return KeyData::new(index, slot.version).into();
        }
        let index = u32::try_from(self.slots.len())
            .ok()
            .filter(|&index| index < u32::MAX)
            .expect("SlotMap is full");
        self.slots.push(Slot {
            version: 1,
            state: State::Occupied(value),
        });
        self.free_head = index + 1;
        KeyData::new(index, 1).into()
    }

    pub fn remove(&mut self, key: K) -> Option<V> {
        let data = key.data();
        let slot = self.slots.get_mut(data.idx as usize)?;
        if slot.version != data.version.get() {
            return None;
        }
        slot.version = slot.version.wrapping_add(1);
        let state = mem::replace(&mut slot.state, State::Vacant(self.free_head));
        self.free_head = data.idx;
        self.len -= 1;
        match state {
            State::Occupied(value) => Some(value),
            State::Vacant(_) => None,
        }
    }

    pub fn clear(&mut self) {
        for index in 1..self.slots.len() {
            let slot = &mut self.slots[index];
            if let State::Occupied(_) = slot.state {
                slot.version = slot.version.wrapping_add(1);
                slot.state = State::Vacant(self.free_head);
                self.free_head = u32::try_from(index).unwrap_or(u32::MAX);
            }
        }
        self.len = 0;
    }

    #[must_use]
    pub fn get(&self, key: K) -> Option<&V> {
        let data = key.data();
        match self.slots.get(data.idx as usize) {
            Some(Slot {
                version,
                state: State::Occupied(value),
            }) if *version == data.version.get() => Some(value),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, key: K) -> Option<&mut V> {
        let data = key.data();
        match self.slots.get_mut(data.idx as usize) {
            Some(Slot {
                version,
                state: State::Occupied(value),
            }) if *version == data.version.get() => Some(value),
            _ => None,
        }
    }

    pub fn keys(&self) -> impl Iterator<Item = K> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| match slot.state {
                State::Occupied(_) => {
                    Some(KeyData::new(u32::try_from(index).ok()?, slot.version).into())
                }
                State::Vacant(_) => None,
            })
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.slots.iter().filter_map(|slot| match &slot.state {
            State::Occupied(value) => Some(value),
            State::Vacant(_) => None,
        })
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.slots
            .iter_mut()
            .filter_map(|slot| match &mut slot.state {
                State::Occupied(value) => Some(value),
                State::Vacant(_) => None,
            })
    }
}

impl<K: Key, V> Default for SlotMap<K, V> {
    fn default() -> Self {
        Self::with_capacity(0)
    }
}

impl<K: Key, V> Index<K> for SlotMap<K, V> {
    type Output = V;

    fn index(&self, key: K) -> &V {
        self.get(key).expect("invalid SlotMap key used")
    }
}

impl<K: Key, V> IndexMut<K> for SlotMap<K, V> {
    fn index_mut(&mut self, key: K) -> &mut V {
        self.get_mut(key).expect("invalid SlotMap key used")
    }
}

#[derive(Clone, Debug)]
struct Entry<V> {
    version: NonZeroU32,
    value: V,
}

#[derive(Clone, Debug)]
pub struct SecondaryMap<K: Key, V> {
    slots: Vec<Option<Entry<V>>>,
    key: PhantomData<fn(K) -> K>,
}

impl<K: Key, V> SecondaryMap<K, V> {
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            slots: Vec::with_capacity(capacity),
            key: PhantomData,
        }
    }

    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        let data = key.data();
        if data.is_null() {
            return None;
        }
        let index = data.idx as usize;
        if self.slots.len() <= index {
            self.slots.resize_with(index + 1, || None);
        }
        let slot = &mut self.slots[index];
        if let Some(entry) = slot {
            if entry.version == data.version {
                return Some(mem::replace(&mut entry.value, value));
            }
            if data.version.get().wrapping_sub(entry.version.get()) >= 1 << 31 {
                return None;
            }
        }
        *slot = Some(Entry {
            version: data.version,
            value,
        });
        None
    }

    pub fn remove(&mut self, key: K) -> Option<V> {
        let data = key.data();
        let slot = self.slots.get_mut(data.idx as usize)?;
        if slot.as_ref()?.version != data.version {
            return None;
        }
        slot.take().map(|entry| entry.value)
    }

    #[must_use]
    pub fn get(&self, key: K) -> Option<&V> {
        let data = key.data();
        match self.slots.get(data.idx as usize) {
            Some(Some(entry)) if entry.version == data.version => Some(&entry.value),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, key: K) -> Option<&mut V> {
        let data = key.data();
        match self.slots.get_mut(data.idx as usize) {
            Some(Some(entry)) if entry.version == data.version => Some(&mut entry.value),
            _ => None,
        }
    }

    pub fn get_disjoint_mut<const COUNT: usize>(
        &mut self,
        keys: [K; COUNT],
    ) -> Option<[&mut V; COUNT]> {
        for (position, key) in keys.iter().enumerate() {
            let index = key.data().idx;
            if self.get(*key).is_none()
                || keys[..position]
                    .iter()
                    .any(|other| other.data().idx == index)
            {
                return None;
            }
        }
        let slots = self.slots.as_mut_ptr();
        Some(keys.map(
            |key| match unsafe { &mut *slots.add(key.data().idx as usize) } {
                Some(entry) => &mut entry.value,
                None => unreachable!(),
            },
        ))
    }
}

impl<K: Key, V> Default for SecondaryMap<K, V> {
    fn default() -> Self {
        Self::with_capacity(0)
    }
}
