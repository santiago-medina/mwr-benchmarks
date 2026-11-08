//! Template segments retain normalized raw text and independently cooked escapes.
use super::parser::SyntaxError;
use std::{iter::Peekable, str::CharIndices};
pub(super) fn segment(
    source: &str,
    chars: &mut Peekable<CharIndices<'_>>,
    offset: usize,
) -> Result<(Option<String>, String, bool), SyntaxError> {
    let error = |message| SyntaxError { offset, message };
    let start = chars.peek().map(|(i, _)| *i).unwrap_or(source.len());
    while let Some((end, ch)) = chars.next() {
        if ch == '\\' {
            chars.next().ok_or(error("unterminated template escape"))?;
            continue;
        }
        let tail = ch == '`';
        if tail || ch == '$' && chars.peek().is_some_and(|(_, ch)| *ch == '{') {
            if !tail {
                chars.next();
            }
            let raw = source[start..end].replace("\r\n", "\n").replace('\r', "\n");
            return Ok((
                cook(&raw).map_err(|_| error("surrogate template storage unsupported"))?,
                raw,
                tail,
            ));
        }
    }
    Err(error("unterminated template literal"))
}
fn cook(raw: &str) -> Result<Option<String>, ()> {
    let mut chars = raw.chars().peekable();
    let mut units = vec![];
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            units.extend(ch.encode_utf16(&mut [0; 2]).iter().copied());
            continue;
        }
        let Some(ch) = chars.next() else {
            return Ok(None);
        };
        let ch = match ch {
            '\n' | '\u{2028}' | '\u{2029}' => continue,
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            'b' => '\u{8}',
            'f' => '\u{c}',
            'v' => '\u{b}',
            '0' if !chars.peek().is_some_and(|ch| ch.is_ascii_digit()) => '\0',
            '0'..='9' => return Ok(None),
            'x' | 'u' => {
                let braced = ch == 'u' && chars.peek() == Some(&'{');
                if braced {
                    chars.next();
                }
                let mut number = 0u32;
                let mut digits = 0;
                let width = if ch == 'x' { 2 } else { 4 };
                loop {
                    if braced && chars.peek() == Some(&'}') {
                        chars.next();
                        break;
                    }
                    if !braced && digits == width {
                        break;
                    }
                    let Some(digit) = chars.next().and_then(|ch| ch.to_digit(16)) else {
                        return Ok(None);
                    };
                    number = match number.checked_mul(16).and_then(|n| n.checked_add(digit)) {
                        Some(n) => n,
                        None => return Ok(None),
                    };
                    digits += 1;
                    if number > 0x10ffff {
                        return Ok(None);
                    }
                }
                if digits == 0 {
                    return Ok(None);
                }
                if number <= 0xffff {
                    units.push(number as u16);
                } else {
                    let n = number - 0x10000;
                    units.push(0xd800 + (n >> 10) as u16);
                    units.push(0xdc00 + (n & 1023) as u16);
                }
                continue;
            }
            ch => ch,
        };
        units.extend(ch.encode_utf16(&mut [0; 2]).iter().copied());
    }
    String::from_utf16(&units).map(Some).map_err(|_| ())
}
