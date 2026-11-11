//! Own-property storage for ordinary objects (§10.1 [[OwnPropertyKeys]] order is
//! derived by the caller; this keeps insertion order). Design: PROPERTIES.md.
//!
//! Entries live in insertion order with tombstones for deleted keys; objects with
//! more than `INDEX_AT` live keys also keep a hash index, so lookup, insert and
//! delete are O(1) instead of a linear scan.
use super::objects::{Key, Property};
use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};

/// Hasher for property keys: a multiply-rotate word hash (the FxHash construction)
/// started from a per-process random seed. SipHash dominated property profiles
/// (BYTECODE_VM.md, stage 2 prelude). Keys come from page script, so the seed keeps
/// collisions from being precomputed offline; this is the same trade-off engines make
/// with their seeded string hashes, and the index degrades to O(n) probes, not
/// unsafety, under a successful attack.
#[derive(Clone, Copy)]
pub(super) struct KeyHasher(u64);
impl Hasher for KeyHasher {
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for chunk in &mut chunks {
            let mut word = [0u8; 8];
            word.copy_from_slice(chunk);
            self.write_u64(u64::from_le_bytes(word));
        }
        let rest = chunks.remainder();
        if !rest.is_empty() {
            let mut word = [0u8; 8];
            word[..rest.len()].copy_from_slice(rest);
            self.write_u64(u64::from_le_bytes(word) ^ ((rest.len() as u64) << 56));
        }
    }
    fn write_u8(&mut self, byte: u8) {
        self.write_u64(byte as u64);
    }
    fn write_u64(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
    fn write_usize(&mut self, word: usize) {
        self.write_u64(word as u64);
    }
    fn finish(&self) -> u64 {
        // Final avalanche so hashbrown's top-7-bit tags see the low words too.
        let mut h = self.0;
        h ^= h >> 33;
        h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
        h ^ (h >> 33)
    }
}
#[derive(Clone, Copy)]
pub(super) struct BuildKeyHasher(u64);
impl Default for BuildKeyHasher {
    fn default() -> Self {
        static SEED: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
        Self(*SEED.get_or_init(|| {
            std::collections::hash_map::RandomState::new()
                .build_hasher()
                .finish()
        }))
    }
}
impl BuildHasher for BuildKeyHasher {
    type Hasher = KeyHasher;
    fn build_hasher(&self) -> KeyHasher {
        KeyHasher(self.0)
    }
}

const INDEX_AT: usize = 8;

/// Hasher for the index's `u64` keys, which are already seeded, avalanched key hashes.
#[derive(Clone, Copy, Default)]
struct HashOnly(u64);
impl Hasher for HashOnly {
    fn write(&mut self, _: &[u8]) {
        unreachable!("the index hashes only u64 key hashes")
    }
    fn write_u64(&mut self, word: u64) {
        self.0 = word;
    }
    fn finish(&self) -> u64 {
        self.0
    }
}
#[derive(Clone, Copy, Default)]
struct BuildHashOnly;
impl BuildHasher for BuildHashOnly {
    type Hasher = HashOnly;
    fn build_hasher(&self) -> HashOnly {
        HashOnly(0)
    }
}

/// Entry positions sharing one key hash (almost always one). The index stores hashes,
/// not keys, so appending a key never clones it; lookups compare against `entries`.
#[derive(Clone)]
enum Bucket {
    One(usize),
    Many(Vec<usize>),
}

#[derive(Clone, Default)]
struct Index {
    keys: BuildKeyHasher,
    positions: HashMap<u64, Bucket, BuildHashOnly>,
}
impl Index {
    fn hash(&self, key: &Key) -> u64 {
        self.keys.hash_one(key)
    }
    fn add(&mut self, hash: u64, at: usize) {
        use std::collections::hash_map::Entry;
        match self.positions.entry(hash) {
            Entry::Vacant(slot) => {
                slot.insert(Bucket::One(at));
            }
            Entry::Occupied(mut slot) => match slot.get_mut() {
                Bucket::Many(all) => all.push(at),
                Bucket::One(first) => {
                    let first = *first;
                    slot.insert(Bucket::Many(vec![first, at]));
                }
            },
        }
    }
    fn drop_position(&mut self, hash: u64, at: usize) {
        let Some(bucket) = self.positions.get_mut(&hash) else {
            return;
        };
        match bucket {
            Bucket::One(_) => {
                self.positions.remove(&hash);
            }
            Bucket::Many(all) => {
                all.retain(|p| *p != at);
                if let [only] = all[..] {
                    *bucket = Bucket::One(only);
                }
            }
        }
    }
}

