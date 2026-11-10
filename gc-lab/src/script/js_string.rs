//! ECMAScript String values (§6.1.4): sequences of UTF-16 code units, which may be
//! ill-formed (a lone surrogate is a legal element). Design: STRINGS.md.
//!
//! Canonical dual representation: `Utf8` when the code units are well-formed UTF-16,
//! `Units` exactly when they are not. Every constructor normalises, so two equal
//! strings always share a variant and the derived `Eq`/`Hash` are correct.
//!
//! `Utf8` caches its code-unit length and, on first indexed access, its code units, so
//! `length`, `charCodeAt` and `s[i]` are O(1) after one O(n) pass (STRINGS.md, perf
//! controls of literature check #335). A long concatenation is a `Rope` node, flattened
//! once on first read, so a `s += x` loop is linear; equality and hashing always look at
//! the flattened (canonical) form.
use super::vm::ScriptError;
use std::borrow::Cow;
use std::cell::{OnceCell, RefCell};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

#[derive(Clone)]
pub(super) struct JsString(Repr);

#[derive(Clone)]
enum Repr {
    Utf8(Rc<Utf8>),
    /// Invariant: contains at least one unpaired surrogate.
    Units(Rc<[u16]>),
    /// A lazy concatenation; `JsString::repr` never returns this variant.
    Rope(Rc<Rope>),
}

/// Equality and hashing over the canonical form: a flat `Utf8` (well-formed) or `Units`
/// (ill-formed) string, so equal code-unit sequences compare and hash equal.
impl PartialEq for JsString {
    fn eq(&self, other: &Self) -> bool {
        match (self.repr(), other.repr()) {
            (Repr::Utf8(a), Repr::Utf8(b)) => Rc::ptr_eq(a, b) || a.text == b.text,
            (Repr::Units(a), Repr::Units(b)) => a == b,
            _ => false,
        }
    }
}
impl Eq for JsString {}
impl Hash for JsString {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self.repr() {
            Repr::Utf8(u) => {
                0u8.hash(state);
                u.text.hash(state)
            }
            Repr::Units(units) => {
                1u8.hash(state);
                units.hash(state)
            }
            Repr::Rope(_) => unreachable!("repr() is flat"),
        }
    }
}

/// Concatenations below this storage size are copied eagerly.
const ROPE_MIN: usize = 256;

/// `left ++ right`, flattened on first read. Invariant: `parts` is `None` only once `flat`
/// is set, and `flat` never holds `Repr::Rope`.
struct Rope {
    parts: RefCell<Option<(JsString, JsString)>>,
    len_units: usize,
    size: usize,
    flat: OnceCell<Repr>,
}
impl Rope {
    fn flat(&self) -> &Repr {
        self.flat.get_or_init(|| {
            let (left, right) = self.parts.borrow_mut().take().expect("rope parts");
            // Explicit stack: a `+=` loop builds a chain as deep as its iteration count.
            let mut stack = vec![right, left];
            let mut out = Builder::default();
            while let Some(part) = stack.pop() {
                match &part.0 {
                    Repr::Rope(rope) => match rope.flat.get() {
                        Some(flat) => out.push_repr(flat),
                        None => {
                            let parts = rope.parts.borrow();
                            let (l, r) = parts.as_ref().expect("rope parts");
                            stack.push(r.clone());
                            stack.push(l.clone());
                        }
                    },
                    leaf => out.push_repr(leaf),
                }
            }
            out.finish().0
        })
    }
}
/// Iterative drop, for the same deep chains.
impl Drop for Rope {
    fn drop(&mut self) {
        let mut stack: Vec<JsString> = vec![];
        if let Some((l, r)) = self.parts.get_mut().take() {
            stack.extend([l, r]);
        }
        while let Some(part) = stack.pop() {
            if let Repr::Rope(rope) = part.0 {
                if let Ok(mut rope) = Rc::try_unwrap(rope) {
                    if let Some((l, r)) = rope.parts.get_mut().take() {
                        stack.extend([l, r]);
                    }
                }
            }
        }
    }
}

