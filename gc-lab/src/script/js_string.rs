//! ECMAScript String values (§6.1.4): sequences of UTF-16 code units, which may be
//! ill-formed (a lone surrogate is a legal element). Design: STRINGS.md.
//!
//! Canonical dual representation: `Utf8` when the code units are well-formed UTF-16,
//! `Units` exactly when they are not. Every constructor normalises, so two equal
//! strings always share a variant and the derived `Eq`/`Hash` are correct.
use super::vm::ScriptError;
use std::borrow::Cow;
use std::fmt;
use std::rc::Rc;

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct JsString(Repr);

#[derive(Clone, PartialEq, Eq, Hash)]
enum Repr {
    Utf8(Rc<str>),
    /// Invariant: contains at least one unpaired surrogate.
    Units(Rc<[u16]>),
}

/// The error reported by code paths that still need scalar text.
pub(super) const UNPAIRED: &str = "unpaired UTF-16 string storage";

impl JsString {
    /// Builds a String from arbitrary code units, normalising to the canonical form.
    pub(super) fn from_units(units: &[u16]) -> Self {
        match String::from_utf16(units) {
            Ok(text) => Self(Repr::Utf8(text.into())),
            Err(_) => Self(Repr::Units(units.into())),
        }
    }
    /// The scalar text, when the string is well-formed.
    pub(super) fn as_str(&self) -> Option<&str> {
        match &self.0 {
            Repr::Utf8(text) => Some(text),
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
        match &self.0 {
            Repr::Utf8(text) => Cow::Borrowed(text),
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
    /// The code units (§6.1.4).
    pub(super) fn units(&self) -> Cow<'_, [u16]> {
        match &self.0 {
            Repr::Utf8(text) => Cow::Owned(text.encode_utf16().collect()),
            Repr::Units(units) => Cow::Borrowed(units),
        }
    }
    /// The length in code units.
    pub(super) fn len_units(&self) -> usize {
        match &self.0 {
            Repr::Utf8(text) => text.encode_utf16().count(),
            Repr::Units(units) => units.len(),
        }
    }
    pub(super) fn is_empty(&self) -> bool {
        match &self.0 {
            Repr::Utf8(text) => text.is_empty(),
            Repr::Units(_) => false,
        }
    }
    pub(super) fn unit_at(&self, index: usize) -> Option<u16> {
        match &self.0 {
            Repr::Utf8(text) => text.encode_utf16().nth(index),
            Repr::Units(units) => units.get(index).copied(),
        }
    }
    /// IsStringWellFormedUnicode (§7.2.11 / §6.1.4.1).
    pub(super) fn is_well_formed(&self) -> bool {
        matches!(self.0, Repr::Utf8(_))
    }
    /// String.prototype.toWellFormed steps (§22.1.3.32): each lone surrogate becomes U+FFFD.
    pub(super) fn to_well_formed(&self) -> Self {
        match &self.0 {
            Repr::Utf8(_) => self.clone(),
            Repr::Units(units) => Self::from(String::from_utf16_lossy(units)),
        }
    }
    /// String concatenation (§13.15.3 ApplyStringOrNumericBinaryOperator step 1.b).
    /// A lead surrogate at the end of `self` and a trail at the start of `other` pair up.
    pub(super) fn concat(&self, other: &Self) -> Self {
        match (&self.0, &other.0) {
            (Repr::Utf8(a), Repr::Utf8(b)) => {
                let mut text = String::with_capacity(a.len() + b.len());
                text.push_str(a);
                text.push_str(b);
                Self(Repr::Utf8(text.into()))
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
            Repr::Utf8(text) => text.len(),
            Repr::Units(units) => units.len(),
        }
    }
    /// Lossy scalar text (U+FFFD for lone surrogates): diagnostics only, never JS-visible.
    pub(super) fn lossy(&self) -> Cow<'_, str> {
        match &self.0 {
            Repr::Utf8(text) => Cow::Borrowed(text),
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
        match (&mut self.units, &part.0) {
            (None, Repr::Utf8(t)) => self.text.push_str(t),
            (None, Repr::Units(u)) => {
                let mut units: Vec<u16> = self.text.encode_utf16().collect();
                units.extend_from_slice(u);
                self.text.clear();
                self.units = Some(units);
            }
            (Some(units), Repr::Utf8(t)) => units.extend(t.encode_utf16()),
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
        Self(Repr::Utf8(text.into()))
    }
}
impl From<&String> for JsString {
    fn from(text: &String) -> Self {
        Self(Repr::Utf8(text.as_str().into()))
    }
}
impl From<&str> for JsString {
    fn from(text: &str) -> Self {
        Self(Repr::Utf8(text.into()))
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
        Self(Repr::Utf8(text.into()))
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
        match &self.0 {
            Repr::Utf8(text) => fmt::Debug::fmt(&**text, f),
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
}