/// Bound on the holes a store may open past the end of `elements` (OBJECT_MODEL.md
/// slice 4): a new index at or past `2 * present + ELEMENT_SLACK` goes to the sparse map,
/// so holes never outnumber the present elements by more than the slack (a doubling
/// index sequence such as `x[2^k - 2]` must not grow the Vec exponentially).
const ELEMENT_SLACK: usize = 16;

type Entry = (Key, Property);

#[derive(Clone, Default)]
pub(super) struct PropertyMap {
    /// Non-element keys (and sparse array indices) in insertion order, with tombstones.
    entries: Vec<Option<Entry>>,
    index: Option<Index>,
    live: usize,
    /// Dense elements (OBJECT_MODEL.md slice 4): the property of array index `i` is
    /// `elements[i]`, `None` being a hole. Invariant: an index below `elements.len()`
    /// is never in `entries`, and `elements` only grows while no index is in
    /// `entries` (`sparse` is false), so an index lookup has exactly one place to look.
    /// The key is kept with the property so iteration hands out `&(Key, Property)`.
    elements: Vec<Option<Entry>>,
    element_count: usize,
    /// Some array index lives in `entries` ("dictionary elements"); it stays set.
    sparse: bool,
}

impl PropertyMap {
    /// The element slot of `key` when it is an array index below `elements.len()`.
    fn element(&self, key: &Key) -> Option<usize> {
        if self.elements.is_empty() {
            return None;
        }
        key.index()
            .map(|i| i as usize)
            .filter(|i| *i < self.elements.len())
    }
    fn position(&self, key: &Key) -> Option<usize> {
        let matches = |at: usize| self.entries[at].as_ref().is_some_and(|(k, _)| k == key);
        match &self.index {
            Some(index) => match index.positions.get(&index.hash(key))? {
                Bucket::One(at) => matches(*at).then_some(*at),
                Bucket::Many(all) => all.iter().copied().find(|at| matches(*at)),
            },
            None => self
                .entries
                .iter()
                .position(|e| e.as_ref().is_some_and(|(k, _)| k == key)),
        }
    }
    /// OBJECT_MODEL.md slice 3: the entry at `at` when its key is the string `name` (a
    /// slot hint is validated by key on every use, so a stale hint only costs a miss).
    pub(super) fn named_at(&self, at: usize, name: &str) -> Option<&Property> {
        match self.entries.get(at)? {
            Some((Key::String(k), p)) if k == name => Some(p),
            _ => None,
        }
    }
    pub(super) fn named_at_mut(&mut self, at: usize, name: &str) -> Option<&mut Property> {
        match self.entries.get_mut(at)? {
            Some((Key::String(k), p)) if k == name => Some(p),
            _ => None,
        }
    }
    /// The entry index of `key`, for a slot hint (named entries only).
    pub(super) fn index_of(&self, key: &Key) -> Option<usize> {
        if self.element(key).is_some() {
            return None;
        }
        self.position(key)
    }
    /// The property of array index `index` without building its key (slice 4 fast path).
    pub(super) fn get_index(&self, index: u32) -> Option<&Property> {
        match self.elements.get(index as usize) {
            Some(slot) => slot.as_ref().map(|(_, p)| p),
            None if self.sparse => self.get(&Key::String(index.to_string())),
            None => None,
        }
    }
    pub(super) fn get_index_mut(&mut self, index: u32) -> Option<&mut Property> {
        if (index as usize) < self.elements.len() {
            return self.elements[index as usize].as_mut().map(|(_, p)| p);
        }
        if self.sparse {
            return self.get_mut(&Key::String(index.to_string()));
        }
        None
    }
    pub(super) fn get(&self, key: &Key) -> Option<&Property> {
        if let Some(i) = self.element(key) {
            return self.elements[i].as_ref().map(|(_, p)| p);
        }
        let at = self.position(key)?;
        self.entries[at].as_ref().map(|(_, p)| p)
    }
    pub(super) fn get_mut(&mut self, key: &Key) -> Option<&mut Property> {
        if let Some(i) = self.element(key) {
            return self.elements[i].as_mut().map(|(_, p)| p);
        }
        let at = self.position(key)?;
        self.entries[at].as_mut().map(|(_, p)| p)
    }
    /// Replaces the value of an existing key in place, or appends a new key.
    pub(super) fn insert(&mut self, key: Key, property: Property) {
        if let Some(slot) = self.get_mut(&key) {
            *slot = property;
            return;
        }
        self.push((key, property));
    }
    /// Appends a key the caller knows is absent.
    pub(super) fn push(&mut self, (key, property): Entry) {
        if let Some(i) = key.index().map(|i| i as usize) {
            let len = self.elements.len();
            if i < len || !self.sparse && i < 2 * self.element_count + ELEMENT_SLACK {
                if i >= len {
                    self.elements.resize(i + 1, None);
                }
                debug_assert!(self.elements[i].is_none());
                self.elements[i] = Some((key, property));
                self.element_count += 1;
                return;
            }
            self.sparse = true;
        }
        let at = self.entries.len();
        if let Some(index) = &mut self.index {
            let hash = index.hash(&key);
            index.add(hash, at);
        }
        self.entries.push(Some((key, property)));
        self.live += 1;
        if self.index.is_none() && self.live > INDEX_AT {
            self.reindex();
        }
    }
    /// Drops trailing holes, so `elements.len()` is one past the highest element.
    fn trim(&mut self) {
        while matches!(self.elements.last(), Some(None)) {
            self.elements.pop();
        }
    }
    pub(super) fn remove(&mut self, key: &Key) -> Option<Property> {
        if let Some(i) = self.element(key) {
            let (_, property) = self.elements[i].take()?;
            self.element_count -= 1;
            self.trim();
            return Some(property);
        }
        let at = self.position(key)?;
        let (_, property) = self.entries[at].take()?;
        if let Some(index) = &mut self.index {
            let hash = index.hash(key);
            index.drop_position(hash, at);
        }
        self.live -= 1;
        if self.entries.len() > 2 * self.live + INDEX_AT {
            self.entries.retain(Option::is_some);
            self.reindex();
        }
        Some(property)
    }
    pub(super) fn retain(&mut self, mut keep: impl FnMut(&Entry) -> bool) {
        for slot in self.elements.iter_mut().chain(&mut self.entries) {
            if slot.as_ref().is_some_and(|e| !keep(e)) {
                *slot = None;
            }
        }
        self.element_count = self.elements.iter().flatten().count();
        self.trim();
        self.entries.retain(Option::is_some);
        self.live = self.entries.len();
        self.reindex();
    }
    fn reindex(&mut self) {
        self.index = (self.live > INDEX_AT).then(|| {
            let mut index = Index::default();
            index.positions.reserve(self.live);
            for (at, entry) in self.entries.iter().enumerate() {
                if let Some((k, _)) = entry {
                    let hash = index.hash(k);
                    index.add(hash, at);
                }
            }
            index
        });
    }
    pub(super) fn len(&self) -> usize {
        self.live + self.element_count
    }
    /// Dense elements in ascending index order, then the other keys in insertion order.
    pub(super) fn iter(&self) -> Iter<'_> {
        self.into_iter()
    }
}