/// Well-formed text plus derived caches. Equality and hashing look at `text` only.
struct Utf8 {
    text: Box<str>,
    /// Number of UTF-16 code units; equals `text.len()` exactly when `text` is ASCII.
    len_units: usize,
    /// The code units, built on first use for non-ASCII text.
    wide: OnceCell<Box<[u16]>>,
}
impl Utf8 {
    fn new(text: Box<str>) -> Self {
        let len_units = if text.is_ascii() {
            text.len()
        } else {
            text.chars().map(char::len_utf16).sum()
        };
        Self {
            text,
            len_units,
            wide: OnceCell::new(),
        }
    }
    fn is_ascii(&self) -> bool {
        self.len_units == self.text.len()
    }
    fn wide(&self) -> &[u16] {
        self.wide.get_or_init(|| self.text.encode_utf16().collect())
    }
}
impl PartialEq for Utf8 {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
    }
}
impl Eq for Utf8 {}
impl Hash for Utf8 {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.text.hash(state)
    }
}

/// The error reported by code paths that still need scalar text.
pub(super) const UNPAIRED: &str = "unpaired UTF-16 string storage";

impl JsString {
    /// Builds a String from arbitrary code units, normalising to the canonical form.
    pub(super) fn from_units(units: &[u16]) -> Self {
        match String::from_utf16(units) {
            Ok(text) => Self::utf8(text.into_boxed_str()),
            Err(_) => Self::ill_formed(units),
        }
    }
    /// The only constructor of `Repr::Units`: the caller has checked that `units` is
    /// ill-formed, which is the canonical-form invariant behind the derived `Eq`/`Hash`.
    /// The canonical (flat) representation.
    fn repr(&self) -> &Repr {
        match &self.0 {
            Repr::Rope(rope) => rope.flat(),
            flat => flat,
        }
    }
    fn utf8(text: Box<str>) -> Self {
        Self(Repr::Utf8(Rc::new(Utf8::new(text))))
    }
    fn ill_formed(units: &[u16]) -> Self {
        debug_assert!(
            String::from_utf16(units).is_err(),
            "Repr::Units must hold an unpaired surrogate"
        );
        Self(Repr::Units(units.into()))
    }
    /// The scalar text, when the string is well-formed.
    pub(super) fn as_str(&self) -> Option<&str> {
        match self.repr() {
            Repr::Rope(_) => unreachable!("repr() is flat"),
            Repr::Utf8(u) => Some(&u.text),
            Repr::Units(_) => None,
        }
    }
    /// The scalar text, or Unsupported for sites that do not handle lone surrogates yet.
    pub(super) fn text(&self) -> Result<&str, ScriptError> {
        self.as_str().ok_or(ScriptError::Unsupported(UNPAIRED))
    }
    /// The scalar text before the first lone surrogate: exact for algorithms that
    /// stop at the first code point that is not whitespace or numeric syntax
    /// (parseInt §19.2.5, parseFloat §19.2.4), because a lone surrogate is neither.
    pub(super) fn scalar_prefix(&self) -> Cow<'_, str> {
        match self.repr() {
            Repr::Rope(_) => unreachable!("repr() is flat"),
            Repr::Utf8(u) => Cow::Borrowed(&u.text),
            Repr::Units(units) => Cow::Owned(
                char::decode_utf16(units.iter().copied())
                    .map_while(Result::ok)
                    .collect(),
            ),
        }
    }
    /// The string split into well-formed runs (`Ok`) and lone surrogate code units (`Err`).
    pub(super) fn scalar_runs(&self) -> Vec<Result<String, u16>> {
        let mut out = vec![];
        let mut run = String::new();
        for c in char::decode_utf16(self.units().iter().copied()) {
            match c {
                Ok(c) => run.push(c),
                Err(e) => {
                    if !run.is_empty() {
                        out.push(Ok(std::mem::take(&mut run)));
                    }
                    out.push(Err(e.unpaired_surrogate()));
                }
            }
        }
        if !run.is_empty() {
            out.push(Ok(run));
        }
        out
    }
    /// The code units (§6.1.4). Borrowed: the first call on `Utf8` text
    /// builds and caches them, later calls are free.
    pub(super) fn units(&self) -> Cow<'_, [u16]> {
        match self.repr() {
            Repr::Rope(_) => unreachable!("repr() is flat"),
            Repr::Utf8(u) => Cow::Borrowed(u.wide()),
            Repr::Units(units) => Cow::Borrowed(units),
        }
    }
    /// The length in code units, O(1).
    pub(super) fn len_units(&self) -> usize {
        match &self.0 {
            Repr::Rope(rope) => rope.len_units,
            Repr::Utf8(u) => u.len_units,
            Repr::Units(units) => units.len(),
        }
    }
    pub(super) fn is_empty(&self) -> bool {
        match &self.0 {
            Repr::Utf8(u) => u.text.is_empty(),
            Repr::Units(_) | Repr::Rope(_) => false,
        }
    }
    /// The code unit at `index`: O(1) for ASCII text, O(1) after the first call otherwise.
    pub(super) fn unit_at(&self, index: usize) -> Option<u16> {
        match self.repr() {
            Repr::Rope(_) => unreachable!("repr() is flat"),
            Repr::Utf8(u) if u.is_ascii() => u.text.as_bytes().get(index).map(|&b| b as u16),
            Repr::Utf8(u) => u.wide().get(index).copied(),
            Repr::Units(units) => units.get(index).copied(),
        }
    }
    /// IsStringWellFormedUnicode (§7.2.11 / §6.1.4.1).
    pub(super) fn is_well_formed(&self) -> bool {
        matches!(self.repr(), Repr::Utf8(_))
    }
    /// String.prototype.toWellFormed steps (§22.1.3.32): each lone surrogate becomes U+FFFD.
    pub(super) fn to_well_formed(&self) -> Self {
        match self.repr() {
            Repr::Rope(_) => unreachable!("repr() is flat"),
            Repr::Utf8(_) => self.clone(),
            Repr::Units(units) => Self::from(String::from_utf16_lossy(units)),
        }
    }
    /// String concatenation (§13.15.3 ApplyStringOrNumericBinaryOperator step 1.b).
    /// A lead surrogate at the end of `self` and a trail at the start of `other` pair up.
    pub(super) fn concat(&self, other: &Self) -> Self {
        if other.is_empty() {
            return self.clone();
        }
        if self.is_empty() {
            return other.clone();
        }
        let size = self.size() + other.size();
        if size >= ROPE_MIN {
            return Self(Repr::Rope(Rc::new(Rope {
                parts: RefCell::new(Some((self.clone(), other.clone()))),
                len_units: self.len_units() + other.len_units(),
                size,
                flat: OnceCell::new(),
            })));
        }
        match (self.repr(), other.repr()) {
            (Repr::Utf8(a), Repr::Utf8(b)) => {
                let mut text = String::with_capacity(a.text.len() + b.text.len());
                text.push_str(&a.text);
                text.push_str(&b.text);
                Self(Repr::Utf8(Rc::new(Utf8 {
                    text: text.into_boxed_str(),
                    len_units: a.len_units + b.len_units,
                    wide: OnceCell::new(),
                })))
            }
            _ => {
                let mut units = self.units().into_owned();
                units.extend_from_slice(&other.units());
                Self::from_units(&units)
            }
        }
    }
    /// Storage size (UTF-8 bytes or code units), for allocation guards only.
    pub(super) fn size(&self) -> usize {
        match &self.0 {
            Repr::Rope(rope) => rope.size,
            Repr::Utf8(u) => u.text.len(),
            Repr::Units(units) => units.len(),
        }
    }
    /// Lossy scalar text (U+FFFD for lone surrogates): diagnostics only, never JS-visible.
    pub(super) fn lossy(&self) -> Cow<'_, str> {
        match self.repr() {
            Repr::Rope(_) => unreachable!("repr() is flat"),
            Repr::Utf8(u) => Cow::Borrowed(&u.text),
            Repr::Units(units) => Cow::Owned(String::from_utf16_lossy(units)),
        }
    }
}

