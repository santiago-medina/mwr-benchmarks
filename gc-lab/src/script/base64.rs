//! Uint8Array base64/hex: ECMA-262 §23.3.2.x / §23.3.3.x (Uint8Array.fromBase64, fromHex,
//! prototype.toBase64, toHex, setFromBase64, setFromHex) and the abstract operations
//! FromBase64, FromHex, DecodeBase64Chunk and SkipAsciiWhitespace, transcribed step by step.
//! The decoders never call user code, so the target buffer cannot change while decoding.
use super::{
    buffer_elements::Element,
    typed_arrays::Typed,
    vm::{Engine, ScriptError, Value},
};
use crate::dual::Id;
type Result<T> = std::result::Result<T, ScriptError>;

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// The result record of FromBase64 / FromHex: [[Read]], [[Bytes]], [[Error]] (a SyntaxError).
struct Decoded {
    read: usize,
    bytes: Vec<u8>,
    error: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum LastChunk {
    Loose,
    Strict,
    StopBeforePartial,
}

fn skip_ascii_whitespace(s: &[u8], mut index: usize) -> usize {
    while index < s.len() && matches!(s[index], 0x09 | 0x0A | 0x0C | 0x0D | 0x20) {
        index += 1;
    }
    index
}

fn sextet(c: u8) -> Option<u32> {
    ALPHABET.iter().position(|&a| a == c).map(|p| p as u32)
}

/// DecodeBase64Chunk: pads a 2- or 3-character chunk with "A"s; `None` is the SyntaxError
/// for non-zero padding bits when throwOnExtraBits is true.
fn decode_chunk(chunk: &[u8], throw_on_extra_bits: bool) -> Option<Vec<u8>> {
    let mut n = 0u32;
    for i in 0..4 {
        n = (n << 6) | chunk.get(i).map_or(0, |&c| sextet(c).unwrap_or(0));
    }
    let b = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
    match chunk.len() {
        2 if throw_on_extra_bits && b[1] != 0 => None,
        2 => Some(vec![b[0]]),
        3 if throw_on_extra_bits && b[2] != 0 => None,
        3 => Some(vec![b[0], b[1]]),
        _ => Some(b.to_vec()),
    }
}

/// FromBase64(string, alphabet, lastChunkHandling [, maxLength]). Every accepted character
/// is ASCII, so byte offsets equal UTF-16 offsets wherever [[Read]] is reported; the first
/// non-ASCII byte is outside the alphabet and stops decoding with an error.
fn from_base64(s: &[u8], url: bool, last: LastChunk, max_length: usize) -> Decoded {
    let fail = |read, bytes| Decoded {
        read,
        bytes,
        error: true,
    };
    let ok = |read, bytes| Decoded {
        read,
        bytes,
        error: false,
    };
    // 2. maxLength = 0 decodes nothing.
    if max_length == 0 {
        return ok(0, Vec::new());
    }
    let (mut read, mut bytes, mut chunk) = (0, Vec::new(), Vec::with_capacity(4));
    let (mut index, length) = (0, s.len());
    loop {
        index = skip_ascii_whitespace(s, index);
        // 4.b End of input.
        if index == length {
            if !chunk.is_empty() {
                match last {
                    LastChunk::StopBeforePartial => return ok(read, bytes),
                    LastChunk::Loose if chunk.len() == 1 => return fail(read, bytes),
                    LastChunk::Loose => bytes.extend(decode_chunk(&chunk, false).unwrap()),
                    LastChunk::Strict => return fail(read, bytes),
                }
            }
            return ok(length, bytes);
        }
        let mut c = s[index];
        index += 1;
        // 4.e Padding.
        if c == b'=' {
            if chunk.len() < 2 {
                return fail(read, bytes);
            }
            index = skip_ascii_whitespace(s, index);
            if chunk.len() == 2 {
                if index == length {
                    if last == LastChunk::StopBeforePartial {
                        return ok(read, bytes);
                    }
                    return fail(read, bytes);
                }
                if s[index] == b'=' {
                    index = skip_ascii_whitespace(s, index + 1);
                }
            }
            if index < length {
                return fail(read, bytes);
            }
            match decode_chunk(&chunk, last == LastChunk::Strict) {
                Some(decoded) => bytes.extend(decoded),
                None => return fail(read, bytes),
            }
            return ok(length, bytes);
        }
        // 4.f base64url substitutions.
        if url {
            match c {
                b'+' | b'/' => return fail(read, bytes),
                b'-' => c = b'+',
                b'_' => c = b'/',
                _ => {}
            }
        }
        if sextet(c).is_none() {
            return fail(read, bytes);
        }
        // 4.h-i Stop before a chunk that would overflow maxLength.
        let remaining = max_length - bytes.len();
        if (remaining == 1 && chunk.len() == 2) || (remaining == 2 && chunk.len() == 3) {
            return ok(read, bytes);
        }
        chunk.push(c);
        if chunk.len() == 4 {
            bytes.extend(decode_chunk(&chunk, false).unwrap());
            chunk.clear();
            read = index;
            if bytes.len() == max_length {
                return ok(read, bytes);
            }
        }
    }
}

/// FromHex(string [, maxLength]); `utf16_length` is the string's length in code units.
fn from_hex(s: &[u8], utf16_length: usize, max_length: usize) -> Decoded {
    let mut decoded = Decoded {
        read: 0,
        bytes: Vec::new(),
        error: false,
    };
    if utf16_length % 2 != 0 {
        decoded.error = true;
        return decoded;
    }
    while decoded.read < utf16_length && decoded.bytes.len() < max_length {
        let pair = s.get(decoded.read..decoded.read + 2).and_then(|p| {
            let digit = |c: u8| (c as char).to_digit(16);
            Some((digit(p[0])? << 4 | digit(p[1])?) as u8)
        });
        let Some(byte) = pair else {
            decoded.error = true;
            return decoded;
        };
        decoded.read += 2;
        decoded.bytes.push(byte);
    }
    decoded
}

fn to_base64(bytes: &[u8], url: bool, omit_padding: bool) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let symbol = |n: u32| {
        let c = ALPHABET[(n & 63) as usize];
        (match (url, c) {
            (true, b'+') => b'-',
            (true, b'/') => b'_',
            _ => c,
        }) as char
    };
    for group in bytes.chunks(3) {
        let n = group
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for i in 0..=group.len() {
            out.push(symbol(n >> (18 - 6 * i)));
        }
        if !omit_padding {
            for _ in group.len()..3 {
                out.push('=');
            }
        }
    }
    out
}