type Iter<'a> = std::iter::Chain<
    std::iter::Flatten<std::slice::Iter<'a, Option<Entry>>>,
    std::iter::Flatten<std::slice::Iter<'a, Option<Entry>>>,
>;
impl<'a> IntoIterator for &'a PropertyMap {
    type Item = &'a Entry;
    type IntoIter = Iter<'a>;
    fn into_iter(self) -> Self::IntoIter {
        self.elements
            .iter()
            .flatten()
            .chain(self.entries.iter().flatten())
    }
}
impl<'a> IntoIterator for &'a mut PropertyMap {
    type Item = &'a mut Entry;
    type IntoIter = std::iter::Chain<
        std::iter::Flatten<std::slice::IterMut<'a, Option<Entry>>>,
        std::iter::Flatten<std::slice::IterMut<'a, Option<Entry>>>,
    >;
    fn into_iter(self) -> Self::IntoIter {
        self.elements
            .iter_mut()
            .flatten()
            .chain(self.entries.iter_mut().flatten())
    }
}

#[cfg(test)]
mod tests {
    use super::super::objects::{Key, Property};
    use super::super::vm::Value;
    use super::PropertyMap;
    fn p(n: f64) -> Property {
        Property::data(Value::number(n), true, true, true)
    }
    #[test]
    fn colliding_hashes_share_a_bucket() {
        let mut index = super::Index::default();
        index.add(7, 1);
        index.add(7, 4);
        index.add(7, 9);
        index.drop_position(7, 4);
        assert!(matches!(index.positions.get(&7), Some(super::Bucket::Many(v)) if v == &[1, 9]));
        index.drop_position(7, 1);
        assert!(matches!(
            index.positions.get(&7),
            Some(super::Bucket::One(9))
        ));
        index.drop_position(7, 9);
        assert!(index.positions.is_empty());
    }
    #[test]
    fn order_and_index() {
        let mut m = PropertyMap::default();
        for i in 0..20 {
            m.insert(Key::from(format!("k{i}")), p(i as f64));
        }
        m.insert(Key::from("k3"), p(30.0));
        for i in (0..20).step_by(2) {
            assert!(m.remove(&Key::from(format!("k{i}"))).is_some());
        }
        assert_eq!(m.len(), 10);
        let keys: Vec<_> = m.iter().map(|(k, _)| k.clone()).collect();
        let want: Vec<_> = (1..20)
            .step_by(2)
            .map(|i| Key::from(format!("k{i}")))
            .collect();
        assert_eq!(keys, want);
        assert_eq!(m.get(&Key::from("k3")), Some(&p(30.0)));
        assert!(m.get(&Key::from("k4")).is_none());
        m.retain(|(k, _)| *k != Key::from("k5"));
        assert_eq!(m.len(), 9);
        m.insert(Key::from("k4"), p(4.0));
        assert_eq!(
            m.iter().last().map(|(k, _)| k.clone()),
            Some(Key::from("k4"))
        );
    }
    /// OBJECT_MODEL.md slice 4: dense elements, holes, the sparse fallback and the
    /// "indices first, ascending" iteration order.
    #[test]
    fn dense_and_sparse_elements() {
        let mut m = PropertyMap::default();
        m.insert(Key::from("length"), p(0.0));
        for i in [2u32, 0, 1, 5] {
            m.insert(Key::from(i.to_string()), p(i as f64));
        }
        assert_eq!(m.elements.len(), 6);
        assert!(!m.sparse);
        assert_eq!(m.get_index(5), Some(&p(5.0)));
        assert!(m.get_index(3).is_none() && m.get(&Key::from("4")).is_none());
        assert_eq!(m.len(), 5);
        // Far past the end: dictionary elements, and the dense part stops growing.
        m.insert(Key::from("1000000"), p(1e6));
        assert!(m.sparse);
        m.insert(Key::from("6"), p(6.0));
        assert_eq!(m.elements.len(), 6);
        assert_eq!(m.get_index(6), Some(&p(6.0)));
        assert_eq!(m.get_index(1_000_000), Some(&p(1e6)));
        // Non-canonical index strings are named keys.
        m.insert(Key::from("01"), p(-1.0));
        assert_eq!(m.get(&Key::from("1")), Some(&p(1.0)));
        let keys: Vec<_> = m.iter().map(|(k, _)| k.clone()).collect();
        let want: Vec<_> = ["0", "1", "2", "5", "length", "1000000", "6", "01"]
            .into_iter()
            .map(Key::from)
            .collect();
        assert_eq!(keys, want);
        // Removing the last element trims the holes before it.
        assert!(m.remove(&Key::from("5")).is_some());
        assert_eq!(m.elements.len(), 3);
        assert_eq!(m.index_of(&Key::from("0")), None);
        m.retain(|(k, _)| k.index().is_none_or(|i| i < 1));
        assert_eq!(m.len(), 3);
        assert_eq!(m.elements.len(), 1);
        assert!(m.get_index(6).is_none());
    }
}