/// Linear-time concatenation of many parts (join, templates, String.raw). Stays UTF-8
/// until a part is ill-formed; `finish` normalises, so pairs split across parts join.
#[derive(Default)]
pub(super) struct Builder {
    text: String,
    units: Option<Vec<u16>>,
}
impl Builder {
    pub(super) fn push(&mut self, part: &JsString) {
        self.push_repr(part.repr())
    }
    fn push_repr(&mut self, part: &Repr) {
        match (&mut self.units, part) {
            (_, Repr::Rope(_)) => unreachable!("repr() is flat"),
            (None, Repr::Utf8(t)) => self.text.push_str(&t.text),
            (None, Repr::Units(u)) => {
                let mut units: Vec<u16> = self.text.encode_utf16().collect();
                units.extend_from_slice(u);
                self.text.clear();
                self.units = Some(units);
            }
            (Some(units), Repr::Utf8(t)) => units.extend_from_slice(t.wide()),
            (Some(units), Repr::Units(u)) => units.extend_from_slice(u),
        }
    }
    pub(super) fn push_str(&mut self, part: &str) {
        match &mut self.units {
            None => self.text.push_str(part),
            Some(units) => units.extend(part.encode_utf16()),
        }
    }
    /// Storage size so far, for allocation guards.
    pub(super) fn size(&self) -> usize {
        self.units.as_ref().map_or(self.text.len(), Vec::len)
    }
    pub(super) fn finish(self) -> JsString {
        match self.units {
            None => JsString::from(self.text),
            Some(units) => JsString::from_units(&units),
        }
    }
}