impl Engine<'_> {
    pub(super) fn uint8_base64_builtin(
        &mut self,
        name: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Undefined);
        match name {
            "Uint8Array.fromBase64" => {
                let Value::String(string) = arg(0) else {
                    return self.throw_error("TypeError", "fromBase64 requires a string");
                };
                let string = string.lossy();
                let options = self.base64_options(arg(1))?;
                let url = self.alphabet_option(&options)?;
                let last = self.last_chunk_option(&options)?;
                let decoded = from_base64(string.as_bytes(), url, last, usize::MAX);
                self.uint8_from_bytes(decoded)
            }
            "Uint8Array.fromHex" => {
                let Value::String(string) = arg(0) else {
                    return self.throw_error("TypeError", "fromHex requires a string");
                };
                let string = string.lossy();
                let length = string.encode_utf16().count();
                self.uint8_from_bytes(from_hex(string.as_bytes(), length, usize::MAX))
            }
            "Uint8Array.prototype.toBase64" => {
                let (_, typed) = self.validate_uint8(&receiver)?;
                let options = self.base64_options(arg(0))?;
                let url = self.alphabet_option(&options)?;
                let omit = match &options {
                    Some(o) => self.get_member(o.clone(), "omitPadding")?.truthy(),
                    None => false,
                };
                let bytes = self.uint8_bytes(&typed)?;
                Ok(Value::String((to_base64(&bytes, url, omit)).into()))
            }
            "Uint8Array.prototype.toHex" => {
                let (_, typed) = self.validate_uint8(&receiver)?;
                let bytes = self.uint8_bytes(&typed)?;
                Ok(Value::String(
                    bytes
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>()
                        .into(),
                ))
            }
            "Uint8Array.prototype.setFromBase64" | "Uint8Array.prototype.setFromHex" => {
                let (_, typed) = self.validate_uint8(&receiver)?;
                // ValidateUint8Array(into, write) (immutable-arraybuffer §23.3.3.1).
                if self.typed_immutable(&typed)? {
                    return self
                        .throw_error("TypeError", "cannot write to an immutable ArrayBuffer");
                }
                let Value::String(string) = arg(0) else {
                    return self.throw_error("TypeError", "the input must be a string");
                };
                let string = string.lossy();
                let decoded = if name.ends_with("Base64") {
                    let options = self.base64_options(arg(1))?;
                    let url = self.alphabet_option(&options)?;
                    let last = self.last_chunk_option(&options)?;
                    let Some(length) = self.typed_length(&typed)? else {
                        return self.throw_error("TypeError", "typed view is out of bounds");
                    };
                    from_base64(string.as_bytes(), url, last, length)
                } else {
                    let Some(length) = self.typed_length(&typed)? else {
                        return self.throw_error("TypeError", "typed view is out of bounds");
                    };
                    let units = string.encode_utf16().count();
                    from_hex(string.as_bytes(), units, length)
                };
                // SetUint8ArrayBytes, then the error (bytes decoded before it stay written).
                let buffer = self
                    .data(typed.view.buffer)?
                    .buffer
                    .as_ref()
                    .unwrap()
                    .clone();
                let start = typed.view.offset;
                buffer.borrow_mut().bytes[start..start + decoded.bytes.len()]
                    .copy_from_slice(&decoded.bytes);
                if decoded.error {
                    return self.throw_error("SyntaxError", "invalid encoded string");
                }
                let result = self.new_object(Some(self.intrinsics.as_ref().unwrap().object))?;
                self.protect(&Value::Object(result))?;
                self.put_data(result, "read", Value::number(decoded.read as f64))?;
                let written = Value::number(decoded.bytes.len() as f64);
                self.put_data(result, "written", written)?;
                Ok(Value::Object(result))
            }
            _ => Err(ScriptError::Unsupported("Uint8Array operation")),
        }
    }

    /// GetOptionsObject: undefined stands for an empty null-prototype object (`None`).
    fn base64_options(&mut self, options: Value) -> Result<Option<Value>> {
        match options {
            Value::Undefined => Ok(None),
            Value::Object(_) => Ok(Some(options)),
            _ => self.throw_error("TypeError", "options must be an object"),
        }
    }

    /// The "alphabet" option: undefined is "base64"; anything but the two strings throws
    /// (no ToString). Returns true for "base64url".
    fn alphabet_option(&mut self, options: &Option<Value>) -> Result<bool> {
        let value = match options {
            Some(o) => self.get_member(o.clone(), "alphabet")?,
            None => Value::Undefined,
        };
        match value {
            Value::Undefined => Ok(false),
            Value::String(s) if s == "base64" => Ok(false),
            Value::String(s) if s == "base64url" => Ok(true),
            _ => self.throw_error("TypeError", "alphabet must be \"base64\" or \"base64url\""),
        }
    }

    fn last_chunk_option(&mut self, options: &Option<Value>) -> Result<LastChunk> {
        let value = match options {
            Some(o) => self.get_member(o.clone(), "lastChunkHandling")?,
            None => Value::Undefined,
        };
        match value {
            Value::Undefined => Ok(LastChunk::Loose),
            Value::String(s) if s == "loose" => Ok(LastChunk::Loose),
            Value::String(s) if s == "strict" => Ok(LastChunk::Strict),
            Value::String(s) if s == "stop-before-partial" => Ok(LastChunk::StopBeforePartial),
            _ => self.throw_error(
                "TypeError",
                "lastChunkHandling must be \"loose\", \"strict\" or \"stop-before-partial\"",
            ),
        }
    }

    /// ValidateUint8Array: a TypedArray whose [[TypedArrayName]] is "Uint8Array".
    fn validate_uint8(&mut self, value: &Value) -> Result<(Id, Typed)> {
        if let Value::Object(id) = value {
            if let Some(typed) = self.data(*id).ok().and_then(|d| d.typed.clone()) {
                if typed.kind == Element::U8 {
                    return Ok((*id, typed));
                }
            }
        }
        self.throw_error("TypeError", "receiver is not a Uint8Array")
    }

    /// GetUint8ArrayBytes: TypeError when the view is detached or out of bounds.
    fn uint8_bytes(&mut self, typed: &Typed) -> Result<Vec<u8>> {
        let Some(length) = self.typed_length(typed)? else {
            return self.throw_error("TypeError", "typed view is detached or out of bounds");
        };
        let buffer = self
            .data(typed.view.buffer)?
            .buffer
            .as_ref()
            .unwrap()
            .borrow();
        let start = typed.view.offset;
        Ok(buffer.bytes[start..start + length].to_vec())
    }

    /// Throws the decoder's SyntaxError, else AllocateTypedArray(%Uint8Array%) with the bytes.
    fn uint8_from_bytes(&mut self, decoded: Decoded) -> Result<Value> {
        if decoded.error {
            return self.throw_error("SyntaxError", "invalid encoded string");
        }
        let proto = self.intrinsics.as_ref().unwrap().collection["Uint8Array"];
        let array = self.typed_owned(proto, Element::U8, decoded.bytes.len() as u64)?;
        let Value::Object(id) = array else {
            unreachable!()
        };
        let typed = self.data(id)?.typed.clone().unwrap();
        let buffer = self
            .data(typed.view.buffer)?
            .buffer
            .as_ref()
            .unwrap()
            .clone();
        buffer.borrow_mut().bytes[..decoded.bytes.len()].copy_from_slice(&decoded.bytes);
        Ok(array)
    }
}
