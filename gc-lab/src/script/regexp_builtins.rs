//! RegExp objects over the pure matcher in `regexp.rs`: constructor, prototype
//! methods, the Symbol protocol methods, %RegExpStringIterator% and the String
//! methods that delegate to them. Strings are converted to UTF-16 per call.
use super::js_string::JsString;
use super::{
    objects::{Descriptor, Key},
    regexp::{self, Flags, Program},
    vm::{Engine, ScriptError, Value},
};
use crate::dual::Id;
use std::{cell::RefCell, collections::HashMap, rc::Rc};
type Result<T> = std::result::Result<T, ScriptError>;

const MATCH: u64 = 6;
const MATCH_ALL: u64 = 7;
const REPLACE: u64 = 8;
const SEARCH: u64 = 9;
const SPLIT: u64 = 10;
const MAX_UNITS: usize = 16_777_216;

/// The [[OriginalSource]], [[OriginalFlags]] and [[RegExpMatcher]] slots.
pub(super) struct Matcher {
    pub source: String,
    pub flags: Flags,
    pub program: Rc<Program>,
}
/// %RegExpStringIterator% state.
#[derive(Clone)]
pub(super) struct Cursor {
    pub regexp: Id,
    pub text: JsString,
    pub global: bool,
    pub unicode: bool,
    pub done: bool,
}