impl Default for JsString {
    fn default() -> Self {
        Self::from("")
    }
}
impl From<String> for JsString {
    fn from(text: String) -> Self {
        Self::utf8(text.into_boxed_str())
    }
}
impl From<&String> for JsString {
    fn from(text: &String) -> Self {
        Self::utf8(text.as_str().into())
    }
}
impl From<&str> for JsString {
    fn from(text: &str) -> Self {
        Self::utf8(text.into())
    }
}
impl From<&JsString> for JsString {
    fn from(text: &JsString) -> Self {
        text.clone()
    }
}
impl From<char> for JsString {
    fn from(c: char) -> Self {
        Self::from(c.to_string())
    }
}
impl From<Cow<'_, str>> for JsString {
    fn from(text: Cow<'_, str>) -> Self {
        Self::utf8(text.into())
    }
}
impl PartialEq<str> for JsString {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == Some(other)
    }
}
impl PartialEq<&str> for JsString {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == Some(*other)
    }
}
impl PartialEq<String> for JsString {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == Some(other.as_str())
    }
}
/// Lossy: diagnostics and error messages only.
impl fmt::Display for JsString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.lossy())
    }
}
impl fmt::Debug for JsString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.repr() {
            Repr::Rope(_) => unreachable!("repr() is flat"),
            Repr::Utf8(u) => fmt::Debug::fmt(&*u.text, f),
            Repr::Units(units) => write!(f, "JsString(units {units:x?})"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::JsString;
    #[test]
    fn canonical_forms() {
        let lead = JsString::from_units(&[0xD83D]);
        let trail = JsString::from_units(&[0xDE00]);
        assert!(!lead.is_well_formed());
        let pair = lead.concat(&trail);
        assert!(pair.is_well_formed());
        assert_eq!(pair, JsString::from("\u{1F600}"));
        assert_eq!(pair.len_units(), 2);
        assert_eq!(lead.to_well_formed(), JsString::from("\u{FFFD}"));
        assert_eq!(JsString::from_units(&[0x61, 0x62]), JsString::from("ab"));
        assert_eq!(lead.unit_at(0), Some(0xD83D));
        let mut b = super::Builder::default();
        b.push_str("a");
        b.push(&lead);
        b.push(&trail);
        assert_eq!(b.finish(), JsString::from("a\u{1F600}"));
    }

    /// Literature check #335: equality and hashing must agree with code-unit equality
    /// for every construction path (from_units, concat, Builder, slicing at any seam).
    #[test]
    fn equality_and_hash_follow_code_units() {
        use std::collections::hash_map::DefaultHasher;
        use std::collections::HashSet;
        use std::hash::{Hash, Hasher};
        fn hash(s: &JsString) -> u64 {
            let mut h = DefaultHasher::new();
            s.hash(&mut h);
            h.finish()
        }
        // xorshift64*: deterministic, no dependency.
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            state.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };
        const POOL: [u16; 8] = [0x61, 0xD83D, 0xDE00, 0xD800, 0xDFFF, 0xFFFF, 0xE9, 0x0];
        let mut seen: HashSet<JsString> = HashSet::new();
        let mut by_units: HashSet<Vec<u16>> = HashSet::new();
        for _ in 0..4000 {
            let len = (next() % 7) as usize;
            let units: Vec<u16> = (0..len).map(|_| POOL[(next() % 8) as usize]).collect();
            let whole = JsString::from_units(&units);
            assert_eq!(&*whole.units(), &units[..]);
            assert_eq!(whole.len_units(), units.len());
            for cut in 0..=units.len() {
                let (l, r) = units.split_at(cut);
                let (l, r) = (JsString::from_units(l), JsString::from_units(r));
                let joined = l.concat(&r);
                let mut b = super::Builder::default();
                b.push(&l);
                b.push(&r);
                let built = b.finish();
                for other in [&joined, &built] {
                    assert_eq!(other, &whole, "seam {cut} in {units:x?}");
                    assert_eq!(hash(other), hash(&whole));
                }
            }
            for (i, u) in units.iter().enumerate() {
                assert_eq!(whole.unit_at(i), Some(*u));
            }
            assert_eq!(seen.insert(whole), by_units.insert(units));
        }
        assert_eq!(seen.len(), by_units.len());
    }

