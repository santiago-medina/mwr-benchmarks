//! JSON syntax and host-independent serialization using ordinary object operations.
use super::{
    objects::Key,
    vm::{Engine, ScriptError, Value},
};
use crate::dual::Id;
type Result<T> = std::result::Result<T, ScriptError>;
enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}
struct Reader<'a> {
    text: &'a [u8],
    pos: usize,
}
impl Reader<'_> {
    fn ws(&mut self) {
        while self
            .text
            .get(self.pos)
            .is_some_and(|c| matches!(c, b' ' | b'\t' | b'\n' | b'\r'))
        {
            self.pos += 1;
        }
    }
    fn take(&mut self, c: u8) -> bool {
        if self.text.get(self.pos) == Some(&c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn hex(&mut self) -> std::result::Result<u16, &'static str> {
        let mut n = 0u16;
        for _ in 0..4 {
            let c = *self.text.get(self.pos).ok_or("incomplete Unicode escape")?;
            self.pos += 1;
            n = n * 16
                + match c {
                    b'0'..=b'9' => (c - b'0') as u16,
                    b'a'..=b'f' => (c - b'a' + 10) as u16,
                    b'A'..=b'F' => (c - b'A' + 10) as u16,
                    _ => return Err("invalid Unicode escape"),
                };
        }
        Ok(n)
    }
    fn string(&mut self) -> std::result::Result<String, &'static str> {
        if !self.take(b'"') {
            return Err("expected JSON string");
        }
        let mut out = String::new();
        loop {
            let c = *self.text.get(self.pos).ok_or("unterminated JSON string")?;
            self.pos += 1;
            match c {
                b'"' => return Ok(out),
                0..=31 => return Err("control character in JSON string"),
                b'\\' => {
                    let c = *self.text.get(self.pos).ok_or("incomplete JSON escape")?;
                    self.pos += 1;
                    match c {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let a = self.hex()?;
                            let code = if (0xd800..=0xdbff).contains(&a) {
                                if !self.take(b'\\') || !self.take(b'u') {
                                    return Err("unpaired surrogate storage unsupported");
                                }
                                let b = self.hex()?;
                                if !(0xdc00..=0xdfff).contains(&b) {
                                    return Err("unpaired surrogate storage unsupported");
                                }
                                0x10000 + (((a as u32) - 0xd800) << 10) + (b as u32 - 0xdc00)
                            } else {
                                a as u32
                            };
                            out.push(
                                char::from_u32(code)
                                    .ok_or("unpaired surrogate storage unsupported")?,
                            );
                        }
                        _ => return Err("invalid JSON escape"),
                    }
                }
                0x20..=0x7f => out.push(c as char),
                _ => {
                    self.pos -= 1;
                    let s =
                        std::str::from_utf8(&self.text[self.pos..]).map_err(|_| "invalid UTF-8")?;
                    let ch = s.chars().next().unwrap();
                    out.push(ch);
                    self.pos += ch.len_utf8();
                }
            }
        }
    }
    fn value(&mut self, depth: usize) -> std::result::Result<Json, &'static str> {
        if depth > 256 {
            return Err("JSON nesting limit");
        }
        self.ws();
        match self.text.get(self.pos).copied() {
            Some(b'"') => Ok(Json::String(self.string()?)),
            Some(b'[') => {
                self.pos += 1;
                self.ws();
                let mut items = vec![];
                if self.take(b']') {
                    return Ok(Json::Array(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    self.ws();
                    if self.take(b']') {
                        break;
                    }
                    if !self.take(b',') {
                        return Err("expected comma in JSON array");
                    }
                }
                Ok(Json::Array(items))
            }
            Some(b'{') => {
                self.pos += 1;
                self.ws();
                let mut items = vec![];
                if self.take(b'}') {
                    return Ok(Json::Object(items));
                }
                loop {
                    self.ws();
                    let key = self.string()?;
                    self.ws();
                    if !self.take(b':') {
                        return Err("expected colon in JSON object");
                    }
                    items.push((key, self.value(depth + 1)?));
                    self.ws();
                    if self.take(b'}') {
                        break;
                    }
                    if !self.take(b',') {
                        return Err("expected comma in JSON object");
                    }
                }
                Ok(Json::Object(items))
            }
            Some(b'n' | b't' | b'f') => {
                for (word, v) in [
                    (b"null".as_slice(), Json::Null),
                    (b"true".as_slice(), Json::Bool(true)),
                    (b"false".as_slice(), Json::Bool(false)),
                ] {
                    if self.text[self.pos..].starts_with(word) {
                        self.pos += word.len();
                        return Ok(v);
                    }
                }
                Err("invalid JSON literal")
            }
            Some(b'-' | b'0'..=b'9') => {
                let start = self.pos;
                self.take(b'-');
                if !self.take(b'0') {
                    if !self
                        .text
                        .get(self.pos)
                        .is_some_and(|c| matches!(c, b'1'..=b'9'))
                    {
                        return Err("invalid JSON number");
                    }
                    while self.text.get(self.pos).is_some_and(u8::is_ascii_digit) {
                        self.pos += 1;
                    }
                }
                if self.take(b'.') {
                    let start = self.pos;
                    while self.text.get(self.pos).is_some_and(u8::is_ascii_digit) {
                        self.pos += 1;
                    }
                    if start == self.pos {
                        return Err("missing fraction digits");
                    }
                }
                if self.take(b'e') || self.take(b'E') {
                    if !self.take(b'+') {
                        self.take(b'-');
                    }
                    let start = self.pos;
                    while self.text.get(self.pos).is_some_and(u8::is_ascii_digit) {
                        self.pos += 1;
                    }
                    if start == self.pos {
                        return Err("missing exponent digits");
                    }
                }
                let s = std::str::from_utf8(&self.text[start..self.pos]).unwrap();
                Ok(Json::Number(s.parse().map_err(|_| "invalid JSON number")?))
            }
            _ => Err("invalid JSON value"),
        }
    }
}
fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 32 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
struct Writer {
    replacer: Value,
    keys: Option<Vec<String>>,
    gap: String,
    stack: Vec<Id>,
}
impl Engine<'_> {
    fn json_materialize(&mut self, node: Json) -> Result<Value> {
        match node {
            Json::Null => Ok(Value::Null),
            Json::Bool(b) => Ok(Value::Bool(b)),
            Json::Number(n) => Ok(Value::number(n)),
            Json::String(s) => Ok(Value::String(s)),
            Json::Array(items) => {
                let id = self.new_array(items.len() as u32)?;
                self.protect(&Value::Object(id))?;
                for (i, item) in items.into_iter().enumerate() {
                    let v = self.json_materialize(item)?;
                    self.put_data(id, i.to_string(), v)?;
                }
                Ok(Value::Object(id))
            }
            Json::Object(items) => {
                let id = self.new_object(Some(self.intrinsics.as_ref().unwrap().object))?;
                self.protect(&Value::Object(id))?;
                for (key, item) in items {
                    let v = self.json_materialize(item)?;
                    self.put_data(id, key, v)?;
                }
                Ok(Value::Object(id))
            }
        }
    }
    fn json_revive(
        &mut self,
        holder: Id,
        key: String,
        reviver: Value,
        depth: usize,
    ) -> Result<Value> {
        self.tick()?;
        if depth > 256 {
            return self.throw_error("RangeError", "JSON reviver nesting limit");
        }
        let value = self.get_member(Value::Object(holder), key.clone())?;
        self.protect(&value)?;
        if let Value::Object(id) = value.clone() {
            let keys = if self.data(id)?.array {
                let len = self.length_of(id)?;
                (0..len).map(|i| i.to_string()).collect()
            } else {
                self.json_keys(id)?
            };
            for key in keys {
                let v = self.json_revive(id, key.clone(), reviver.clone(), depth + 1)?;
                if v == Value::Undefined {
                    self.delete(id, key)?;
                } else {
                    self.define(
                        id,
                        key,
                        super::objects::Descriptor {
                            value: Some(v),
                            writable: Some(true),
                            enumerable: Some(true),
                            configurable: Some(true),
                            ..Default::default()
                        },
                    )?;
                }
            }
        }
        self.invoke_this(
            reviver,
            Value::Object(holder),
            vec![Value::String(key), value],
        )
    }
    fn json_keys(&self, id: Id) -> Result<Vec<String>> {
        let mut keys = vec![];
        for key in self.own_keys(id)? {
            if let Key::String(s) = key {
                if self.own(id, &s)?.is_some_and(|p| p.enumerable) {
                    keys.push(s);
                }
            }
        }
        Ok(keys)
    }
    pub(super) fn json_builtin(&mut self, name: &str, args: Vec<Value>) -> Result<Value> {
        let input = args.first().cloned().unwrap_or(Value::Undefined);
        if name == "JSON.parse" {
            let text = self.to_string(input)?;
            let mut reader = Reader {
                text: text.as_bytes(),
                pos: 0,
            };
            let parsed = reader.value(0).and_then(|v| {
                reader.ws();
                if reader.pos == reader.text.len() {
                    Ok(v)
                } else {
                    Err("trailing JSON input")
                }
            });
            let node = match parsed {
                Ok(v) => v,
                Err("unpaired surrogate storage unsupported") => {
                    return Err(ScriptError::Unsupported("JSON unpaired UTF-16 surrogate"))
                }
                Err(message) => return self.throw_error("SyntaxError", message),
            };
            let value = self.json_materialize(node)?;
            self.protect(&value)?;
            let reviver = args.get(1).cloned().unwrap_or(Value::Undefined);
            if !self.callable(&reviver) {
                return Ok(value);
            }
            let holder = self.new_object(Some(self.intrinsics.as_ref().unwrap().object))?;
            self.protect(&Value::Object(holder))?;
            self.put_data(holder, "", value)?;
            return self.json_revive(holder, String::new(), reviver, 0);
        }
        let replacer = args.get(1).cloned().unwrap_or(Value::Undefined);
        let mut keys = None;
        if let Value::Object(id) = replacer.clone() {
            if self.data(id)?.array {
                let len = self.length_of(id)?;
                let mut list = vec![];
                for i in 0..len {
                    self.tick()?;
                    let mut v = self.get_member(replacer.clone(), i.to_string())?;
                    self.protect(&v)?;
                    if let Value::Object(id) = v.clone() {
                        if let Some(boxed) = self.data(id)?.boxed.clone() {
                            v = match boxed {
                                Value::String(_) => Value::String(self.to_string(v)?),
                                Value::Number(_) => Value::number(self.to_number(v)?),
                                _ => Value::Undefined,
                            };
                        }
                    }
                    if matches!(v, Value::String(_) | Value::Number(_)) {
                        let s = self.to_string(v)?;
                        if !list.contains(&s) {
                            list.push(s);
                        }
                    }
                }
                keys = Some(list);
            }
        }
        let mut space = args.get(2).cloned().unwrap_or(Value::Undefined);
        if let Value::Object(id) = space.clone() {
            if let Some(boxed) = self.data(id)?.boxed.clone() {
                space = match boxed {
                    Value::String(_) => Value::String(self.to_string(space)?),
                    Value::Number(_) => Value::number(self.to_number(space)?),
                    _ => Value::Undefined,
                };
            }
        }
        let gap = match space {
            Value::Number(n) => " ".repeat(f64::from_bits(n).clamp(0.0, 10.0) as usize),
            Value::String(s) => {
                let mut units = 0;
                s.chars()
                    .take_while(|c| {
                        units += c.len_utf16();
                        units <= 10
                    })
                    .collect()
            }
            _ => String::new(),
        };
        let mut writer = Writer {
            replacer: if self.callable(&replacer) {
                replacer
            } else {
                Value::Undefined
            },
            keys,
            gap,
            stack: vec![],
        };
        let holder = self.new_object(Some(self.intrinsics.as_ref().unwrap().object))?;
        self.protect(&Value::Object(holder))?;
        self.put_data(holder, "", input)?;
        Ok(match self.json_serialize(holder, "", &mut writer)? {
            Some(s) => Value::String(s),
            None => Value::Undefined,
        })
    }
    fn json_serialize(
        &mut self,
        holder: Id,
        key: &str,
        writer: &mut Writer,
    ) -> Result<Option<String>> {
        self.tick()?;
        if writer.stack.len() > 256 {
            return self.throw_error("RangeError", "JSON serialization nesting limit");
        }
        let mut value = self.get_member(Value::Object(holder), key)?;
        self.protect(&value)?;
        if matches!(value, Value::Object(_)) {
            let method = self.get_member(value.clone(), "toJSON")?;
            self.protect(&method)?;
            if self.callable(&method) {
                value = self.invoke_this(method, value, vec![Value::String(key.into())])?;
                self.protect(&value)?;
            }
        }
        if writer.replacer != Value::Undefined {
            value = self.invoke_this(
                writer.replacer.clone(),
                Value::Object(holder),
                vec![Value::String(key.into()), value],
            )?;
            self.protect(&value)?;
        }
        if let Value::Object(id) = value.clone() {
            if let Some(boxed) = self.data(id)?.boxed.clone() {
                value = match boxed {
                    Value::Number(_) => Value::number(self.to_number(value)?),
                    Value::String(_) => Value::String(self.to_string(value)?),
                    Value::Bool(_) => boxed,
                    _ => value,
                };
            }
        }
        match value {
            Value::Null => Ok(Some("null".into())),
            Value::Bool(v) => Ok(Some(v.to_string())),
            Value::Number(n) => {
                let n = f64::from_bits(n);
                Ok(Some(if n.is_finite() {
                    if n == 0.0 {
                        "0".into()
                    } else {
                        self.to_string(Value::number(n))?
                    }
                } else {
                    "null".into()
                }))
            }
            Value::String(s) => Ok(Some(quote(&s))),
            Value::Object(id) => {
                if self.callable(&Value::Object(id)) {
                    return Ok(None);
                }
                if writer.stack.contains(&id) {
                    return self.throw_error("TypeError", "cyclic JSON value");
                }
                let array = self.data(id)?.array;
                let keys = if array {
                    let len = self.length_of(id)?;
                    (0..len).map(|i| i.to_string()).collect()
                } else if let Some(keys) = writer.keys.clone() {
                    keys
                } else {
                    self.json_keys(id)?
                };
                writer.stack.push(id);
                let depth = writer.stack.len();
                let mut parts = vec![];
                for key in keys {
                    let part = self.frame(|e| e.json_serialize(id, &key, writer))?;
                    if array {
                        parts.push(part.unwrap_or_else(|| "null".into()));
                    } else if let Some(part) = part {
                        parts.push(format!(
                            "{}:{}{}",
                            quote(&key),
                            if writer.gap.is_empty() { "" } else { " " },
                            part
                        ));
                    }
                }
                writer.stack.pop();
                let (open, close) = if array { ("[", "]") } else { ("{", "}") };
                let output = if parts.is_empty() {
                    format!("{open}{close}")
                } else if writer.gap.is_empty() {
                    format!("{open}{}{close}", parts.join(","))
                } else {
                    let indent = writer.gap.repeat(depth);
                    format!(
                        "{open}\n{indent}{}\n{}{close}",
                        parts.join(&format!(",\n{indent}")),
                        writer.gap.repeat(depth - 1)
                    )
                };
                Ok(Some(output))
            }
            _ => Ok(None),
        }
    }
}
