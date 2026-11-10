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

#[derive(Clone, Default)]
pub(super) struct PropertyMap {
    entries: Vec<Option<(Key, Property)>>,
    index: Option<HashMap<Key, usize, BuildKeyHasher>>,
    live: usize,
}

impl PropertyMap {
    fn position(&self, key: &Key) -> Option<usize> {
        match &self.index {
            Some(index) => index.get(key).copied(),
            None => self
                .entries
                .iter()
                .position(|e| e.as_ref().is_some_and(|(k, _)| k == key)),
        }
    }
    pub(super) fn get(&self, key: &Key) -> Option<&Property> {
        let at = self.position(key)?;
        self.entries[at].as_ref().map(|(_, p)| p)
    }
    pub(super) fn get_mut(&mut self, key: &Key) -> Option<&mut Property> {
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
    pub(super) fn push(&mut self, (key, property): (Key, Property)) {
        let at = self.entries.len();
        if let Some(index) = &mut self.index {
            index.insert(key.clone(), at);
        }
        self.entries.push(Some((key, property)));
        self.live += 1;
        if self.index.is_none() && self.live > INDEX_AT {
            self.reindex();
        }
    }
    pub(super) fn remove(&mut self, key: &Key) -> Option<Property> {
        let at = self.position(key)?;
        let (_, property) = self.entries[at].take()?;
        if let Some(index) = &mut self.index {
            index.remove(key);
        }
        self.live -= 1;
        if self.entries.len() > 2 * self.live + INDEX_AT {
            self.entries.retain(Option::is_some);
            self.reindex();
        }
        Some(property)
    }
    pub(super) fn retain(&mut self, mut keep: impl FnMut(&(Key, Property)) -> bool) {
        for slot in &mut self.entries {
            if slot.as_ref().is_some_and(|e| !keep(e)) {
                *slot = None;
            }
        }
        self.entries.retain(Option::is_some);
        self.live = self.entries.len();
        self.reindex();
    }
    fn reindex(&mut self) {
        self.index = (self.live > INDEX_AT).then(|| {
            self.entries
                .iter()
                .enumerate()
                .filter_map(|(at, e)| e.as_ref().map(|(k, _)| (k.clone(), at)))
                .collect()
        });
    }
    pub(super) fn len(&self) -> usize {
        self.live
    }
    pub(super) fn iter(&self) -> impl Iterator<Item = &(Key, Property)> {
        self.entries.iter().flatten()
    }
}

impl<'a> IntoIterator for &'a PropertyMap {
    type Item = &'a (Key, Property);
    type IntoIter = std::iter::Flatten<std::slice::Iter<'a, Option<(Key, Property)>>>;
    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter().flatten()
    }
}
impl<'a> IntoIterator for &'a mut PropertyMap {
    type Item = &'a mut (Key, Property);
    type IntoIter = std::iter::Flatten<std::slice::IterMut<'a, Option<(Key, Property)>>>;
    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter_mut().flatten()
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
    fn order_and_index() {
        let mut m = PropertyMap::default();
        for i in 0..20 {
            m.insert(Key::from(i.to_string()), p(i as f64));
        }
        m.insert(Key::from("3"), p(30.0));
        for i in (0..20).step_by(2) {
            assert!(m.remove(&Key::from(i.to_string())).is_some());
        }
        assert_eq!(m.len(), 10);
        let keys: Vec<_> = m.iter().map(|(k, _)| k.clone()).collect();
        let want: Vec<_> = (1..20)
            .step_by(2)
            .map(|i| Key::from(i.to_string()))
            .collect();
        assert_eq!(keys, want);
        assert_eq!(m.get(&Key::from("3")), Some(&p(30.0)));
        assert!(m.get(&Key::from("4")).is_none());
        m.retain(|(k, _)| *k != Key::from("5"));
        assert_eq!(m.len(), 9);
        m.insert(Key::from("4"), p(4.0));
        assert_eq!(
            m.iter().last().map(|(k, _)| k.clone()),
            Some(Key::from("4"))
        );
    }
}
