//! String operations count UTF-16 code units; scalar-only storage limits are explicit.
use super::js_string::JsString;
use super::{
    objects::{Descriptor, Key},
    vm::{Engine, ScriptError, Value},
};
use crate::dual::Id;
use std::collections::HashMap;
type Result<T> = std::result::Result<T, ScriptError>;
/// %StringIteratorPrototype% state: the code units and the next position (§22.1.5.1).
#[derive(Clone)]
pub(super) struct Cursor {
    pub units: std::rc::Rc<[u16]>,
    pub pos: usize,
}
pub(super) const METHODS: &[&str] = &[
    "at",
    "charAt",
    "charCodeAt",
    "codePointAt",
    "concat",
    "endsWith",
    "includes",
    "indexOf",
    "lastIndexOf",
    "padEnd",
    "padStart",
    "repeat",
    "replace",
    "replaceAll",
    "match",
    "matchAll",
    "search",
    "split",
    "slice",
    "startsWith",
    "substring",
    "toLowerCase",
    "toUpperCase",
    "toLocaleLowerCase",
    "toLocaleUpperCase",
    "localeCompare",
    "isWellFormed",
    "toWellFormed",
    "trim",
    "trimEnd",
    "trimStart",
];
/// A String value from arbitrary code units (lone surrogates allowed, §6.1.4).
pub(super) fn string(units: &[u16]) -> Result<Value> {
    Ok(Value::String(JsString::from_units(units)))
}
/// CodePointAt (§11.1.4): the code point at `pos` and its length in code units.
pub(super) fn code_point_at(units: &[u16], pos: usize) -> (u32, usize) {
    let a = units[pos];
    if (0xd800..=0xdbff).contains(&a) {
        if let Some(&b) = units.get(pos + 1) {
            if (0xdc00..=0xdfff).contains(&b) {
                return (
                    0x10000 + ((a as u32 - 0xd800) << 10) + (b as u32 - 0xdc00),
                    2,
                );
            }
        }
    }
    (a as u32, 1)
}
/// Applies `map` to each maximal well-formed run; lone surrogates are copied unchanged.
/// Used by case mapping (§22.1.3.28/.30, StringToCodePoints keeps lone surrogates):
/// a surrogate is neither cased nor case-ignorable, so it also ends any Final_Sigma
/// context, and mapping the runs separately equals mapping the whole string.
fn map_runs(units: &[u16], map: impl Fn(&str) -> String) -> Vec<u16> {
    let mut out = Vec::with_capacity(units.len());
    let mut run = String::new();
    for c in char::decode_utf16(units.iter().copied()) {
        match c {
            Ok(c) => run.push(c),
            Err(e) => {
                out.extend(map(&run).encode_utf16());
                run.clear();
                out.push(e.unpaired_surrogate());
            }
        }
    }
    out.extend(map(&run).encode_utf16());
    out
}
pub(super) fn whitespace(c: char) -> bool {
    matches!(c,'\u{9}'..='\u{d}'|'\u{20}'|'\u{a0}'|'\u{1680}'|'\u{2000}'..='\u{200a}'|'\u{2028}'|'\u{2029}'|'\u{202f}'|'\u{205f}'|'\u{3000}'|'\u{feff}')
}
impl Engine<'_> {
    pub(super) fn bootstrap_strings(
        &mut self,
        env: Id,
        objects: &mut HashMap<String, Id>,
        iterator: Id,
    ) -> Result<()> {
        let function = self.intrinsics.as_ref().unwrap().function;
        for method in METHODS {
            let name = format!("String.prototype.{method}");
            let f = self.intrinsic_function(env, &name, Some(function))?;
            self.intrinsics.as_mut().unwrap().functions.insert(name, f);
            self.hidden(objects["String.prototype"], *method, Value::Object(f))?;
        }
        for method in ["fromCharCode", "fromCodePoint", "raw"] {
            let name = format!("String.{method}");
            let f = self.intrinsic_function(env, &name, Some(function))?;
            self.intrinsics.as_mut().unwrap().functions.insert(name, f);
            self.hidden(objects["String"], method, Value::Object(f))?;
        }
        let method = self.intrinsic_function(env, "String.prototype.iterator", Some(function))?;
        self.hidden(
            objects["String.prototype"],
            Key::Symbol(0),
            Value::Object(method),
        )?;
        let proto = self.new_object(Some(iterator))?;
        self.runtime.link(env, proto)?;
        self.intrinsics
            .as_mut()
            .unwrap()
            .collection
            .insert("StringIterator".into(), proto);
        let next =
            self.intrinsic_function(env, "%StringIteratorPrototype%.next", Some(function))?;
        self.hidden(proto, "next", Value::Object(next))?;
        self.define(
            proto,
            Key::Symbol(1),
            Descriptor {
                value: Some(Value::String("String Iterator".into())),
                configurable: Some(true),
                ..Default::default()
            },
        )?;
        Ok(())
    }
    pub(super) fn string_builtin(
        &mut self,
        name: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        if name == "String.raw" {
            return self.string_raw(args);
        }
        if matches!(
            name,
            "String.prototype.replace" | "String.prototype.replaceAll" | "String.prototype.split"
        ) {
            return self.string_search_builtin(name, receiver, args);
        }
        if matches!(
            name,
            "String.prototype.match" | "String.prototype.matchAll" | "String.prototype.search"
        ) {
            return self.string_regexp_method(name, receiver, args);
        }
        let arg = |i| args.get(i).cloned().unwrap_or(Value::Undefined);
        if name == "%StringIteratorPrototype%.next" {
            let Value::Object(id) = receiver else {
                return self.throw_error("TypeError", "invalid string iterator receiver");
            };
            let Some(mut cursor) = self.data(id).ok().and_then(|d| d.string_cursor.clone()) else {
                return self.throw_error("TypeError", "invalid string iterator receiver");
            };
            // §22.1.5.1: yield CodePointAt(s, position).[[CodeUnitCount]] units at a time.
            let next = if cursor.pos < cursor.units.len() {
                let (_, n) = code_point_at(&cursor.units, cursor.pos);
                let part = JsString::from_units(&cursor.units[cursor.pos..cursor.pos + n]);
                cursor.pos += n;
                Some(part)
            } else {
                cursor.units = std::rc::Rc::from([]);
                cursor.pos = 0;
                None
            };
            let done = next.is_none();
            let mut data = self.data(id)?.clone();
            data.string_cursor = Some(cursor);
            self.replace_data(id, data)?;
            return self.iterator_result(next.map(Value::String).unwrap_or(Value::Undefined), done);
        }
        if name == "String.fromCharCode" {
            let mut units = vec![];
            for v in args {
                units.push(super::numeric::uint32(self.to_number(v)?) as u16);
            }
            return string(&units);
        }
        if name == "String.fromCodePoint" {
            let mut units = vec![];
            for v in args {
                let n = self.to_number(v)?;
                if !n.is_finite() || n.trunc() != n || !(0.0..=1114111.0).contains(&n) {
                    return self.throw_error("RangeError", "invalid code point");
                }
                let code = n as u32;
                if code <= 0xffff {
                    units.push(code as u16);
                } else {
                    let code = code - 0x10000;
                    units.push((0xd800 + (code >> 10)) as u16);
                    units.push((0xdc00 + (code & 0x3ff)) as u16);
                }
            }
            return string(&units);
        }
        if matches!(receiver, Value::Null | Value::Undefined) {
            return self.throw_error("TypeError", "null or undefined string receiver");
        }
        let text = self.to_js_string(receiver)?;
        if name == "String.prototype.iterator" {
            let proto = self.intrinsics.as_ref().unwrap().collection["StringIterator"];
            let id = self.new_object(Some(proto))?;
            self.protect(&Value::Object(id))?;
            let mut data = self.data(id)?.clone();
            data.string_cursor = Some(Cursor {
                units: text.units().into(),
                pos: 0,
            });
            self.replace_data(id, data)?;
            return Ok(Value::Object(id));
        }
        let units: Vec<u16> = text.units().into_owned();
        let len = units.len() as u64;
        let method = name.strip_prefix("String.prototype.").unwrap_or(name);
        match method {
            "charAt" | "at" | "charCodeAt" | "codePointAt" => {
                let mut pos = self.integer(arg(0))?;
                if method == "at" && pos < 0.0 {
                    pos += len as f64;
                }
                if pos < 0.0 || pos >= len as f64 || !pos.is_finite() {
                    return Ok(match method {
                        "charAt" => Value::String(String::new().into()),
                        "charCodeAt" => Value::number(f64::NAN),
                        _ => Value::Undefined,
                    });
                }
                let pos = pos as usize;
                let a = units[pos];
                if method == "charCodeAt" {
                    return Ok(Value::number(a as f64));
                }
                if method == "codePointAt" {
                    return Ok(Value::number(code_point_at(&units, pos).0 as f64));
                }
                string(&units[pos..pos + 1])
            }
            "concat" => {
                let mut out = text;
                for v in args {
                    out = out.concat(&self.to_js_string(v)?);
                }
                Ok(Value::String(out))
            }
            "slice" | "substring" => {
                let (mut start, mut end) = if method == "slice" {
                    (
                        self.relative_index(arg(0), len)?,
                        if arg(1) == Value::Undefined {
                            len
                        } else {
                            self.relative_index(arg(1), len)?
                        },
                    )
                } else {
                    (
                        self.integer(arg(0))?.clamp(0.0, len as f64) as u64,
                        if arg(1) == Value::Undefined {
                            len
                        } else {
                            self.integer(arg(1))?.clamp(0.0, len as f64) as u64
                        },
                    )
                };
                if method == "substring" && start > end {
                    std::mem::swap(&mut start, &mut end);
                }
                string(&units[start as usize..end.max(start) as usize])
            }
            "indexOf" | "lastIndexOf" | "includes" | "startsWith" | "endsWith" => {
                if matches!(method, "includes" | "startsWith" | "endsWith")
                    && self.string_is_regexp(arg(0))?
                {
                    return self
                        .throw_error("TypeError", "regular expression search is not allowed");
                }
                let search: Vec<u16> = self.to_js_string(arg(0))?.units().into_owned();
                let n = if method == "lastIndexOf" {
                    let n = self.to_number(arg(1))?;
                    if n.is_nan() {
                        len as f64
                    } else {
                        n.trunc()
                    }
                } else if method == "endsWith" && arg(1) == Value::Undefined {
                    len as f64
                } else {
                    self.integer(arg(1))?
                };
                let pos = n.clamp(0.0, len as f64) as usize;
                let at = |i: usize| {
                    units
                        .get(i..i.saturating_add(search.len()))
                        .is_some_and(|part| part == search.as_slice())
                };
                if method == "startsWith" {
                    return Ok(Value::Bool(at(pos)));
                }
                if method == "endsWith" {
                    return Ok(Value::Bool(search.len() <= pos && at(pos - search.len())));
                }
                let found = if method == "lastIndexOf" {
                    (0..=pos).rev().find(|i| at(*i))
                } else {
                    (pos..=len as usize).find(|i| at(*i))
                };
                Ok(if method == "includes" {
                    Value::Bool(found.is_some())
                } else {
                    Value::number(found.map(|i| i as f64).unwrap_or(-1.0))
                })
            }
            "repeat" => {
                let n = self.integer(arg(0))?;
                if n < 0.0 || !n.is_finite() {
                    return self.throw_error("RangeError", "invalid string repeat count");
                }
                if text.is_empty() {
                    return Ok(Value::String(text));
                }
                if n > 16777216.0
                    || units
                        .len()
                        .checked_mul(n as usize)
                        .is_none_or(|n| n > 16777216)
                {
                    return self.throw_error("RangeError", "host string allocation limit");
                }
                Ok(Value::String(match text.as_str() {
                    Some(t) => t.repeat(n as usize).into(),
                    None => JsString::from_units(&units.repeat(n as usize)),
                }))
            }
            "padStart" | "padEnd" => {
                let length = self.integer(arg(0))?.clamp(0.0, 9007199254740991.0) as u64;
                if length <= len {
                    return Ok(Value::String(text));
                }
                let pad = if arg(1) == Value::Undefined {
                    " ".into()
                } else {
                    self.to_js_string(arg(1))?
                };
                if pad.is_empty() {
                    return Ok(Value::String(text));
                }
                if length > 16777216 {
                    return self.throw_error("RangeError", "host string allocation limit");
                }
                let pad: Vec<u16> = pad.units().into_owned();
                let fill: Vec<u16> = pad
                    .iter()
                    .copied()
                    .cycle()
                    .take((length - len) as usize)
                    .collect();
                let result = if method == "padStart" {
                    fill.into_iter().chain(units).collect::<Vec<_>>()
                } else {
                    units.into_iter().chain(fill).collect::<Vec<_>>()
                };
                string(&result)
            }
            // §22.1.3.28/.30: full Unicode default case mapping. Without ECMA-402 the
            // locale variants (§22.1.3.26/.27) use the root locale, i.e. the same mapping.
            "toLowerCase" | "toLocaleLowerCase" => match text.as_str() {
                Some(t) => Ok(Value::String(t.to_lowercase().into())),
                None => string(&map_runs(&units, str::to_lowercase)),
            },
            "toUpperCase" | "toLocaleUpperCase" => match text.as_str() {
                Some(t) => Ok(Value::String(t.to_uppercase().into())),
                None => string(&map_runs(&units, str::to_uppercase)),
            },
            // §22.1.3.12: implementation-defined order; here code-unit order, the
            // same order as IsLessThan on strings. Canonical equivalence (step 5's
            // requirement) needs Unicode normalization data, not available yet.
            "localeCompare" => {
                let that: Vec<u16> = self.to_js_string(arg(0))?.units().into_owned();
                Ok(Value::number(match units.cmp(&that) {
                    std::cmp::Ordering::Less => -1.0,
                    std::cmp::Ordering::Equal => 0.0,
                    std::cmp::Ordering::Greater => 1.0,
                }))
            }
            // §22.1.3.10 isWellFormed, §22.1.3.32 toWellFormed.
            "isWellFormed" => Ok(Value::Bool(text.is_well_formed())),
            "toWellFormed" => Ok(Value::String(text.to_well_formed())),
            // §22.1.3.35.1 TrimString: WhiteSpace and LineTerminator are BMP scalar
            // values, so trimming code units never splits a pair or drops a surrogate.
            "trim" | "trimStart" | "trimEnd" => {
                let white = |u: &u16| char::from_u32(*u as u32).is_some_and(whitespace);
                let start = if method == "trimEnd" {
                    0
                } else {
                    units.iter().position(|u| !white(u)).unwrap_or(units.len())
                };
                let end = if method == "trimStart" {
                    units.len()
                } else {
                    units
                        .iter()
                        .rposition(|u| !white(u))
                        .map_or(start, |i| i + 1)
                };
                string(&units[start..end.max(start)])
            }
            _ => Err(ScriptError::Unsupported("string intrinsic")),
        }
    }
}