thread_local! {
    static CACHE: RefCell<HashMap<(String, String), Rc<Program>>> = RefCell::new(HashMap::new());
}
/// Compiles with a small per-thread cache keyed by pattern and flags.
pub(super) fn compile_cached(
    pattern: &str,
    flags: Flags,
) -> std::result::Result<Rc<Program>, String> {
    let key = (pattern.to_string(), flags.text());
    if let Some(p) = CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return Ok(p);
    }
    let program = Rc::new(regexp::compile(pattern, flags)?);
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() > 4096 {
            c.clear();
        }
        c.insert(key, program.clone());
    });
    Ok(program)
}
/// Early-error check for a RegExp literal.
pub(super) fn literal_ok(pattern: &str, flags: &str) -> bool {
    Flags::parse(flags).is_some_and(|f| compile_cached(pattern, f).is_ok())
}
fn units(text: &JsString) -> Vec<u16> {
    text.units().into_owned()
}
fn slice(u: &[u16], a: usize, b: usize) -> Result<Value> {
    super::strings::string(&u[a.min(u.len())..b.min(u.len()).max(a.min(u.len()))])
}
fn push(out: &mut Vec<u16>, part: &[u16]) -> Result<()> {
    if out.len().saturating_add(part.len()) > MAX_UNITS {
        return Err(ScriptError::Runtime("string result limit"));
    }
    out.extend_from_slice(part);
    Ok(())
}
/// EscapeRegExpPattern for the `source` getter.
fn escape_source(source: &str) -> String {
    if source.is_empty() {
        return "(?:)".into();
    }
    let mut out = String::new();
    let mut class = false;
    let mut chars = source.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                out.push('\\');
                if let Some(n) = chars.next() {
                    match n {
                        '\n' => out.push('n'),
                        '\r' => out.push('r'),
                        '\u{2028}' => out.push_str("u2028"),
                        '\u{2029}' => out.push_str("u2029"),
                        _ => out.push(n),
                    }
                }
                continue;
            }
            '/' if !class => out.push_str("\\/"),
            '[' => {
                class = true;
                out.push(c)
            }
            ']' => {
                class = false;
                out.push(c)
            }
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            _ => out.push(c),
        }
    }
    out
}
/// RegExp.escape.
fn escape(text: &JsString) -> String {
    let mut out = String::new();
    for (i, c) in char::decode_utf16(text.units().iter().copied()).enumerate() {
        // §22.2.5.1.1 EncodeForRegExpEscape step 5: a lone surrogate is a UnicodeEscape.
        let c = match c {
            Ok(c) => c,
            Err(e) => {
                out.push_str(&format!("\\u{:04x}", e.unpaired_surrogate()));
                continue;
            }
        };
        if i == 0 && c.is_ascii_alphanumeric() {
            out.push_str(&format!("\\x{:02x}", c as u32));
            continue;
        }
        if "^$\\.*+?()[]{}|/".contains(c) {
            out.push('\\');
            out.push(c);
            continue;
        }
        let control = match c {
            '\t' => Some('t'),
            '\n' => Some('n'),
            '\u{b}' => Some('v'),
            '\u{c}' => Some('f'),
            '\r' => Some('r'),
            _ => None,
        };
        if let Some(e) = control {
            out.push('\\');
            out.push(e);
            continue;
        }
        if ",-=<>#&!%:;@~'`\"".contains(c) || super::strings::whitespace(c) {
            if (c as u32) <= 0xff {
                out.push_str(&format!("\\x{:02x}", c as u32));
            } else {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{:04x}", unit));
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}
const FLAG_GETTERS: &[(&str, char)] = &[
    ("hasIndices", 'd'),
    ("global", 'g'),
    ("ignoreCase", 'i'),
    ("multiline", 'm'),
    ("dotAll", 's'),
    ("unicode", 'u'),
    ("unicodeSets", 'v'),
    ("sticky", 'y'),
];
impl Engine<'_> {
    fn regexp_slot(&self, value: &Value) -> Option<Rc<Matcher>> {
        match value {
            Value::Object(id) => self.data(*id).ok().and_then(|d| d.regexp.clone()),
            _ => None,
        }
    }
    fn regexp_prototype(&self) -> Id {
        self.intrinsics.as_ref().unwrap().collection["RegExp"]
    }
    fn regexp_constructor(&self) -> Value {
        Value::Object(self.intrinsics.as_ref().unwrap().functions["RegExp"])
    }
    fn to_length(&mut self, value: Value) -> Result<u64> {
        let n = self.integer(value)?;
        Ok(n.clamp(0.0, 9007199254740991.0) as u64)
    }
    fn create_data(&mut self, id: Id, key: impl Into<Key>, value: Value) -> Result<()> {
        self.define(
            id,
            key,
            Descriptor {
                value: Some(value),
                writable: Some(true),
                enumerable: Some(true),
                configurable: Some(true),
                ..Descriptor::default()
            },
        )?;
        Ok(())
    }
    /// IsRegExp.
    pub(super) fn is_regexp(&mut self, value: Value) -> Result<bool> {
        if !matches!(value, Value::Object(_)) {
            return Ok(false);
        }
        let marker = self.get_member(value.clone(), Key::Symbol(MATCH))?;
        if marker != Value::Undefined {
            return Ok(marker.truthy());
        }
        Ok(self.regexp_slot(&value).is_some())
    }
    fn regexp_alloc(&mut self, new_target: Value) -> Result<Id> {
        let prototype = self.get_member(new_target, "prototype")?;
        let prototype = match prototype {
            Value::Object(id) => id,
            _ => self.regexp_prototype(),
        };
        let id = self.new_object(Some(prototype))?;
        self.protect(&Value::Object(id))?;
        self.define(
            id,
            "lastIndex",
            Descriptor {
                value: Some(Value::Undefined),
                writable: Some(true),
                ..Descriptor::default()
            },
        )?;
        Ok(id)
    }
    fn regexp_initialize(&mut self, id: Id, pattern: Value, flags: Value) -> Result<Value> {
        let source = if pattern == Value::Undefined {
            String::new()
        } else {
            self.to_string(pattern)?
        };
        let flag_text = if flags == Value::Undefined {
            String::new()
        } else {
            self.to_string(flags)?
        };
        let Some(parsed) = Flags::parse(&flag_text) else {
            return self.throw_error("SyntaxError", "Invalid regular expression flags");
        };
        let program = match compile_cached(&source, parsed) {
            Ok(p) => p,
            Err(message) => {
                return self.throw_error(
                    "SyntaxError",
                    &format!("Invalid regular expression: /{source}/{flag_text}: {message}"),
                )
            }
        };
        let mut data = self.data(id)?.clone();
        data.regexp = Some(Rc::new(Matcher {
            source,
            flags: parsed,
            program,
        }));
        self.replace_data(id, data)?;
        self.set_member(Value::Object(id), "lastIndex", Value::number(0.0))?;
        Ok(Value::Object(id))
    }
    /// RegExpCreate with %RegExp% as the constructor.
    fn regexp_create(&mut self, pattern: Value, flags: Value) -> Result<Value> {
        let target = self.regexp_constructor();
        let id = self.regexp_alloc(target)?;
        self.regexp_initialize(id, pattern, flags)
    }
    /// Evaluates a RegExp literal (already validated by the parser).
    pub(super) fn regexp_literal(&mut self, pattern: &str, flags: &str) -> Result<Value> {
        let proto = self.regexp_prototype();
        let id = self.new_object(Some(proto))?;
        self.protect(&Value::Object(id))?;
        self.define(
            id,
            "lastIndex",
            Descriptor {
                value: Some(Value::number(0.0)),
                writable: Some(true),
                ..Descriptor::default()
            },
        )?;
        let parsed = Flags::parse(flags).ok_or(ScriptError::Runtime("regexp literal flags"))?;
        let program = match compile_cached(pattern, parsed) {
            Ok(p) => p,
            Err(_) => return self.throw_error("SyntaxError", "Invalid regular expression"),
        };
        let mut data = self.data(id)?.clone();
        data.regexp = Some(Rc::new(Matcher {
            source: pattern.into(),
            flags: parsed,
            program,
        }));
        self.replace_data(id, data)?;
        Ok(Value::Object(id))
    }
    /// The RegExp constructor; `called` is true for a plain call.
    pub(super) fn construct_regexp(
        &mut self,
        new_target: Value,
        args: Vec<Value>,
        called: bool,
    ) -> Result<Value> {
        let pattern = args.first().cloned().unwrap_or(Value::Undefined);
        let flags = args.get(1).cloned().unwrap_or(Value::Undefined);
        self.protect(&pattern)?;
        self.protect(&flags)?;
        let pattern_is_regexp = self.is_regexp(pattern.clone())?;
        if called && pattern_is_regexp && flags == Value::Undefined {
            let constructor = self.get_member(pattern.clone(), "constructor")?;
            if constructor.same_value(&new_target) {
                return Ok(pattern);
            }
        }
        let (p, f) = if let Some(slot) = self.regexp_slot(&pattern) {
            (
                Value::String(slot.source.clone().into()),
                if flags == Value::Undefined {
                    Value::String(slot.flags.text().into())
                } else {
                    flags
                },
            )
        } else if pattern_is_regexp {
            let source = self.get_member(pattern.clone(), "source")?;
            self.protect(&source)?;
            let f = if flags == Value::Undefined {
                let f = self.get_member(pattern.clone(), "flags")?;
                self.protect(&f)?;
                f
            } else {
                flags
            };
            (source, f)
        } else {
            (pattern, flags)
        };
        let id = self.regexp_alloc(new_target)?;
        self.regexp_initialize(id, p, f)
    }
    /// RegExpBuiltinExec.
    fn regexp_builtin_exec(&mut self, id: Id, text: &JsString) -> Result<Value> {
        let r = Value::Object(id);
        let slot = self
            .regexp_slot(&r)
            .ok_or(ScriptError::Runtime("missing regexp slot"))?;
        let last = self.get_member(r.clone(), "lastIndex")?;
        let mut last_index = self.to_length(last)? as usize;
        let flags = slot.flags;
        let (global, sticky) = (flags.g, flags.y);
        if !global && !sticky {
            last_index = 0;
        }
        let input = units(text);
        if last_index > input.len() {
            if global || sticky {
                self.set_member(r, "lastIndex", Value::number(0.0))?;
            }
            return Ok(Value::Null);
        }
        self.tick()?;
        let found = slot
            .program
            .exec(&input, last_index, sticky)
            .map_err(|_| ScriptError::Runtime("regular expression step limit"))?;
        let Some(caps) = found else {
            if global || sticky {
                self.set_member(r, "lastIndex", Value::number(0.0))?;
            }
            return Ok(Value::Null);
        };
        let (start, end) = caps[0].unwrap();
        if global || sticky {
            self.set_member(r, "lastIndex", Value::number(end as f64))?;
        }
        let array = self.new_array(0)?;
        self.protect(&Value::Object(array))?;
        self.create_data(array, "index", Value::number(start as f64))?;
        self.create_data(array, "input", Value::String(text.clone()))?;
        let mut values = vec![];
        for cap in &caps {
            values.push(match cap {
                Some((a, b)) => slice(&input, *a, *b)?,
                None => Value::Undefined,
            });
        }
        self.create_data(array, "0", values[0].clone())?;
        let names = &slot.program.names;
        let groups = if names.is_empty() {
            Value::Undefined
        } else {
            let g = self.new_object(None)?;
            self.protect(&Value::Object(g))?;
            Value::Object(g)
        };
        self.create_data(array, "groups", groups.clone())?;
        for (i, v) in values.iter().enumerate().skip(1) {
            self.create_data(array, i.to_string(), v.clone())?;
        }
        if let Value::Object(g) = groups {
            for (name, indices) in names {
                let v = indices
                    .iter()
                    .find(|i| caps[**i].is_some())
                    .map(|i| values[*i].clone())
                    .unwrap_or(Value::Undefined);
                self.create_data(g, name.as_str(), v)?;
            }
        }
        if flags.d {
            let indices = self.new_array(0)?;
            self.protect(&Value::Object(indices))?;
            let mut pairs = vec![];
            for (i, cap) in caps.iter().enumerate() {
                let v = match cap {
                    Some((a, b)) => {
                        let pair = self.new_array(0)?;
                        self.protect(&Value::Object(pair))?;
                        self.create_data(pair, "0", Value::number(*a as f64))?;
                        self.create_data(pair, "1", Value::number(*b as f64))?;
                        Value::Object(pair)
                    }
                    None => Value::Undefined,
                };
                self.create_data(indices, i.to_string(), v.clone())?;
                pairs.push(v);
            }
            let groups = if names.is_empty() {
                Value::Undefined
            } else {
                let g = self.new_object(None)?;
                self.protect(&Value::Object(g))?;
                for (name, list) in names {
                    let v = list
                        .iter()
                        .find(|i| caps[**i].is_some())
                        .map(|i| pairs[*i].clone())
                        .unwrap_or(Value::Undefined);
                    self.create_data(g, name.as_str(), v)?;
                }
                Value::Object(g)
            };
            self.create_data(indices, "groups", groups)?;
            self.create_data(array, "indices", Value::Object(indices))?;
        }
        Ok(Value::Object(array))
    }
    /// RegExpExec.
    fn regexp_exec(&mut self, r: Value, text: &JsString) -> Result<Value> {
        let exec = self.get_member(r.clone(), "exec")?;
        self.protect(&exec)?;
        if self.callable(&exec) {
            let result = self.invoke_this(exec, r, vec![Value::String(text.into())])?;
            if !matches!(result, Value::Object(_) | Value::Null) {
                return self.throw_error("TypeError", "exec result must be an object or null");
            }
            self.protect(&result)?;
            return Ok(result);
        }
        let Value::Object(id) = r else {
            return self.throw_error("TypeError", "receiver is not a RegExp");
        };
        if self.regexp_slot(&r).is_none() {
            return self.throw_error("TypeError", "receiver is not a RegExp");
        }
        self.regexp_builtin_exec(id, text)
    }
    fn species_constructor(&mut self, object: Value, default: Value) -> Result<Value> {
        let c = self.get_member(object, "constructor")?;
        if c == Value::Undefined {
            return Ok(default);
        }
        if !matches!(c, Value::Object(_)) {
            return self.throw_error("TypeError", "constructor is not an object");
        }
        let s = self.get_member(c, Key::Symbol(2))?;
        if matches!(s, Value::Undefined | Value::Null) {
            return Ok(default);
        }
        if self.constructible(&s) {
            self.protect(&s)?;
            return Ok(s);
        }
        self.throw_error("TypeError", "species is not a constructor")
    }
    fn require_object(&mut self, value: &Value) -> Result<()> {
        if matches!(value, Value::Object(_)) {
            Ok(())
        } else {
            self.throw_error("TypeError", "RegExp method called on a non-object")
        }
    }
    fn flags_of(&mut self, r: Value) -> Result<String> {
        let f = self.get_member(r, "flags")?;
        self.to_string(f)
    }
    fn advance_last_index(&mut self, r: Value, text: &[u16], unicode: bool) -> Result<()> {
        let last = self.get_member(r.clone(), "lastIndex")?;
        let this_index = self.to_length(last)?;
        let next = if (this_index as usize) < text.len() {
            regexp::advance(text, this_index as usize, unicode) as f64
        } else {
            this_index as f64 + 1.0
        };
        self.set_member(r, "lastIndex", Value::number(next))
    }
    /// GetSubstitution (ES2023 form).
    #[allow(clippy::too_many_arguments)]
    fn get_substitution(
        &mut self,
        matched: &[u16],
        text: &[u16],
        position: usize,
        captures: &[Value],
        named: &Value,
        template: &[u16],
        out: &mut Vec<u16>,
    ) -> Result<()> {
        let dollar = b'$' as u16;
        let digit = |u: u16| (b'0' as u16..=b'9' as u16).contains(&u);
        let mut i = 0;
        while i < template.len() {
            self.tick()?;
            let c = template[i];
            let next = template.get(i + 1).copied();
            if c != dollar || next.is_none() {
                push(out, &template[i..i + 1])?;
                i += 1;
                continue;
            }
            let n = next.unwrap();
            if n == dollar {
                push(out, &[dollar])?;
                i += 2;
            } else if n == b'`' as u16 {
                push(out, &text[..position.min(text.len())])?;
                i += 2;
            } else if n == b'&' as u16 {
                push(out, matched)?;
                i += 2;
            } else if n == b'\'' as u16 {
                let tail = (position + matched.len()).min(text.len());
                push(out, &text[tail..])?;
                i += 2;
            } else if digit(n) {
                let two = template.get(i + 2).is_some_and(|u| digit(*u));
                let mut count = if two { 2 } else { 1 };
                let mut index = (n - b'0' as u16) as usize;
                if two {
                    index = index * 10 + (template[i + 2] - b'0' as u16) as usize;
                    if index > captures.len() {
                        count = 1;
                        index = (n - b'0' as u16) as usize;
                    }
                }
                if (1..=captures.len()).contains(&index) {
                    let cap = captures[index - 1].clone();
                    if cap != Value::Undefined {
                        let s = self.to_js_string(cap)?;
                        push(out, &units(&s))?;
                    }
                } else {
                    push(out, &template[i..i + 1 + count])?;
                }
                i += 1 + count;
            } else if n == b'<' as u16 {
                let close = template[i..].iter().position(|u| *u == b'>' as u16);
                match (close, named) {
                    (Some(gt), Value::Object(_)) => {
                        let name = super::strings::string(&template[i + 2..i + gt])?;
                        let name = self.to_string(name)?;
                        let cap = self.get_member(named.clone(), name.as_str())?;
                        if cap != Value::Undefined {
                            let s = self.to_js_string(cap)?;
                            push(out, &units(&s))?;
                        }
                        i += gt + 1;
                    }
                    _ => {
                        push(out, &template[i..i + 2])?;
                        i += 2;
                    }
                }
            } else {
                push(out, &[dollar])?;
                i += 1;
            }
        }
        Ok(())
    }
    pub(super) fn bootstrap_regexp(
        &mut self,
        env: Id,
        objects: &mut HashMap<String, Id>,
        iterator: Id,
    ) -> Result<()> {
        let i = self.intrinsics.as_ref().unwrap();
        let function = i.function;
        let object = i.object;
        let proto = self.new_object(Some(object))?;
        self.runtime.link(env, proto)?;
        objects.insert("RegExp.prototype".into(), proto);
        self.intrinsics
            .as_mut()
            .unwrap()
            .collection
            .insert("RegExp".into(), proto);
        let ctor = objects["RegExp"];
        let named = |engine: &mut Self, f: Id, name: &str, length: usize| -> Result<()> {
            engine.define(
                f,
                "length",
                Descriptor {
                    value: Some(Value::number(length as f64)),
                    ..Default::default()
                },
            )?;
            engine.define(
                f,
                "name",
                Descriptor {
                    value: Some(Value::String(name.into())),
                    configurable: Some(true),
                    ..Default::default()
                },
            )?;
            Ok(())
        };
        named(self, ctor, "RegExp", 2)?;
        let escape = self.intrinsic_function(env, "RegExp.escape", Some(function))?;
        named(self, escape, "escape", 1)?;
        self.hidden(ctor, "escape", Value::Object(escape))?;
        let species = self.intrinsic_function(env, "RegExp.species", Some(function))?;
        named(self, species, "get [Symbol.species]", 0)?;
        self.define(
            ctor,
            Key::Symbol(2),
            Descriptor {
                get: Some(Value::Object(species)),
                configurable: Some(true),
                ..Default::default()
            },
        )?;
        for (method, length) in [("exec", 1), ("test", 1), ("toString", 0), ("compile", 2)] {
            let f = self.intrinsic_function(
                env,
                &format!("RegExp.prototype.{method}"),
                Some(function),
            )?;
            named(self, f, method, length)?;
            self.hidden(proto, method, Value::Object(f))?;
        }
        for (symbol, method, length) in [
            (MATCH, "match", 1),
            (MATCH_ALL, "matchAll", 1),
            (REPLACE, "replace", 2),
            (SEARCH, "search", 1),
            (SPLIT, "split", 2),
        ] {
            let f = self.intrinsic_function(
                env,
                &format!("RegExp.prototype.@@{method}"),
                Some(function),
            )?;
            named(self, f, &format!("[Symbol.{method}]"), length)?;
            self.hidden(proto, Key::Symbol(symbol), Value::Object(f))?;
        }
        let getters = ["flags", "source"]
            .into_iter()
            .chain(FLAG_GETTERS.iter().map(|(n, _)| *n));
        for getter in getters {
            let f = self.intrinsic_function(
                env,
                &format!("RegExp.prototype.get.{getter}"),
                Some(function),
            )?;
            named(self, f, &format!("get {getter}"), 0)?;
            self.define(
                proto,
                getter,
                Descriptor {
                    get: Some(Value::Object(f)),
                    configurable: Some(true),
                    ..Default::default()
                },
            )?;
        }
        let it = self.new_object(Some(iterator))?;
        self.runtime.link(env, it)?;
        self.intrinsics
            .as_mut()
            .unwrap()
            .collection
            .insert("RegExpStringIterator".into(), it);
        let next =
            self.intrinsic_function(env, "%RegExpStringIteratorPrototype%.next", Some(function))?;
        named(self, next, "next", 0)?;
        self.hidden(it, "next", Value::Object(next))?;
        self.define(
            it,
            Key::Symbol(1),
            Descriptor {
                value: Some(Value::String("RegExp String Iterator".into())),
                configurable: Some(true),
                ..Default::default()
            },
        )?;
        Ok(())
    }
    pub(super) fn regexp_builtin(
        &mut self,
        name: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Undefined);
        for a in &args {
            self.protect(a)?;
        }
        self.protect(&receiver)?;
        match name {
            "RegExp" => {
                let target = self.regexp_constructor();
                return self.construct_regexp(target, args, true);
            }
            "RegExp.species" => return Ok(receiver),
            "RegExp.escape" => {
                let Value::String(text) = arg(0) else {
                    return self.throw_error("TypeError", "RegExp.escape requires a string");
                };
                return Ok(Value::String(escape(&text).into()));
            }
            "%RegExpStringIteratorPrototype%.next" => return self.regexp_iterator_next(receiver),
            _ => {}
        }
        if let Some(getter) = name.strip_prefix("RegExp.prototype.get.") {
            self.require_object(&receiver)?;
            if getter == "flags" {
                let mut out = String::new();
                for (property, c) in FLAG_GETTERS {
                    let v = self.get_member(receiver.clone(), *property)?;
                    if v.truthy() {
                        out.push(*c);
                    }
                }
                return Ok(Value::String(out.into()));
            }
            let slot = self.regexp_slot(&receiver);
            let Some(slot) = slot else {
                if receiver == Value::Object(self.regexp_prototype()) {
                    return Ok(if getter == "source" {
                        Value::String("(?:)".into())
                    } else {
                        Value::Undefined
                    });
                }
                return self.throw_error("TypeError", "receiver is not a RegExp");
            };
            if getter == "source" {
                return Ok(Value::String(escape_source(&slot.source).into()));
            }
            let c = FLAG_GETTERS.iter().find(|(n, _)| *n == getter).unwrap().1;
            return Ok(Value::Bool(slot.flags.text().contains(c)));
        }
        let method = name.strip_prefix("RegExp.prototype.").unwrap_or(name);
        match method {
            "exec" => {
                let Value::Object(id) = receiver else {
                    return self.throw_error("TypeError", "receiver is not a RegExp");
                };
                if self.regexp_slot(&receiver).is_none() {
                    return self.throw_error("TypeError", "receiver is not a RegExp");
                }
                let text = self.to_js_string(arg(0))?;
                self.regexp_builtin_exec(id, &text)
            }
            "test" => {
                self.require_object(&receiver)?;
                let text = self.to_js_string(arg(0))?;
                let m = self.regexp_exec(receiver, &text)?;
                Ok(Value::Bool(m != Value::Null))
            }
            "toString" => {
                self.require_object(&receiver)?;
                let source = self.get_member(receiver.clone(), "source")?;
                let source = self.to_string(source)?;
                let flags = self.flags_of(receiver)?;
                Ok(Value::String((format!("/{source}/{flags}")).into()))
            }
            "compile" => {
                let Value::Object(id) = receiver else {
                    return self.throw_error("TypeError", "receiver is not a RegExp");
                };
                if self.regexp_slot(&receiver).is_none() {
                    return self.throw_error("TypeError", "receiver is not a RegExp");
                }
                let (p, f) = if let Some(slot) = self.regexp_slot(&arg(0)) {
                    if arg(1) != Value::Undefined {
                        return self.throw_error("TypeError", "flags must be undefined");
                    }
                    (
                        Value::String(slot.source.clone().into()),
                        Value::String(slot.flags.text().into()),
                    )
                } else {
                    (arg(0), arg(1))
                };
                self.regexp_initialize(id, p, f)
            }
            "@@match" => {
                self.require_object(&receiver)?;
                let text = self.to_js_string(arg(0))?;
                let flags = self.flags_of(receiver.clone())?;
                if !flags.contains('g') {
                    return self.regexp_exec(receiver, &text);
                }
                let unicode = flags.contains('u') || flags.contains('v');
                let input = units(&text);
                self.set_member(receiver.clone(), "lastIndex", Value::number(0.0))?;
                let array = self.new_array(0)?;
                self.protect(&Value::Object(array))?;
                let mut n = 0u64;
                loop {
                    self.tick()?;
                    let result = self.regexp_exec(receiver.clone(), &text)?;
                    if result == Value::Null {
                        return Ok(if n == 0 {
                            Value::Null
                        } else {
                            Value::Object(array)
                        });
                    }
                    let m = self.get_member(result, "0")?;
                    let m = self.to_js_string(m)?;
                    let empty = m.is_empty();
                    self.create_data(array, n.to_string(), Value::String(m.into()))?;
                    if empty {
                        self.advance_last_index(receiver.clone(), &input, unicode)?;
                    }
                    n += 1;
                }
            }
            "@@matchAll" => {
                self.require_object(&receiver)?;
                let text = self.to_js_string(arg(0))?;
                let default = self.regexp_constructor();
                let c = self.species_constructor(receiver.clone(), default)?;
                let flags = self.flags_of(receiver.clone())?;
                let matcher = self.construct(
                    c,
                    vec![receiver.clone(), Value::String(flags.clone().into())],
                )?;
                self.protect(&matcher)?;
                let last = self.get_member(receiver, "lastIndex")?;
                let last = self.to_length(last)?;
                self.set_member(matcher.clone(), "lastIndex", Value::number(last as f64))?;
                let Value::Object(regexp) = matcher else {
                    return self.throw_error("TypeError", "matcher is not an object");
                };
                let proto = self.intrinsics.as_ref().unwrap().collection["RegExpStringIterator"];
                let id = self.new_object(Some(proto))?;
                self.protect(&Value::Object(id))?;
                let mut data = self.data(id)?.clone();
                data.regexp_cursor = Some(Cursor {
                    regexp,
                    text,
                    global: flags.contains('g'),
                    unicode: flags.contains('u') || flags.contains('v'),
                    done: false,
                });
                self.replace_data(id, data)?;
                Ok(Value::Object(id))
            }
            "@@search" => {
                self.require_object(&receiver)?;
                let text = self.to_js_string(arg(0))?;
                let previous = self.get_member(receiver.clone(), "lastIndex")?;
                self.protect(&previous)?;
                if !previous.same_value(&Value::number(0.0)) {
                    self.set_member(receiver.clone(), "lastIndex", Value::number(0.0))?;
                }
                let result = self.regexp_exec(receiver.clone(), &text)?;
                let current = self.get_member(receiver.clone(), "lastIndex")?;
                if !current.same_value(&previous) {
                    self.set_member(receiver, "lastIndex", previous)?;
                }
                if result == Value::Null {
                    return Ok(Value::number(-1.0));
                }
                self.get_member(result, "index")
            }
            "@@split" => self.regexp_split(receiver, arg(0), arg(1)),
            "@@replace" => self.regexp_replace(receiver, arg(0), arg(1)),
            _ => Err(ScriptError::Unsupported("RegExp method")),
        }
    }
    fn regexp_iterator_next(&mut self, receiver: Value) -> Result<Value> {
        let Value::Object(id) = receiver else {
            return self.throw_error("TypeError", "invalid RegExp String Iterator");
        };
        let Some(cursor) = self.data(id).ok().and_then(|d| d.regexp_cursor.clone()) else {
            return self.throw_error("TypeError", "invalid RegExp String Iterator");
        };
        if cursor.done {
            return self.iterator_result(Value::Undefined, true);
        }
        let r = Value::Object(cursor.regexp);
        let m = self.regexp_exec(r.clone(), &cursor.text)?;
        let finish = |engine: &mut Self| -> Result<()> {
            let mut data = engine.data(id)?.clone();
            if let Some(c) = data.regexp_cursor.as_mut() {
                c.done = true;
            }
            engine.replace_data(id, data)
        };
        if m == Value::Null {
            finish(self)?;
            return self.iterator_result(Value::Undefined, true);
        }
        if cursor.global {
            let s = self.get_member(m.clone(), "0")?;
            let s = self.to_js_string(s)?;
            if s.is_empty() {
                let input = units(&cursor.text);
                self.advance_last_index(r, &input, cursor.unicode)?;
            }
            return self.iterator_result(m, false);
        }
        finish(self)?;
        self.iterator_result(m, false)
    }
    fn regexp_split(&mut self, rx: Value, string: Value, limit: Value) -> Result<Value> {
        self.require_object(&rx)?;
        let text = self.to_js_string(string)?;
        let default = self.regexp_constructor();
        let c = self.species_constructor(rx.clone(), default)?;
        let flags = self.flags_of(rx.clone())?;
        let unicode = flags.contains('u') || flags.contains('v');
        let new_flags = if flags.contains('y') {
            flags
        } else {
            format!("{flags}y")
        };
        let splitter = self.construct(c, vec![rx, Value::String(new_flags.into())])?;
        self.protect(&splitter)?;
        let array = self.new_array(0)?;
        self.protect(&Value::Object(array))?;
        let lim = if limit == Value::Undefined {
            u32::MAX
        } else {
            super::numeric::uint32(self.to_number(limit)?)
        } as u64;
        if lim == 0 {
            return Ok(Value::Object(array));
        }
        let input = units(&text);
        let size = input.len();
        if size == 0 {
            let z = self.regexp_exec(splitter, &text)?;
            if z == Value::Null {
                self.create_data(array, "0", Value::String(text.into()))?;
            }
            return Ok(Value::Object(array));
        }
        let mut length = 0u64;
        let (mut p, mut q) = (0usize, 0usize);
        while q < size {
            self.tick()?;
            self.set_member(splitter.clone(), "lastIndex", Value::number(q as f64))?;
            let z = self.regexp_exec(splitter.clone(), &text)?;
            if z == Value::Null {
                q = regexp::advance(&input, q, unicode);
                continue;
            }
            let last = self.get_member(splitter.clone(), "lastIndex")?;
            let e = (self.to_length(last)? as usize).min(size);
            if e == p {
                q = regexp::advance(&input, q, unicode);
                continue;
            }
            let t = slice(&input, p, q)?;
            self.create_data(array, length.to_string(), t)?;
            length += 1;
            if length == lim {
                return Ok(Value::Object(array));
            }
            p = e;
            let Value::Object(zid) = z else {
                unreachable!()
            };
            let captures = self.length_of(zid)?.saturating_sub(1);
            for i in 1..=captures {
                let cap = self.get_member(z.clone(), i.to_string())?;
                self.create_data(array, length.to_string(), cap)?;
                length += 1;
                if length == lim {
                    return Ok(Value::Object(array));
                }
            }
            q = p;
        }
        let t = slice(&input, p, size)?;
        self.create_data(array, length.to_string(), t)?;
        Ok(Value::Object(array))
    }
    fn regexp_replace(&mut self, rx: Value, string: Value, replace: Value) -> Result<Value> {
        self.require_object(&rx)?;
        let text = self.to_js_string(string)?;
        let input = units(&text);
        let functional = self.callable(&replace);
        let template = if functional {
            vec![]
        } else {
            units(&self.to_js_string(replace.clone())?)
        };
        let flags = self.flags_of(rx.clone())?;
        let global = flags.contains('g');
        let unicode = flags.contains('u') || flags.contains('v');
        if global {
            self.set_member(rx.clone(), "lastIndex", Value::number(0.0))?;
        }
        let mut results = vec![];
        loop {
            self.tick()?;
            let result = self.regexp_exec(rx.clone(), &text)?;
            if result == Value::Null {
                break;
            }
            results.push(result.clone());
            if !global {
                break;
            }
            let m = self.get_member(result, "0")?;
            if self.to_js_string(m)?.is_empty() {
                self.advance_last_index(rx.clone(), &input, unicode)?;
            }
        }
        let mut out = vec![];
        let mut next = 0usize;
        for result in results {
            let Value::Object(rid) = result else {
                unreachable!()
            };
            let count = self.length_of(rid)?.saturating_sub(1);
            let matched = self.get_member(result.clone(), "0")?;
            let matched = units(&self.to_js_string(matched)?);
            let position = self.get_member(result.clone(), "index")?;
            let position = self.integer(position)?.clamp(0.0, input.len() as f64) as usize;
            let mut captures = vec![];
            for n in 1..=count {
                let cap = self.get_member(result.clone(), n.to_string())?;
                let cap = if cap == Value::Undefined {
                    cap
                } else {
                    Value::String((self.to_js_string(cap)?).into())
                };
                captures.push(cap);
            }
            let named = self.get_member(result.clone(), "groups")?;
            self.protect(&named)?;
            let mut replacement = vec![];
            if functional {
                let mut call = vec![super::strings::string(&matched)?];
                call.extend(captures);
                call.push(Value::number(position as f64));
                call.push(Value::String(text.clone().into()));
                if named != Value::Undefined {
                    call.push(named);
                }
                let v = self.invoke_this(replace.clone(), Value::Undefined, call)?;
                replacement = units(&self.to_js_string(v)?);
            } else {
                let named = if named == Value::Undefined {
                    named
                } else {
                    Value::Object(self.object_coercible(named)?)
                };
                self.get_substitution(
                    &matched,
                    &input,
                    position,
                    &captures,
                    &named,
                    &template,
                    &mut replacement,
                )?;
            }
            if position >= next {
                push(&mut out, &input[next..position])?;
                push(&mut out, &replacement)?;
                next = position + matched.len();
            }
        }
        if next < input.len() {
            push(&mut out, &input[next..])?;
        }
        super::strings::string(&out)
    }
    /// String.prototype.match / matchAll / search.
    pub(super) fn string_regexp_method(
        &mut self,
        name: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        if matches!(receiver, Value::Null | Value::Undefined) {
            return self.throw_error("TypeError", "null or undefined string receiver");
        }
        let regexp = args.first().cloned().unwrap_or(Value::Undefined);
        self.protect(&receiver)?;
        self.protect(&regexp)?;
        let (symbol, flags) = match name {
            "String.prototype.match" => (MATCH, Value::Undefined),
            "String.prototype.matchAll" => (MATCH_ALL, Value::String("g".into())),
            _ => (SEARCH, Value::Undefined),
        };
        if !matches!(regexp, Value::Null | Value::Undefined) {
            if symbol == MATCH_ALL && self.is_regexp(regexp.clone())? {
                let f = self.get_member(regexp.clone(), "flags")?;
                if matches!(f, Value::Null | Value::Undefined) {
                    return self.throw_error("TypeError", "missing regular expression flags");
                }
                if !self.to_string(f)?.contains('g') {
                    return self
                        .throw_error("TypeError", "matchAll requires a global regular expression");
                }
            }
            let method = self.get_member(regexp.clone(), Key::Symbol(symbol))?;
            self.protect(&method)?;
            if !matches!(method, Value::Null | Value::Undefined) {
                if !self.callable(&method) {
                    return self.throw_error("TypeError", "string protocol method is not callable");
                }
                return self.invoke_this(method, regexp, vec![receiver]);
            }
        }
        let text = self.to_js_string(receiver)?;
        let rx = self.regexp_create(regexp, flags)?;
        self.protect(&rx)?;
        let method = self.get_member(rx.clone(), Key::Symbol(symbol))?;
        self.protect(&method)?;
        self.invoke_this(method, rx, vec![Value::String(text.into())])
    }
}
