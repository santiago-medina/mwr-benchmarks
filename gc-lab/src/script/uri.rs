//! URI encoding validates percent escapes and Unicode scalar sequences before conversion.
use super::vm::{Engine, ScriptError, Value};
type Result<T> = std::result::Result<T, ScriptError>;
fn reserved(byte: u8) -> bool {
    b";/?:@&=+$,#".contains(&byte)
}
fn unescaped(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte)
}
fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}
fn escaped(input: &[u8], index: usize) -> Option<u8> {
    if input.get(index) != Some(&b'%') {
        return None;
    }
    Some(hex(*input.get(index + 1)?)? * 16 + hex(*input.get(index + 2)?)?)
}
fn decode(input: &str, uri: bool) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = String::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            let c = input[index..].chars().next()?;
            out.push(c);
            index += c.len_utf8();
            continue;
        }
        let first = escaped(bytes, index)?;
        if first < 128 {
            if uri && reserved(first) {
                out.push_str(&input[index..index + 3]);
            } else {
                out.push(first as char);
            }
            index += 3;
            continue;
        }
        let count = match first {
            0xc2..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf4 => 4,
            _ => return None,
        };
        let mut utf8 = [0u8; 4];
        utf8[0] = first;
        for offset in 1..count {
            utf8[offset] = escaped(bytes, index + offset * 3)?;
        }
        out.push_str(std::str::from_utf8(&utf8[..count]).ok()?);
        index += count * 3;
    }
    Some(out)
}
impl Engine<'_> {
    pub(super) fn uri_builtin(&mut self, name: &str, args: Vec<Value>) -> Result<Value> {
        let text = self.to_js_string(args.first().cloned().unwrap_or(Value::Undefined))?;
        // §19.2.6.5 Encode step 3.c.ii: CodePointAt reports a lone surrogate
        // ([[IsUnpairedSurrogate]]), which throws a URIError.
        let Some(text) = text.as_str() else {
            if name.starts_with("encode") {
                return self.throw_error("URIError", "URI malformed");
            }
            // §19.2.6.6 Decode step 4.b: a code unit other than `%` is copied as is.
            // Escapes are ASCII, so decoding each well-formed run separately is exact:
            // an escape interrupted by a lone surrogate is malformed either way.
            let reserved = name == "decodeURI";
            let mut units: Vec<u16> = vec![];
            for run in text.scalar_runs() {
                match run {
                    Ok(s) => match decode(&s, reserved) {
                        Some(d) => units.extend(d.encode_utf16()),
                        None => return self.throw_error("URIError", "malformed URI sequence"),
                    },
                    Err(unit) => units.push(unit),
                }
            }
            return Ok(Value::String(super::js_string::JsString::from_units(
                &units,
            )));
        };
        if name.starts_with("decode") {
            return match decode(&text, name == "decodeURI") {
                Some(s) => Ok(Value::String(s.into())),
                None => self.throw_error("URIError", "malformed URI sequence"),
            };
        }
        let uri = name == "encodeURI";
        let mut out = String::new();
        const HEX: &[u8] = b"0123456789ABCDEF";
        for byte in text.bytes() {
            if unescaped(byte) || uri && reserved(byte) {
                out.push(byte as char);
            } else {
                out.push('%');
                out.push(HEX[(byte >> 4) as usize] as char);
                out.push(HEX[(byte & 15) as usize] as char);
            }
        }
        Ok(Value::String(out.into()))
    }
}
