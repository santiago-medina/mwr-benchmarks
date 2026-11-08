//! String operations count UTF-16 code units; scalar-only storage limits are explicit.
use super::{
    objects::{Descriptor, Key},
    vm::{Engine, ScriptError, Value},
};
use crate::dual::Id;
use std::collections::HashMap;
type Result<T> = std::result::Result<T, ScriptError>;
#[derive(Clone)]
pub(super) struct Cursor {
    pub text: String,
    pub byte: usize,
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
    "slice",
    "startsWith",
    "substring",
    "toLowerCase",
    "toUpperCase",
    "trim",
    "trimEnd",
    "trimStart",
];
fn string(units: &[u16]) -> Result<Value> {
    String::from_utf16(units)
        .map(Value::String)
        .map_err(|_| ScriptError::Unsupported("unpaired UTF-16 string storage"))
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
        for method in ["fromCharCode", "fromCodePoint"] {
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
        let arg = |i| args.get(i).cloned().unwrap_or(Value::Undefined);
        if name == "%StringIteratorPrototype%.next" {
            let Value::Object(id) = receiver else {
                return self.throw_error("TypeError", "invalid string iterator receiver");
            };
            let Some(mut cursor) = self.data(id).ok().and_then(|d| d.string_cursor.clone()) else {
                return self.throw_error("TypeError", "invalid string iterator receiver");
            };
            let next = cursor.text[cursor.byte..].chars().next();
            if let Some(c) = next {
                cursor.byte += c.len_utf8();
            } else {
                cursor.text.clear();
                cursor.byte = 0;
            }
            let mut data = self.data(id)?.clone();
            data.string_cursor = Some(cursor);
            self.replace_data(id, data)?;
            return self.iterator_result(
                next.map(|c| Value::String(c.to_string()))
                    .unwrap_or(Value::Undefined),
                next.is_none(),
            );
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
        let text = self.to_string(receiver)?;
        if name == "String.prototype.iterator" {
            let proto = self.intrinsics.as_ref().unwrap().collection["StringIterator"];
            let id = self.new_object(Some(proto))?;
            self.protect(&Value::Object(id))?;
            let mut data = self.data(id)?.clone();
            data.string_cursor = Some(Cursor { text, byte: 0 });
            self.replace_data(id, data)?;
            return Ok(Value::Object(id));
        }
        let units: Vec<u16> = text.encode_utf16().collect();
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
                        "charAt" => Value::String(String::new()),
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
                    let n = if (0xd800..=0xdbff).contains(&a)
                        && units
                            .get(pos + 1)
                            .is_some_and(|b| (0xdc00..=0xdfff).contains(b))
                    {
                        0x10000 + ((a as u32 - 0xd800) << 10) + (units[pos + 1] as u32 - 0xdc00)
                    } else {
                        a as u32
                    };
                    return Ok(Value::number(n as f64));
                }
                string(&units[pos..pos + 1])
            }
            "concat" => {
                let mut out = text;
                for v in args {
                    out.push_str(&self.to_string(v)?);
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
                let search: Vec<u16> = self.to_string(arg(0))?.encode_utf16().collect();
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
                    || text
                        .len()
                        .checked_mul(n as usize)
                        .is_none_or(|n| n > 16777216)
                {
                    return self.throw_error("RangeError", "host string allocation limit");
                }
                Ok(Value::String(text.repeat(n as usize)))
            }
            "padStart" | "padEnd" => {
                let length = self.integer(arg(0))?.clamp(0.0, 9007199254740991.0) as u64;
                if length <= len {
                    return Ok(Value::String(text));
                }
                let pad = if arg(1) == Value::Undefined {
                    " ".into()
                } else {
                    self.to_string(arg(1))?
                };
                if pad.is_empty() {
                    return Ok(Value::String(text));
                }
                if length > 16777216 {
                    return self.throw_error("RangeError", "host string allocation limit");
                }
                let pad: Vec<u16> = pad.encode_utf16().collect();
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
            "toLowerCase" => Ok(Value::String(text.to_lowercase())),
            "toUpperCase" => Ok(Value::String(text.to_uppercase())),
            "trim" => Ok(Value::String(text.trim_matches(whitespace).into())),
            "trimStart" => Ok(Value::String(text.trim_start_matches(whitespace).into())),
            "trimEnd" => Ok(Value::String(text.trim_end_matches(whitespace).into())),
            _ => Err(ScriptError::Unsupported("string intrinsic")),
        }
    }
}
