//! Lexical grammar pieces beyond ASCII: Unicode identifiers (ID_Start/ID_Continue,
//! `\u` escapes, ZWNJ/ZWJ) and RegularExpressionLiteral bodies.
use super::parser::SyntaxError;
use super::unicode_id::{ID_CONTINUE, ID_START};
use std::{iter::Peekable, str::CharIndices};
fn in_table(table: &[(u32, u32)], c: char) -> bool {
    let c = c as u32;
    table
        .binary_search_by(|&(a, b)| {
            if b < c {
                std::cmp::Ordering::Less
            } else if a > c {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}
/// IdentifierStartChar: ID_Start, `$`, `_`.
pub(super) fn id_start(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_alphabetic() || c == '$' || c == '_'
    } else {
        in_table(ID_START, c)
    }
}
/// IdentifierPartChar: ID_Continue, `$`, ZWNJ, ZWJ.
pub(super) fn id_continue(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_alphanumeric() || c == '$' || c == '_'
    } else {
        c == '\u{200c}' || c == '\u{200d}' || in_table(ID_CONTINUE, c)
    }
}
/// Can an identifier (or a `#name`) start with this source character?
pub(super) fn starts_identifier(c: char) -> bool {
    c == '\\' || id_start(c)
}
type Chars<'a> = Peekable<CharIndices<'a>>;
/// `\uXXXX` or `\u{X...}` after the backslash has been consumed.
fn unicode_escape(chars: &mut Chars<'_>, error: SyntaxError) -> Result<char, SyntaxError> {
    if chars.next().map(|(_, c)| c) != Some('u') {
        return Err(error);
    }
    let mut n = 0u32;
    if chars.peek().is_some_and(|(_, c)| *c == '{') {
        chars.next();
        let mut digits = 0;
        loop {
            let (_, c) = chars.next().ok_or(error.clone())?;
            if c == '}' && digits > 0 {
                break;
            }
            n = n * 16 + c.to_digit(16).ok_or(error.clone())?;
            digits += 1;
            if n > 0x10ffff {
                return Err(error);
            }
        }
    } else {
        for _ in 0..4 {
            let (_, c) = chars.next().ok_or(error.clone())?;
            n = n * 16 + c.to_digit(16).ok_or(error.clone())?;
        }
    }
    char::from_u32(n).ok_or(error)
}
/// Lexes IdentifierName whose first source character `first` was already taken.
/// Returns the cooked name and whether any escape sequence occurred.
pub(super) fn identifier(
    chars: &mut Chars<'_>,
    first: char,
    offset: usize,
) -> Result<(String, bool), SyntaxError> {
    let error = SyntaxError {
        offset,
        message: "invalid identifier escape",
    };
    let mut escaped = false;
    let mut name = String::new();
    let mut c = Some(first);
    while let Some(ch) = c {
        let cooked = if ch == '\\' {
            escaped = true;
            unicode_escape(chars, error.clone())?
        } else {
            ch
        };
        let valid = if name.is_empty() {
            id_start(cooked)
        } else {
            id_continue(cooked)
        };
        if !valid {
            return Err(error);
        }
        name.push(cooked);
        c = match chars.peek() {
            Some(&(_, next)) if next == '\\' || id_continue(next) => {
                chars.next();
                Some(next)
            }
            _ => None,
        };
    }
    Ok((name, escaped))
}
/// RegularExpressionBody and flags after the opening `/` was consumed.
pub(super) fn regexp(
    chars: &mut Chars<'_>,
    offset: usize,
) -> Result<(String, String), SyntaxError> {
    let error = |message| SyntaxError { offset, message };
    let line_end = |c: char| matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}');
    let mut body = String::new();
    let mut class = false;
    loop {
        let (_, c) = chars.next().ok_or(error("unterminated regexp literal"))?;
        if line_end(c) {
            return Err(error("unterminated regexp literal"));
        }
        match c {
            '/' if !class => break,
            '[' => class = true,
            ']' => class = false,
            '\\' => {
                body.push(c);
                let (_, next) = chars.next().ok_or(error("unterminated regexp literal"))?;
                if line_end(next) {
                    return Err(error("unterminated regexp literal"));
                }
                body.push(next);
                continue;
            }
            _ => {}
        }
        body.push(c);
    }
    if body.is_empty() {
        return Err(error("empty regexp literal"));
    }
    let mut flags = String::new();
    while let Some(&(_, c)) = chars.peek() {
        if c == '\\' {
            return Err(error("escaped regexp flag"));
        }
        if !id_continue(c) {
            break;
        }
        chars.next();
        if !"dgimsuvy".contains(c) || flags.contains(c) {
            return Err(error("invalid regexp flags"));
        }
        flags.push(c);
    }
    if flags.contains('u') && flags.contains('v') {
        return Err(error("invalid regexp flags"));
    }
    Ok((body, flags))
}
