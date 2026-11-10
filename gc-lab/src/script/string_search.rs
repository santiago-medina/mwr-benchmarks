//! Literal-string search protocols; RegExp dispatch uses ordinary symbol hooks.
use super::{
    objects::Key,
    vm::{Engine, ScriptError, Value},
};
type Result<T> = std::result::Result<T, ScriptError>;
const MAX_UNITS: usize = 16_777_216;
fn append(out: &mut Vec<u16>, part: &[u16]) -> Result<()> {
    if out.len().saturating_add(part.len()) > MAX_UNITS {
        return Err(ScriptError::Runtime("string result limit"));
    }
    out.try_reserve(part.len())
        .map_err(|_| ScriptError::Runtime("string allocation failed"))?;
    out.extend_from_slice(part);
    Ok(())
}
impl Engine<'_> {
    pub(super) fn string_is_regexp(&mut self, value: Value) -> Result<bool> {
        if !matches!(value, Value::Object(_)) {
            return Ok(false);
        }
        self.is_regexp(value)
    }
    fn literal_index(
        &mut self,
        text: &[u16],
        search: &[u16],
        from: usize,
    ) -> Result<Option<usize>> {
        if search.len() > text.len() {
            return Ok(None);
        }
        for at in from..=text.len() - search.len() {
            self.tick()?;
            let mut equal = true;
            for (a, b) in text[at..].iter().zip(search) {
                self.tick()?;
                if a != b {
                    equal = false;
                    break;
                }
            }
            if equal {
                return Ok(Some(at));
            }
        }
        Ok(None)
    }
    pub(super) fn string_search_builtin(
        &mut self,
        name: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        if matches!(receiver, Value::Null | Value::Undefined) {
            return self.throw_error("TypeError", "null or undefined string receiver");
        }
        let search = args.first().cloned().unwrap_or(Value::Undefined);
        let second = args.get(1).cloned().unwrap_or(Value::Undefined);
        self.protect(&receiver)?;
        self.protect(&search)?;
        self.protect(&second)?;
        let split = name == "String.prototype.split";
        let all = name == "String.prototype.replaceAll";
        if matches!(search, Value::Object(_)) {
            if all && self.string_is_regexp(search.clone())? {
                let flags = self.get_member(search.clone(), "flags")?;
                self.protect(&flags)?;
                if matches!(flags, Value::Null | Value::Undefined) {
                    return self.throw_error("TypeError", "missing regular expression flags");
                }
                if !self.to_string(flags)?.contains('g') {
                    return self
                        .throw_error("TypeError", "replaceAll requires global regular expression");
                }
            }
            let hook = self.get_member(search.clone(), Key::Symbol(if split { 10 } else { 8 }))?;
            self.protect(&hook)?;
            if !matches!(hook, Value::Null | Value::Undefined) {
                if !self.callable(&hook) {
                    return self.throw_error("TypeError", "string protocol method is not callable");
                }
                return self.invoke_this(hook, search, vec![receiver, second]);
            }
        }
        let text = self.to_js_string(receiver)?;
        let units: Vec<u16> = text.units().into_owned();
        if split {
            let limit = if second == Value::Undefined {
                u32::MAX
            } else {
                super::numeric::uint32(self.to_number(second)?)
            } as usize;
            let separator = self.to_js_string(search.clone())?;
            let separator: Vec<u16> = separator.units().into_owned();
            let out = self.new_array(0)?;
            self.protect(&Value::Object(out))?;
            if limit == 0 {
                return Ok(Value::Object(out));
            }
            if search == Value::Undefined {
                self.append_array(out, Some(Value::String(text.clone())), false)?;
            } else if separator.is_empty() {
                for unit in units.iter().take(limit) {
                    self.tick()?;
                    self.append_array(out, Some(super::strings::string(&[*unit])?), false)?;
                }
            } else {
                let mut start = 0;
                let mut count = 0;
                while let Some(at) = self.literal_index(&units, &separator, start)? {
                    self.append_array(
                        out,
                        Some(super::strings::string(&units[start..at])?),
                        false,
                    )?;
                    count += 1;
                    if count == limit {
                        return Ok(Value::Object(out));
                    }
                    start = at + separator.len();
                }
                self.append_array(out, Some(super::strings::string(&units[start..])?), false)?;
            }
            return Ok(Value::Object(out));
        }
        let needle = self.to_js_string(search)?;
        let search: Vec<u16> = needle.units().into_owned();
        let functional = self.callable(&second);
        let template: Vec<u16> = if functional {
            vec![]
        } else {
            self.to_js_string(second.clone())?.units().into_owned()
        };
        let mut out = vec![];
        let mut from = 0;
        let mut end = 0;
        while let Some(at) = self.literal_index(&units, &search, from)? {
            self.tick()?;
            append(&mut out, &units[end..at])?;
            end = at + search.len();
            if functional {
                let replacement = self.invoke_this(
                    second.clone(),
                    Value::Undefined,
                    vec![
                        Value::String(needle.clone().into()),
                        Value::number(at as f64),
                        Value::String(text.clone().into()),
                    ],
                )?;
                self.protect(&replacement)?;
                let replacement: Vec<u16> = self.to_js_string(replacement)?.units().into_owned();
                append(&mut out, &replacement)?;
            } else {
                let mut i = 0;
                while i < template.len() {
                    self.tick()?;
                    let part = if template[i] == b'$' as u16 && i + 1 < template.len() {
                        match template[i + 1] {
                            36 => Some(&template[i..i + 1]),
                            38 => Some(search.as_slice()),
                            96 => Some(&units[..at]),
                            39 => Some(&units[end..]),
                            _ => None,
                        }
                    } else {
                        None
                    };
                    if let Some(part) = part {
                        append(&mut out, part)?;
                        i += 2;
                    } else {
                        append(&mut out, &template[i..i + 1])?;
                        i += 1;
                    }
                }
            }
            if !all {
                break;
            }
            from = at + search.len().max(1);
        }
        append(&mut out, &units[end..])?;
        super::strings::string(&out)
    }
}