    #[test]
    fn ropes_are_canonical_and_shallow_to_drop() {
        use std::collections::HashSet;
        let pad = JsString::from("x".repeat(super::ROPE_MIN));
        let lead = pad.concat(&JsString::from_units(&[0xD83D]));
        let trail = JsString::from_units(&[0xDE00]).concat(&pad);
        assert!(matches!(lead.0, super::Repr::Rope(_)));
        assert_eq!(lead.len_units(), super::ROPE_MIN + 1);
        let joined = lead.concat(&trail);
        assert_eq!(joined.len_units(), 2 * super::ROPE_MIN + 2);
        let flat = JsString::from(format!("{0}\u{1F600}{0}", "x".repeat(super::ROPE_MIN)));
        assert_eq!(joined, flat);
        assert!(joined.is_well_formed());
        let set: HashSet<JsString> = [joined.clone(), flat.clone()].into_iter().collect();
        assert_eq!(set.len(), 1);
        assert!(!lead.is_well_formed());
        // A `+=` chain one million nodes deep: flattening and dropping must not recurse.
        let mut s = pad.clone();
        let a = JsString::from("a");
        for _ in 0..1_000_000 {
            s = s.concat(&a);
        }
        assert_eq!(s.len_units(), super::ROPE_MIN + 1_000_000);
        assert_eq!(s.unit_at(super::ROPE_MIN + 999_999), Some(0x61));
        let mut unflattened = pad.clone();
        for _ in 0..1_000_000 {
            unflattened = a.concat(&unflattened);
        }
        drop(unflattened);
    }
}
