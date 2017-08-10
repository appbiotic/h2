use super::*;

use slab;

use std::ops;
use std::collections::{HashMap, hash_map};
use std::marker::PhantomData;

/// Storage for streams
#[derive(Debug)]
pub(super) struct Store<B> {
    slab: slab::Slab<Stream<B>>,
    ids: HashMap<StreamId, usize>,
}

/// "Pointer" to an entry in the store
pub(super) struct Ptr<'a, B: 'a> {
    key: Key,
    store: &'a mut Store<B>,
}

/// References an entry in the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Key(usize);

#[derive(Debug)]
pub(super) struct List<B> {
    indices: Option<store::Indices>,
    _p: PhantomData<B>,
}

pub(super) trait Next {
    fn next<B>(stream: &Stream<B>) -> Option<Key>;

    fn set_next<B>(stream: &mut Stream<B>, key: Option<Key>);

    fn take_next<B>(stream: &mut Stream<B>) -> Option<Key>;
}

/// A linked list
#[derive(Debug, Clone, Copy)]
struct Indices {
    pub head: Key,
    pub tail: Key,
}

pub(super) enum Entry<'a, B: 'a> {
    Occupied(OccupiedEntry<'a, B>),
    Vacant(VacantEntry<'a, B>),
}

pub(super) struct OccupiedEntry<'a, B: 'a> {
    ids: hash_map::OccupiedEntry<'a, StreamId, usize>,
    slab: &'a mut slab::Slab<Stream<B>>,
}

pub(super) struct VacantEntry<'a, B: 'a> {
    ids: hash_map::VacantEntry<'a, StreamId, usize>,
    slab: &'a mut slab::Slab<Stream<B>>,
}

// ===== impl Store =====

impl<B> Store<B> {
    pub fn new() -> Self {
        Store {
            slab: slab::Slab::new(),
            ids: HashMap::new(),
        }
    }

    pub fn resolve(&mut self, key: Key) -> Ptr<B> {
        Ptr {
            key: key,
            store: self,
        }
    }

    pub fn find_mut(&mut self, id: &StreamId) -> Option<Ptr<B>> {
        if let Some(&key) = self.ids.get(id) {
            Some(Ptr {
                key: Key(key),
                store: self,
            })
        } else {
            None
        }
    }

    pub fn insert(&mut self, id: StreamId, val: Stream<B>) -> Ptr<B> {
        let key = self.slab.insert(val);
        assert!(self.ids.insert(id, key).is_none());

        Ptr {
            key: Key(key),
            store: self,
        }
    }

    pub fn find_entry(&mut self, id: StreamId) -> Entry<B> {
        use self::hash_map::Entry::*;

        match self.ids.entry(id) {
            Occupied(e) => {
                Entry::Occupied(OccupiedEntry {
                    ids: e,
                    slab: &mut self.slab,
                })
            }
            Vacant(e) => {
                Entry::Vacant(VacantEntry {
                    ids: e,
                    slab: &mut self.slab,
                })
            }
        }
    }

    pub fn for_each<F>(&mut self, mut f: F)
        where F: FnMut(&mut Stream<B>)
    {
        for &id in self.ids.values() {
            f(&mut self.slab[id])
        }
    }
}

impl<B> ops::Index<Key> for Store<B> {
    type Output = Stream<B>;

    fn index(&self, key: Key) -> &Self::Output {
        self.slab.index(key.0)
    }
}

impl<B> ops::IndexMut<Key> for Store<B> {
    fn index_mut(&mut self, key: Key) -> &mut Self::Output {
        self.slab.index_mut(key.0)
    }
}

// ===== impl List =====

impl<B> List<B> {
    pub fn new() -> Self {
        List {
            indices: None,
            _p: PhantomData,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_none()
    }

    pub fn take(&mut self) -> Self {
        List {
            indices: self.indices.take(),
            _p: PhantomData,
        }
    }

    pub fn push<N>(&mut self, stream: &mut store::Ptr<B>)
        where N: Next,
    {
        // The next pointer shouldn't be set
        debug_assert!(N::next(stream).is_none());

        // Queue the stream
        match self.indices {
            Some(ref mut idxs) => {
                // Update the current tail node to point to `stream`
                let key = stream.key();
                N::set_next(&mut stream.resolve(idxs.tail), Some(key));

                // Update the tail pointer
                idxs.tail = stream.key();
            }
            None => {
                self.indices = Some(store::Indices {
                    head: stream.key(),
                    tail: stream.key(),
                });
            }
        }
    }

    pub fn pop<'a, N>(&mut self, store: &'a mut Store<B>) -> Option<store::Ptr<'a, B>>
        where N: Next,
    {
        if let Some(mut idxs) = self.indices {
            let mut stream = store.resolve(idxs.head);

            if idxs.head == idxs.tail {
                assert!(N::next(&*stream).is_none());
                self.indices = None;
            } else {
                idxs.head = N::take_next(&mut *stream).unwrap();
                self.indices = Some(idxs);
            }

            return Some(stream);
        }

        None
    }

    pub fn retain<N, F>(&mut self, store: &mut Store<B>, mut f: F)
        where N: Next,
              F: FnMut(&mut Stream<B>) -> bool,
    {
        if let Some(mut idxs) = self.indices {
            let mut prev = None;
            let mut curr = idxs.head;

            loop {
                if f(&mut store[curr]) {
                    // Element is retained, walk to the next
                    if let Some(next) = N::next(&mut store[curr]) {
                        prev = Some(curr);
                        curr = next;
                    } else {
                        // Tail
                        break;
                    }
                } else {
                    // Element is dropped
                    if let Some(prev) = prev {
                        let next = N::take_next(&mut store[curr]);
                        N::set_next(&mut store[prev], next);

                        // current is last element, but guaranteed to not be the
                        // only one
                        if next.is_none() {
                            idxs.tail = prev;
                            break;
                        }
                    } else {
                        if let Some(next) = N::take_next(&mut store[curr]) {
                            curr = next;
                            idxs.head = next;
                        } else {
                            // Only element
                            self.indices = None;
                            return;
                        }
                    }
                }
            }

            self.indices = Some(idxs);
        }
    }
}

// ===== impl Ptr =====

impl<'a, B: 'a> Ptr<'a, B> {
    pub fn key(&self) -> Key {
        self.key
    }

    pub fn store(&mut self) -> &mut Store<B> {
        &mut self.store
    }

    pub fn resolve(&mut self, key: Key) -> Ptr<B> {
        Ptr {
            key: key,
            store: self.store,
        }
    }

    pub fn into_mut(self) -> &'a mut Stream<B> {
        &mut self.store.slab[self.key.0]
    }
}

impl<'a, B: 'a> ops::Deref for Ptr<'a, B> {
    type Target = Stream<B>;

    fn deref(&self) -> &Stream<B> {
        &self.store.slab[self.key.0]
    }
}

impl<'a, B: 'a> ops::DerefMut for Ptr<'a, B> {
    fn deref_mut(&mut self) -> &mut Stream<B> {
        &mut self.store.slab[self.key.0]
    }
}

// ===== impl OccupiedEntry =====

impl<'a, B> OccupiedEntry<'a, B> {
    pub fn key(&self) -> Key {
        Key(*self.ids.get())
    }

    pub fn get(&self) -> &Stream<B> {
        &self.slab[*self.ids.get()]
    }

    pub fn get_mut(&mut self) -> &mut Stream<B> {
        &mut self.slab[*self.ids.get()]
    }

    pub fn into_mut(self) -> &'a mut Stream<B> {
        &mut self.slab[*self.ids.get()]
    }
}

// ===== impl VacantEntry =====

impl<'a, B> VacantEntry<'a, B> {
    pub fn insert(self, value: Stream<B>) -> Key {
        // Insert the value in the slab
        let key = self.slab.insert(value);

        // Insert the handle in the ID map
        self.ids.insert(key);

        Key(key)
    }
}