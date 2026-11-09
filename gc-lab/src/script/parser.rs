//! Bounded grammar for strict scripts; unsupported forms are rejected explicitly.
pub use super::patterns::{Pattern, PatternKey};
use std::collections::HashSet;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxError {
    pub offset: usize,
    pub message: &'static str,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Param {
    pub binding: Pattern,
    pub default: Option<Expr>,
    pub rest: bool,
}
impl From<String> for Param {
    fn from(name: String) -> Self {
        Self {
            binding: Pattern::Name(name),
            default: None,
            rest: false,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    Template(Box<super::templates::Template>),
    Name(String),
    String(String),
    Number(String),
    BigInt(String),
    RegExp {
        pattern: String,
        flags: String,
    },
    Bool(bool),
    Null,
    Undefined,
    This,
    Arrow {
        params: Vec<Param>,
        body: Vec<Stmt>,
    },
    Function {
        name: Option<String>,
        params: Vec<Param>,
        body: Vec<Stmt>,
        method: bool,
    },
    AsyncFunction {
        name: Option<String>,
        params: Vec<Param>,
        body: Vec<Stmt>,
        method: bool,
    },
    AsyncArrow {
        params: Vec<Param>,
        body: Vec<Stmt>,
    },
    GeneratorFunction {
        name: Option<String>,
        params: Vec<Param>,
        body: Vec<Stmt>,
        method: bool,
        asynchronous: bool,
    },
    Yield(Option<Box<Expr>>, bool),
    Await(Box<Expr>),
    Spread(Box<Expr>),
    Sequence(Vec<Expr>),
    Object(Vec<(String, Property)>),
    Array(Vec<Option<Expr>>),
    Member(Box<Expr>, String),
    Computed(Box<Expr>, Box<Expr>),
    Call(Box<Expr>, Vec<Expr>),
    New(Box<Expr>, Vec<Expr>),
    Unary(String, Box<Expr>),
    Binary(String, Box<Expr>, Box<Expr>),
    Assign(Box<Expr>, Box<Expr>),
    Destructure(Box<Pattern>, Box<Expr>),
    Compound(String, Box<Expr>, Box<Expr>),
    Update(Box<Expr>, bool, bool),
    Conditional(Box<Expr>, Box<Expr>, Box<Expr>),
    Class(Box<Class>),
    PrivateName(String),
    SuperMember(String),
    SuperComputed(Box<Expr>),
    SuperCall(Vec<Expr>),
    NewTarget,
    ClassInit(Box<ClassInit>),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Class {
    pub name: Option<String>,
    pub heritage: Option<Expr>,
    pub constructor: Option<(Vec<Param>, Vec<Stmt>)>,
    pub members: Vec<ClassMember>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassMember {
    pub is_static: bool,
    pub key: ClassKey,
    pub kind: MemberKind,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClassKey {
    Named(String),
    Computed(Expr),
    Private(String),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemberKind {
    Method(Expr),
    Getter(Vec<Stmt>),
    Setter(Param, Vec<Stmt>),
    Field(Option<Expr>),
    Block(Vec<Stmt>),
}
/// Internal statements of the synthesized field-initializer functions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClassInit {
    /// Key is Named, Private, or Computed(Name("%k{n}")) read from the class scope.
    Field(ClassKey, Option<Expr>),
    /// Install the private method or accessor "#x" from the class scope.
    PrivateMethod(String),
}
impl Expr {
    pub fn anonymous_function(&self) -> bool {
        matches!(
            self,
            Self::Arrow { .. }
                | Self::AsyncArrow { .. }
                | Self::Function { name: None, .. }
                | Self::AsyncFunction { name: None, .. }
                | Self::GeneratorFunction { name: None, .. }
        ) || matches!(self, Self::Class(class) if class.name.is_none())
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Property {
    CoverDefault(String, Expr),
    Computed(Expr, Box<Property>),
    Spread(Expr),
    Value(Expr),
    Prototype(Expr),
    Getter(Vec<Stmt>),
    Setter(Param, Vec<Stmt>),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stmt {
    Empty,
    /// Marker: the enclosing script or function body is sloppy (non-strict) code.
    Sloppy,
    With(Expr, Box<Stmt>),
    Declarations(Vec<Stmt>),
    Bind {
        kind: String,
        pattern: Pattern,
        init: Option<Expr>,
    },
    Switch(Expr, Vec<(Option<Expr>, Vec<Stmt>)>),
    While {
        test: Expr,
        body: Box<Stmt>,
        post: bool,
    },
    For {
        init: Option<Box<Stmt>>,
        test: Option<Expr>,
        update: Option<Expr>,
        body: Box<Stmt>,
    },
    Const(String, Expr),
    Let(String, Expr),
    Var(String, Option<Expr>),
    Function(String, Vec<Param>, Vec<Stmt>),
    GeneratorFunction(String, Vec<Param>, Vec<Stmt>, bool),
    AsyncFunction(String, Vec<Param>, Vec<Stmt>),
    Set(Expr, String, Expr),
    Expr(Expr),
    Return(Expr),
    Throw(Expr),
    Block(Vec<Stmt>),
    If(Expr, Box<Stmt>, Option<Box<Stmt>>),
    /// `break` or `break label`. `continue label` is desugared to a break of a
    /// synthetic label (see `continue_label`) wrapped around the loop body.
    Break(Option<String>),
    Continue,
    Labeled(String, Box<Stmt>),
    ForIn {
        kind: String,
        binding: Pattern,
        iterable: Expr,
        body: Box<Stmt>,
    },
    ForOf {
        kind: String,
        binding: Pattern,
        iterable: Expr,
        body: Box<Stmt>,
        asynchronous: bool,
    },
    Try {
        body: Vec<Stmt>,
        catch: Option<(Option<Pattern>, Vec<Stmt>)>,
        finally: Option<Vec<Stmt>>,
    },
}
/// Expand declaration lists without introducing a block or crossing a scope boundary.
/// The statement under any `label:` prefixes.
pub fn strip_labels(mut statement: &Stmt) -> &Stmt {
    while let Stmt::Labeled(_, inner) = statement {
        statement = inner;
    }
    statement
}
/// The synthetic label a `continue label` breaks to (it wraps the loop body).
pub(super) fn continue_label(label: &str) -> String {
    format!("\0continue {label}")
}
pub fn direct_statements(body: &[Stmt]) -> Vec<&Stmt> {
    let mut out = vec![];
    for statement in body {
        match strip_labels(statement) {
            Stmt::Declarations(items) => out.extend(direct_statements(items)),
            other => out.push(other),
        }
    }
    out
}
impl Stmt {
    pub fn lexical_names(&self) -> Vec<String> {
        direct_statements(std::slice::from_ref(self))
            .into_iter()
            .flat_map(|s| match s {
                Stmt::Let(name, _) | Stmt::Const(name, _) => vec![name.clone()],
                Stmt::Bind { kind, pattern, .. } if kind != "var" => pattern.names(),
                _ => vec![],
            })
            .collect()
    }
    pub fn iteration_names(&self) -> Vec<String> {
        direct_statements(std::slice::from_ref(self))
            .into_iter()
            .flat_map(|s| match s {
                Stmt::Let(name, _) => vec![name.clone()],
                Stmt::Bind { kind, pattern, .. } if kind == "let" => pattern.names(),
                _ => vec![],
            })
            .collect()
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    Template {
        start: bool,
        cooked: Option<String>,
        raw: String,
        tail: bool,
    },
    Name(String),
    /// An identifier written with `\u` escapes that spells a keyword: never a keyword.
    EscapedName(String),
    RegExp {
        pattern: String,
        flags: String,
    },
    String(String),
    Number(String),
    BigInt(String),
    Punct(char),
    Op(String),
    PrivateName(String),
    End,
}
struct Parser<'a> {
    tokens: Vec<(Token, usize)>,
    cursor: usize,
    depth: usize,
    source: &'a str,
    /// Per enclosing class body: private names referenced inside it.
    private_refs: Vec<Vec<String>>,
    super_property: bool,
    super_call: bool,
    /// Strict mode code (directive prologue, class body, module, or inherited).
    strict: bool,
    /// Offsets of legacy octal/non-octal-decimal literals (rejected in strict code).
    legacy: HashSet<usize>,
    legacy_hit: Option<usize>,
    /// Sloppy duplicate simple parameters seen by the last `params()`.
    duplicates: bool,
    /// Enclosing labels of the current function body: (name, labels a loop, continued).
    labels: Vec<(String, bool, bool)>,
}
pub(super) fn tokenize(source: &str) -> Result<Vec<(Token, usize)>, SyntaxError> {
    tokenize_legacy(source).map(|(tokens, _)| tokens)
}
/// Tokens plus the offsets of legacy literals (`010`, `08`, `"\07"`): early errors in strict code.
pub(super) fn tokenize_legacy(
    source: &str,
) -> Result<(Vec<(Token, usize)>, std::collections::HashSet<usize>), SyntaxError> {
    let mut legacy = std::collections::HashSet::new();
    if source.len() > 8 << 20 {
        return Err(SyntaxError {
            offset: 0,
            message: "source limit",
        });
    }
    let mut tokens = vec![];
    let mut templates: Vec<usize> = vec![];
    // Per open `{`: does it open an expression (object literal)? Decides `}` then `/`.
    let mut braces: Vec<bool> = vec![];
    let mut closed_expression = false;
    let mut chars = source.char_indices().peekable();
    while let Some((offset, c)) = chars.next() {
        if super::strings::whitespace(c) {
            continue;
        }
        let error = |message| SyntaxError { offset, message };
        let token = match c {
            '`' => {
                let (cooked, raw, tail) = super::template_lex::segment(source, &mut chars, offset)?;
                if !tail {
                    if templates.len() >= 64 {
                        return Err(error("template nesting limit"));
                    }
                    templates.push(0);
                }
                Token::Template {
                    start: true,
                    cooked,
                    raw,
                    tail,
                }
            }
            '}' if templates.last() == Some(&0) => {
                let (cooked, raw, tail) = super::template_lex::segment(source, &mut chars, offset)?;
                if tail {
                    templates.pop();
                }
                Token::Template {
                    start: false,
                    cooked,
                    raw,
                    tail,
                }
            }
            '{' => {
                if let Some(depth) = templates.last_mut() {
                    *depth += 1;
                }
                braces.push(
                    brace_opens_expression(tokens.last().map(|(t, _)| t))
                        || function_expression_body(&tokens),
                );
                Token::Punct(c)
            }
            '}' => {
                if let Some(depth) = templates.last_mut() {
                    *depth -= 1;
                }
                closed_expression = braces.pop().unwrap_or(false);
                Token::Punct(c)
            }
            '/' if chars.peek().is_some_and(|(_, c)| *c == '/' || *c == '*') => {
                let style = chars.next().unwrap().1;
                if style == '/' {
                    for (_, c) in chars.by_ref() {
                        if matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}') {
                            break;
                        }
                    }
                } else {
                    let mut end = false;
                    while let Some((_, c)) = chars.next() {
                        if c == '*' && chars.peek().is_some_and(|(_, c)| *c == '/') {
                            chars.next();
                            end = true;
                            break;
                        }
                    }
                    if !end {
                        return Err(error("unterminated comment"));
                    }
                }
                continue;
            }
            '/' if regexp_allowed(tokens.last().map(|(t, _)| t), closed_expression) => {
                let (pattern, flags) = super::identifiers::regexp(&mut chars, offset)?;
                Token::RegExp { pattern, flags }
            }
            c if super::identifiers::starts_identifier(c) => {
                let (n, escaped) = super::identifiers::identifier(&mut chars, c, offset)?;
                if escaped && (reserved(&n) || contextual_keyword(&n)) {
                    Token::EscapedName(n)
                } else {
                    Token::Name(n)
                }
            }
            '#' if chars
                .peek()
                .is_some_and(|(_, c)| super::identifiers::starts_identifier(*c)) =>
            {
                let first = chars.next().unwrap().1;
                let (n, _) = super::identifiers::identifier(&mut chars, first, offset)?;
                Token::PrivateName(format!("#{n}"))
            }
            '0'..='9' | '.'
                if c != '.' || chars.peek().is_some_and(|(_, c)| c.is_ascii_digit()) =>
            {
                let mut raw = c.to_string();
                let mut radix = 10;
                if c == '0'
                    && chars
                        .peek()
                        .is_some_and(|(_, c)| matches!(c, 'x' | 'X' | 'o' | 'O' | 'b' | 'B'))
                {
                    let prefix = chars.next().unwrap().1;
                    raw.push(prefix);
                    radix = match prefix {
                        'x' | 'X' => 16,
                        'o' | 'O' => 8,
                        _ => 2,
                    };
                    while chars
                        .peek()
                        .is_some_and(|(_, c)| c.is_ascii_alphanumeric() || *c == '_')
                    {
                        raw.push(chars.next().unwrap().1);
                    }
                } else {
                    while chars.peek().is_some_and(|(_, c)| {
                        c.is_ascii_digit()
                            || *c == '_'
                            || *c == '.' && !raw.contains(['.', 'e', 'E'])
                            || matches!(c, 'e' | 'E') && !raw.contains(['e', 'E'])
                            || matches!(c, '+' | '-') && raw.ends_with(['e', 'E'])
                    }) {
                        raw.push(chars.next().unwrap().1);
                    }
                    if chars.peek().is_some_and(|(_, c)| *c == 'n') {
                        raw.push(chars.next().unwrap().1);
                    }
                }
                let big = raw.ends_with('n');
                if big {
                    raw.pop();
                }
                let digits = if radix == 10 { raw.as_str() } else { &raw[2..] };
                let bytes = digits.as_bytes();
                for (i, c) in bytes.iter().enumerate() {
                    if *c == b'_'
                        && (i == 0
                            || i + 1 == bytes.len()
                            || !(bytes[i - 1] as char).is_digit(radix)
                            || !(bytes[i + 1] as char).is_digit(radix))
                    {
                        return Err(error("invalid numeric separator"));
                    }
                }
                let mut clean = raw.replace('_', "");
                if radix == 10
                    && clean.len() > 1
                    && clean.starts_with('0')
                    && clean.as_bytes()[1].is_ascii_digit()
                {
                    // Sloppy only: LegacyOctalIntegerLiteral or NonOctalDecimalIntegerLiteral.
                    let integer = raw.split(['.', 'e', 'E']).next().unwrap();
                    let octal = integer.bytes().all(|b| (b'0'..=b'7').contains(&b));
                    if big || raw.contains('_') || octal && integer.len() != raw.len() {
                        return Err(error("invalid legacy numeric literal"));
                    }
                    legacy.insert(offset);
                    if octal {
                        let value = integer[1..]
                            .bytes()
                            .fold(0f64, |n, b| n * 8.0 + (b - b'0') as f64);
                        clean = value.to_string();
                    }
                }
                if big || radix != 10 {
                    if big && radix == 10 && clean.contains(['.', 'e', 'E']) {
                        return Err(error("invalid BigInt literal"));
                    }
                    let value = super::bigint::Big::parse(&clean)
                        .map_err(|_| error("invalid or oversized integer literal"))?;
                    if big {
                        Token::BigInt(clean)
                    } else {
                        Token::Number(value.to_float().to_string())
                    }
                } else {
                    if clean.parse::<f64>().is_err() {
                        return Err(error("invalid numeric literal"));
                    }
                    Token::Number(clean)
                }
            }
            '\'' | '"' => {
                let start = offset + 1;
                let mut end = None;
                while let Some((at, ch)) = chars.next() {
                    if ch == c {
                        end = Some(at);
                        break;
                    }
                    if ch == '\n' || ch == '\r' {
                        return Err(error("newline in string"));
                    }
                    if ch == '\\' {
                        // A CRLF line continuation is consumed as one terminator below.
                        if let Some((_, '\r')) = chars.next() {
                            if chars.peek().is_some_and(|(_, c)| *c == '\n') {
                                chars.next();
                            }
                        }
                    }
                }
                let end = end.ok_or(error("unterminated string"))?;
                let raw = &source[start..end];
                let normalized;
                let raw = if raw.contains('\r') {
                    normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
                    normalized.as_str()
                } else {
                    raw
                };
                // Legacy octal and \8 \9 escapes: early errors in strict code (the parser checks).
                let mut old = false;
                match super::template_lex::cook_legacy(raw, &mut old) {
                    Ok(Some(s)) => {
                        if old {
                            legacy.insert(offset);
                        }
                        Token::String(s)
                    }
                    Ok(None) => return Err(error("invalid escape")),
                    Err(()) => return Err(error("surrogate strings unsupported")),
                }
            }
            '.' if source[offset..].starts_with("...") => {
                chars.next();
                chars.next();
                Token::Op("...".into())
            }
            '(' | ')' | '[' | ']' | '.' | ',' | ';' | ':' => Token::Punct(c),
            '?' if !chars.peek().is_some_and(|(_, c)| *c == '?') => Token::Punct('?'),
            '=' | '!' | '<' | '>' | '+' | '-' | '*' | '/' | '%' | '&' | '|' | '^' | '~' | '?' => {
                let mut op = c.to_string();
                const OPS: &[&str] = &[
                    "===", "!==", ">>>", "**", "<<", ">>", "==", "!=", "<=", ">=", "&&", "||",
                    "=>", "++", "--", "+=", "-=", "*=", "/=", "%=", "**=", "<<=", ">>=", ">>>=",
                    "&=", "|=", "^=", "&&=", "||=", "??=",
                ];
                loop {
                    let Some((_, next)) = chars.peek() else { break };
                    let candidate = format!("{op}{next}");
                    if !OPS.iter().any(|known| known.starts_with(&candidate)) {
                        break;
                    }
                    op.push(chars.next().unwrap().1);
                }
                if op == "=>"
                    && tokens.last().is_some_and(|(_, last)| {
                        source[*last..offset].contains(['\n', '\r', '\u{2028}', '\u{2029}'])
                    })
                {
                    return Err(error("newline before arrow"));
                }
                Token::Op(op)
            }
            _ => return Err(error("unsupported character")),
        };
        tokens.push((token, offset));
        if tokens.len() > 1 << 20 {
            return Err(error("token limit"));
        }
    }
    if !templates.is_empty() {
        return Err(SyntaxError {
            offset: source.len(),
            message: "unterminated template substitution",
        });
    }
    tokens.push((Token::End, source.len()));
    let tokens = (tokens, legacy);
    Ok(tokens)
}
pub fn parse(source: &str) -> Result<Vec<Stmt>, SyntaxError> {
    parse_eval(source, false, false, false)
}
/// Script grammar with the context of a direct eval: inherited strictness and
/// whether `super.x` / `super()` are allowed.
pub(super) fn parse_eval(
    source: &str,
    strict: bool,
    super_property: bool,
    super_call: bool,
) -> Result<Vec<Stmt>, SyntaxError> {
    let (tokens, legacy) = tokenize_legacy(source)?;
    let mut p = Parser::new(tokens, legacy, source, strict);
    p.super_property = super_property;
    p.super_call = super_call;
    p.prologue(0)?;
    let mut body = p.statements(false)?;
    p.legacy_check()?;
    if !p.strict {
        body.insert(0, Stmt::Sloppy);
    }
    validate(&body, &[], false, 0)?;
    Ok(body)
}
impl<'a> Parser<'a> {
    pub(super) fn new(
        tokens: Vec<(Token, usize)>,
        legacy: HashSet<usize>,
        source: &'a str,
        strict: bool,
    ) -> Self {
        Parser {
            tokens,
            cursor: 0,
            depth: 0,
            source,
            private_refs: vec![],
            super_property: false,
            super_call: false,
            strict,
            legacy,
            legacy_hit: None,
            duplicates: false,
            labels: vec![],
        }
    }
    pub(super) fn legacy_check(&self) -> Result<(), SyntaxError> {
        match self.legacy_hit {
            Some(offset) => Err(SyntaxError {
                offset,
                message: "legacy literal in strict mode",
            }),
            None => Ok(()),
        }
    }
    /// Scans the directive prologue starting at token `start` (without consuming it);
    /// a raw `"use strict"` directive makes the enclosing code strict.
    fn prologue(&mut self, start: usize) -> Result<(), SyntaxError> {
        let mut i = start;
        let mut offsets = vec![];
        while let (Token::String(text), offset) = &self.tokens[i] {
            let next = &self.tokens[i + 1];
            let terminated = matches!(next.0, Token::Punct(';') | Token::Punct('}') | Token::End)
                || self.source[*offset..next.1].contains(['\n', '\r', '\u{2028}', '\u{2029}'])
                    && !matches!(next.0, Token::Punct('.' | '(' | '[' | ',' | '?' | '`'))
                    && !matches!(&next.0, Token::Op(_));
            if !terminated {
                break;
            }
            offsets.push(*offset);
            let raw = &self.source[*offset..];
            if text == "use strict"
                && (raw.starts_with("\"use strict\"") || raw.starts_with("'use strict'"))
            {
                self.strict = true;
            }
            i += 1;
            if self.tokens[i].0 == Token::Punct(';') {
                i += 1;
            }
        }
        if self.strict {
            if let Some(offset) = offsets.into_iter().find(|o| self.legacy.contains(o)) {
                return Err(SyntaxError {
                    offset,
                    message: "legacy literal in strict mode",
                });
            }
        }
        Ok(())
    }
    /// A function body block: its own directive prologue; a sloppy body starts with
    /// the `Stmt::Sloppy` marker the runtime reads strictness from.
    fn function_body(&mut self) -> Result<Vec<Stmt>, SyntaxError> {
        let saved = self.strict;
        if self.current() == &Token::Punct('{') {
            self.prologue(self.cursor + 1)?;
        }
        let labels = std::mem::take(&mut self.labels);
        let result = self.block();
        self.labels = labels;
        let strict = std::mem::replace(&mut self.strict, saved);
        let mut body = result?;
        if !strict {
            body.insert(0, Stmt::Sloppy);
        }
        Ok(body)
    }
}
impl Parser<'_> {
    fn current(&self) -> &Token {
        &self.tokens[self.cursor].0
    }
    fn error(&self, message: &'static str) -> SyntaxError {
        SyntaxError {
            offset: self.tokens[self.cursor].1,
            message,
        }
    }
    fn take(&mut self) -> Token {
        let t = self.current().clone();
        if self.strict
            && self.legacy_hit.is_none()
            && self.legacy.contains(&self.tokens[self.cursor].1)
        {
            self.legacy_hit = Some(self.tokens[self.cursor].1);
        }
        self.cursor += 1;
        t
    }
    fn punct(&mut self, c: char) -> bool {
        if self.current() == &Token::Punct(c) {
            self.cursor += 1;
            true
        } else {
            false
        }
    }
    fn op(&mut self, s: &str) -> bool {
        if self.current() == &Token::Op(s.into()) {
            self.cursor += 1;
            true
        } else {
            false
        }
    }
    fn word(&mut self, s: &str) -> bool {
        if self.current() == &Token::Name(s.into()) {
            self.cursor += 1;
            true
        } else {
            false
        }
    }
    fn expect(&mut self, c: char) -> Result<(), SyntaxError> {
        if self.punct(c) {
            Ok(())
        } else {
            Err(self.error("expected punctuation"))
        }
    }
    fn name(&mut self) -> Result<String, SyntaxError> {
        if let Token::Name(n) = self.current().clone() {
            if reserved(&n) {
                return Err(self.error("reserved binding identifier"));
            }
            self.take();
            Ok(n)
        } else if let Token::EscapedName(n) = self.current().clone() {
            if reserved(&n) {
                return Err(self.error("escaped reserved word"));
            }
            self.take();
            Ok(n)
        } else {
            Err(self.error("expected identifier"))
        }
    }
    fn key(&mut self) -> Result<String, SyntaxError> {
        match self.take() {
            Token::Name(s) | Token::EscapedName(s) | Token::String(s) => Ok(s),
            Token::Number(s) => Ok(super::execute::number_string(s.parse().unwrap())),
            Token::BigInt(s) => super::bigint::Big::parse(&s)
                .map(|n| n.radix_string(10))
                .map_err(|_| self.error("invalid BigInt property name")),
            _ => Err(self.error("unsupported property key")),
        }
    }
    fn newline(&self) -> bool {
        self.cursor > 0
            && self.source[self.tokens[self.cursor - 1].1..self.tokens[self.cursor].1]
                .contains(['\n', '\r', '\u{2028}', '\u{2029}'])
    }
    fn semi(&mut self) -> Result<(), SyntaxError> {
        if self.punct(';')
            || matches!(self.current(), Token::End | Token::Punct('}'))
            || self.newline()
        {
            Ok(())
        } else {
            Err(self.error("expected semicolon"))
        }
    }
    fn statements(&mut self, block: bool) -> Result<Vec<Stmt>, SyntaxError> {
        let mut body = vec![];
        while self.current() != &Token::End {
            if self.punct('}') {
                return if block {
                    Ok(body)
                } else {
                    Err(self.error("unexpected closing brace"))
                };
            }
            if self.punct(';') {
                continue;
            }
            body.push(self.statement()?);
        }
        if block {
            Err(self.error("unterminated block"))
        } else {
            Ok(body)
        }
    }
    fn block(&mut self) -> Result<Vec<Stmt>, SyntaxError> {
        if self.depth >= 64 {
            return Err(self.error("syntax nesting limit"));
        }
        self.depth += 1;
        let r = self.expect('{').and_then(|_| self.statements(true));
        self.depth -= 1;
        r
    }
    fn binding_pattern(&mut self) -> Result<Pattern, SyntaxError> {
        if self.depth >= 64 {
            return Err(self.error("binding nesting limit"));
        }
        self.depth += 1;
        let result = self.binding_pattern_inner();
        self.depth -= 1;
        result
    }
    fn binding_element(&mut self) -> Result<Pattern, SyntaxError> {
        let pattern = self.binding_pattern()?;
        if self.op("=") {
            Ok(Pattern::Default(Box::new(pattern), self.expression()?))
        } else {
            Ok(pattern)
        }
    }
    fn binding_pattern_inner(&mut self) -> Result<Pattern, SyntaxError> {
        if self.punct('[') {
            let mut items = vec![];
            let mut rest = None;
            while !self.punct(']') {
                if self.punct(',') {
                    items.push(None);
                    continue;
                }
                if self.op("...") {
                    rest = Some(Box::new(self.binding_pattern()?));
                    self.expect(']')?;
                    break;
                }
                items.push(Some(self.binding_element()?));
                if self.punct(']') {
                    break;
                }
                self.expect(',')?;
            }
            return Ok(Pattern::Array(items, rest));
        }
        if self.punct('{') {
            let mut properties = vec![];
            let mut rest = None;
            while !self.punct('}') {
                if self.op("...") {
                    rest = Some(Box::new(Pattern::Name(self.name()?)));
                    self.expect('}')?;
                    break;
                }
                let (key, pattern) = if self.punct('[') {
                    let expression = self.sequence()?;
                    self.expect(']')?;
                    self.expect(':')?;
                    (PatternKey::Computed(expression), self.binding_element()?)
                } else {
                    let shorthand =
                        matches!(self.current(), Token::Name(_) | Token::EscapedName(_));
                    let key = self.key()?;
                    let pattern = if self.punct(':') {
                        self.binding_element()?
                    } else {
                        if !shorthand || reserved(&key) {
                            return Err(self.error("invalid shorthand binding"));
                        }
                        let pattern = Pattern::Name(key.clone());
                        if self.op("=") {
                            Pattern::Default(Box::new(pattern), self.expression()?)
                        } else {
                            pattern
                        }
                    };
                    (PatternKey::Static(key), pattern)
                };
                properties.push((key, pattern));
                if self.punct('}') {
                    break;
                }
                self.expect(',')?;
            }
            return Ok(Pattern::Object(properties, rest));
        }
        self.name().map(Pattern::Name)
    }
    fn binding_statement(kind: &str, pattern: Pattern, init: Option<Expr>) -> Stmt {
        match pattern {
            Pattern::Name(name) => match kind {
                "const" => Stmt::Const(name, init.unwrap()),
                "let" => Stmt::Let(name, init.unwrap_or(Expr::Undefined)),
                _ => Stmt::Var(name, init),
            },
            pattern => Stmt::Bind {
                kind: kind.into(),
                pattern,
                init,
            },
        }
    }
    fn declaration_tail(&mut self, kind: &str, first: Pattern) -> Result<Stmt, SyntaxError> {
        let mut declarations = vec![];
        let mut pattern = first;
        loop {
            let init = if self.op("=") {
                Some(self.expression()?)
            } else {
                None
            };
            if init.is_none() && (kind == "const" || !matches!(pattern, Pattern::Name(_))) {
                return Err(self.error("binding declaration requires initializer"));
            }
            declarations.push(Self::binding_statement(kind, pattern, init));
            if !self.punct(',') {
                break;
            }
            pattern = self.binding_pattern()?;
        }
        Ok(if declarations.len() == 1 {
            declarations.pop().unwrap()
        } else {
            Stmt::Declarations(declarations)
        })
    }
    fn literal_key(&mut self) -> Result<(String, Option<Expr>), SyntaxError> {
        if self.punct('[') {
            let key = self.sequence()?;
            self.expect(']')?;
            Ok((String::new(), Some(key)))
        } else {
            self.key().map(|key| (key, None))
        }
    }
    fn object_property(&mut self, prototype: &mut bool) -> Result<(String, Property), SyntaxError> {
        if self.op("...") {
            return Ok((String::new(), Property::Spread(self.expression()?)));
        }
        let mut generator = self.op("*");
        let identifier = matches!(self.current(), Token::Name(_) | Token::EscapedName(_));
        let (mut key, mut computed) = self.literal_key()?;
        let asynchronous = !generator
            && computed.is_none()
            && key == "async"
            && !self.newline()
            && (matches!(
                self.current(),
                Token::Name(_)
                    | Token::EscapedName(_)
                    | Token::String(_)
                    | Token::Number(_)
                    | Token::BigInt(_)
                    | Token::Punct('[')
            ) || self.current() == &Token::Op("*".into()));
        if asynchronous {
            generator = self.op("*");
            (key, computed) = self.literal_key()?;
        }
        let property = if !generator && !asynchronous && self.punct(':') {
            let value = self.expression()?;
            if computed.is_none() && key == "__proto__" {
                if *prototype {
                    return Err(self.error("duplicate prototype literal"));
                }
                *prototype = true;
                Property::Prototype(value)
            } else {
                Property::Value(value)
            }
        } else if !generator
            && !asynchronous
            && computed.is_none()
            && (key == "get" || key == "set")
            && !matches!(
                self.current(),
                Token::Punct('(') | Token::Punct(',') | Token::Punct('}')
            )
        {
            let getter = key == "get";
            (key, computed) = self.literal_key()?;
            let (params, body) = self.function_rest(true, false)?;
            match (getter, params.as_slice()) {
                (true, []) => Property::Getter(body),
                (false, [param]) if !param.rest && param.default.is_none() => {
                    Property::Setter(param.clone(), body)
                }
                _ => return Err(self.error("invalid accessor parameters")),
            }
        } else if generator || asynchronous || self.current() == &Token::Punct('(') {
            let (params, body) = self.function_rest(true, false)?;
            Property::Value(if generator {
                Expr::GeneratorFunction {
                    name: None,
                    params,
                    body,
                    method: true,
                    asynchronous,
                }
            } else if asynchronous {
                Expr::AsyncFunction {
                    name: None,
                    params,
                    body,
                    method: true,
                }
            } else {
                Expr::Function {
                    name: None,
                    params,
                    body,
                    method: true,
                }
            })
        } else {
            if computed.is_some() || !identifier {
                return Err(self.error("invalid shorthand property"));
            }
            if self.op("=") {
                Property::CoverDefault(key.clone(), self.expression()?)
            } else {
                Property::Value(Expr::Name(key.clone()))
            }
        };
        Ok((
            key,
            if let Some(expression) = computed {
                Property::Computed(expression, Box::new(property))
            } else {
                property
            },
        ))
    }
    /// Parameters and body of a function, with the given super context.
    fn function_rest(
        &mut self,
        super_property: bool,
        super_call: bool,
    ) -> Result<(Vec<Param>, Vec<Stmt>), SyntaxError> {
        let saved = (self.super_property, self.super_call);
        self.super_property = super_property;
        self.super_call = super_call;
        let sloppy_ordinary = !super_property && !super_call;
        let result = self.params().and_then(|params| {
            let duplicates = std::mem::take(&mut self.duplicates);
            if duplicates && !sloppy_ordinary {
                return Err(self.error("duplicate parameter"));
            }
            let body = self.function_body()?;
            if duplicates && !matches!(body.first(), Some(Stmt::Sloppy)) {
                return Err(self.error("duplicate parameter"));
            }
            Ok((params, body))
        });
        (self.super_property, self.super_call) = saved;
        result
    }
    fn private_reference(&mut self, name: &str) -> Result<(), SyntaxError> {
        match self.private_refs.last_mut() {
            Some(refs) => {
                refs.push(name.to_string());
                Ok(())
            }
            None => Err(self.error("undeclared private name")),
        }
    }
    fn member_after_dot(&mut self, e: Expr) -> Result<Expr, SyntaxError> {
        if let Token::PrivateName(name) = self.current().clone() {
            self.take();
            self.private_reference(&name)?;
            return Ok(Expr::Computed(
                Box::new(e),
                Box::new(Expr::PrivateName(name)),
            ));
        }
        Ok(Expr::Member(Box::new(e), self.key()?))
    }
    fn class_tail(&mut self, name: Option<String>) -> Result<Class, SyntaxError> {
        let saved = std::mem::replace(&mut self.strict, true);
        let result = self.class_tail_strict(name);
        self.strict = saved;
        result
    }
    fn class_tail_strict(&mut self, name: Option<String>) -> Result<Class, SyntaxError> {
        let heritage = if self.word("extends") {
            let saved = (self.super_property, self.super_call);
            let base = self.primary()?;
            let heritage = self.postfix(base);
            (self.super_property, self.super_call) = saved;
            Some(heritage?)
        } else {
            None
        };
        let derived = heritage.is_some();
        self.expect('{')?;
        if self.depth >= 64 {
            return Err(self.error("syntax nesting limit"));
        }
        self.depth += 1;
        self.private_refs.push(vec![]);
        let body = self.class_body(derived);
        self.depth -= 1;
        let refs = self.private_refs.pop().unwrap_or_default();
        let (constructor, members) = body?;
        let declared: Vec<&str> = members
            .iter()
            .filter_map(|m| match &m.key {
                ClassKey::Private(n) => Some(n.as_str()),
                _ => None,
            })
            .collect();
        for name in refs {
            if !declared.contains(&name.as_str()) {
                match self.private_refs.last_mut() {
                    Some(outer) => outer.push(name),
                    None => return Err(self.error("undeclared private name")),
                }
            }
        }
        Ok(Class {
            name,
            heritage,
            constructor,
            members,
        })
    }
    #[allow(clippy::type_complexity)]
    fn class_body(
        &mut self,
        derived: bool,
    ) -> Result<(Option<(Vec<Param>, Vec<Stmt>)>, Vec<ClassMember>), SyntaxError> {
        let mut constructor = None;
        let mut members: Vec<ClassMember> = vec![];
        // (name, static, 0 = other, 1 = getter, 2 = setter)
        let mut privates: Vec<(String, bool, u8)> = vec![];
        let starts_key = |t: &Token| {
            matches!(
                t,
                Token::Name(_)
                    | Token::EscapedName(_)
                    | Token::String(_)
                    | Token::Number(_)
                    | Token::BigInt(_)
                    | Token::PrivateName(_)
                    | Token::Punct('[')
            ) || t == &Token::Op("*".into())
        };
        loop {
            if self.punct('}') {
                break;
            }
            if self.punct(';') {
                continue;
            }
            if self.current() == &Token::End {
                return Err(self.error("unterminated class body"));
            }
            let next = self.tokens.get(self.cursor + 1).map(|t| t.0.clone());
            let mut is_static = false;
            if self.current() == &Token::Name("static".into())
                && next
                    .as_ref()
                    .is_some_and(|t| starts_key(t) || t == &Token::Punct('{'))
            {
                self.take();
                is_static = true;
                if self.current() == &Token::Punct('{') {
                    let saved = (self.super_property, self.super_call);
                    self.super_property = true;
                    self.super_call = false;
                    let labels = std::mem::take(&mut self.labels);
                    let body = self.block();
                    self.labels = labels;
                    (self.super_property, self.super_call) = saved;
                    members.push(ClassMember {
                        is_static: true,
                        key: ClassKey::Named(String::new()),
                        kind: MemberKind::Block(body?),
                    });
                    continue;
                }
            }
            let next = self.tokens.get(self.cursor + 1).map(|t| t.0.clone());
            let mut generator = self.op("*");
            let mut asynchronous = false;
            let mut accessor = None;
            if !generator
                && self.current() == &Token::Name("async".into())
                && next.as_ref().is_some_and(starts_key)
                && !self.source[self.tokens[self.cursor].1..self.tokens[self.cursor + 1].1]
                    .contains(['\n', '\r', '\u{2028}', '\u{2029}'])
            {
                self.take();
                asynchronous = true;
                generator = self.op("*");
            } else if !generator
                && matches!(self.current(), Token::Name(n) if n == "get" || n == "set")
                && next
                    .as_ref()
                    .is_some_and(|t| starts_key(t) && t != &Token::Op("*".into()))
            {
                accessor = Some(self.take() == Token::Name("get".into()));
            }
            let key = match self.current().clone() {
                Token::PrivateName(n) => {
                    self.take();
                    if n == "#constructor" {
                        return Err(self.error("invalid private name #constructor"));
                    }
                    ClassKey::Private(n)
                }
                Token::Punct('[') => {
                    self.take();
                    let saved = (self.super_property, self.super_call);
                    let key = self.expression();
                    (self.super_property, self.super_call) = saved;
                    let key = key?;
                    self.expect(']')?;
                    ClassKey::Computed(key)
                }
                _ => ClassKey::Named(self.key()?),
            };
            let named = |k: &ClassKey, n: &str| matches!(k, ClassKey::Named(s) if s == n);
            let method = generator
                || asynchronous
                || accessor.is_some()
                || self.current() == &Token::Punct('(');
            if let ClassKey::Private(n) = &key {
                let tag = match accessor {
                    Some(true) => 1,
                    Some(false) => 2,
                    None => 0,
                };
                if privates.iter().any(|(m, s, t)| {
                    m == n && (tag == 0 || *t == 0 || *t == tag || *s != is_static)
                }) {
                    return Err(self.error("duplicate private name"));
                }
                privates.push((n.clone(), is_static, tag));
            }
            if method {
                if !is_static && named(&key, "constructor") {
                    if generator || asynchronous || accessor.is_some() {
                        return Err(self.error("invalid special constructor method"));
                    }
                    if constructor.is_some() {
                        return Err(self.error("duplicate class constructor"));
                    }
                    constructor = Some(self.function_rest(true, derived)?);
                    continue;
                }
                if is_static && named(&key, "prototype") {
                    return Err(self.error("static prototype method"));
                }
                let (params, body) = self.function_rest(true, false)?;
                let kind = match accessor {
                    Some(true) => {
                        if !params.is_empty() {
                            return Err(self.error("invalid accessor parameters"));
                        }
                        MemberKind::Getter(body)
                    }
                    Some(false) => match params.as_slice() {
                        [param] if !param.rest => MemberKind::Setter(param.clone(), body),
                        _ => return Err(self.error("invalid accessor parameters")),
                    },
                    None => MemberKind::Method(if generator {
                        Expr::GeneratorFunction {
                            name: None,
                            params,
                            body,
                            method: true,
                            asynchronous,
                        }
                    } else if asynchronous {
                        Expr::AsyncFunction {
                            name: None,
                            params,
                            body,
                            method: true,
                        }
                    } else {
                        Expr::Function {
                            name: None,
                            params,
                            body,
                            method: true,
                        }
                    }),
                };
                members.push(ClassMember {
                    is_static,
                    key,
                    kind,
                });
            } else {
                if named(&key, "constructor") || (is_static && named(&key, "prototype")) {
                    return Err(self.error("invalid class field name"));
                }
                let init = if self.op("=") {
                    let saved = (self.super_property, self.super_call);
                    self.super_property = true;
                    self.super_call = false;
                    let value = self.expression();
                    (self.super_property, self.super_call) = saved;
                    Some(value?)
                } else {
                    None
                };
                self.semi()?;
                members.push(ClassMember {
                    is_static,
                    key,
                    kind: MemberKind::Field(init),
                });
            }
        }
        Ok((constructor, members))
    }
    fn params(&mut self) -> Result<Vec<Param>, SyntaxError> {
        self.expect('(')?;
        let mut params: Vec<Param> = vec![];
        if !self.punct(')') {
            loop {
                let rest = self.op("...");
                let binding = self.binding_pattern()?;
                let names = binding.names();
                for (index, name) in names.iter().enumerate() {
                    if names[..index].contains(name)
                        || params.iter().any(|p| p.binding.names().contains(name))
                    {
                        if self.strict || rest || !matches!(binding, Pattern::Name(_)) {
                            return Err(self.error("duplicate parameter"));
                        }
                        self.duplicates = true;
                    }
                }
                let default = if self.op("=") {
                    if rest {
                        return Err(self.error("rest parameter cannot have a default"));
                    }
                    Some(self.expression()?)
                } else {
                    None
                };
                params.push(Param {
                    binding,
                    default,
                    rest,
                });
                if self.punct(')') {
                    break;
                }
                if rest {
                    return Err(self.error("rest parameter must be last"));
                }
                self.expect(',')?;
                if self.punct(')') {
                    break;
                }
            }
        }
        if self.duplicates
            && params
                .iter()
                .any(|p| p.default.is_some() || p.rest || !matches!(p.binding, Pattern::Name(_)))
        {
            return Err(self.error("duplicate parameter"));
        }
        Ok(params)
    }
    fn iteration_assignment_head(&mut self) -> Result<Option<(Pattern, bool)>, SyntaxError> {
        let start = self.cursor;
        let mut depth = 0usize;
        for index in start..self.tokens.len() {
            match &self.tokens[index].0 {
                Token::Punct('(') | Token::Punct('[') | Token::Punct('{') => depth += 1,
                Token::Punct(')') if depth == 0 => break,
                Token::Punct(')') | Token::Punct(']') | Token::Punct('}') => {
                    depth = depth.saturating_sub(1)
                }
                Token::Punct(';') if depth == 0 => break,
                Token::Name(word)
                    if depth == 0 && index > start && (word == "in" || word == "of") =>
                {
                    let enumeration = word == "in";
                    let mut tokens = self.tokens[start..index].to_vec();
                    tokens.push((Token::End, self.tokens[index].1));
                    let mut head =
                        Parser::new(tokens, self.legacy.clone(), self.source, self.strict);
                    head.depth = self.depth;
                    if !self.private_refs.is_empty() {
                        head.private_refs = vec![vec![]];
                    }
                    head.super_property = self.super_property;
                    head.super_call = self.super_call;
                    if let Ok(expression) = head.sequence() {
                        if head.current() == &Token::End {
                            if let Ok(pattern) = head.assignment_pattern(expression) {
                                if let (Some(outer), Some(inner)) =
                                    (self.private_refs.last_mut(), head.private_refs.pop())
                                {
                                    outer.extend(inner);
                                }
                                self.cursor = index + 1;
                                return Ok(Some((pattern, enumeration)));
                            }
                        }
                    }
                }
                Token::End => break,
                _ => {}
            }
        }
        Ok(None)
    }
    fn statement(&mut self) -> Result<Stmt, SyntaxError> {
        if self.depth >= 64 {
            return Err(self.error("syntax nesting limit"));
        }
        self.depth += 1;
        let r = self.statement_inner();
        self.depth -= 1;
        r
    }
    /// `Identifier :` at the start of a statement; consumes it and returns the label.
    fn label_start(&mut self) -> Option<String> {
        let (Token::Name(name) | Token::EscapedName(name)) = self.current() else {
            return None;
        };
        if reserved(name)
            || (self.strict
                && matches!(
                    name.as_str(),
                    "yield"
                        | "let"
                        | "static"
                        | "implements"
                        | "interface"
                        | "package"
                        | "private"
                        | "protected"
                        | "public"
                ))
            || self.tokens.get(self.cursor + 1).map(|t| &t.0) != Some(&Token::Punct(':'))
        {
            return None;
        }
        let name = name.clone();
        self.cursor += 2;
        Some(name)
    }
    /// The optional label after `break` / `continue` (none across a line terminator).
    fn jump_label(&mut self) -> Result<Option<String>, SyntaxError> {
        if self.newline() || !matches!(self.current(), Token::Name(_) | Token::EscapedName(_)) {
            return Ok(None);
        }
        self.name().map(Some)
    }
    /// A labelled statement. Labels of an iteration statement accept `continue`;
    /// each continued label wraps the loop body in its synthetic break label.
    fn labelled(&mut self, first: String) -> Result<Stmt, SyntaxError> {
        let mut names = vec![first];
        while let Some(next) = self.label_start() {
            names.push(next);
        }
        for (i, name) in names.iter().enumerate() {
            if names[..i].contains(name) || self.labels.iter().any(|(n, ..)| n == name) {
                return Err(self.error("duplicate label"));
            }
        }
        let is_loop =
            matches!(self.current(), Token::Name(w) if w == "for" || w == "while" || w == "do");
        if matches!(self.current(), Token::Name(w) if w == "function") {
            let generator = matches!(self.tokens.get(self.cursor + 1).map(|t| &t.0), Some(Token::Op(o)) if o == "*");
            if self.strict || generator {
                return Err(self.error("labelled function declaration"));
            }
        }
        if matches!(self.current(), Token::Name(w) if w == "class" || w == "const")
            || (matches!(self.current(), Token::Name(w) if w == "let")
                && matches!(
                    self.tokens.get(self.cursor + 1).map(|t| &t.0),
                    Some(Token::Punct('['))
                ))
        {
            return Err(self.error("declaration in labelled statement"));
        }
        let mark = self.labels.len();
        self.labels
            .extend(names.iter().map(|name| (name.clone(), is_loop, false)));
        let body = self.statement();
        let labels: Vec<_> = self.labels.drain(mark..).collect();
        let mut body = body?;
        if let Stmt::While { body: inner, .. }
        | Stmt::For { body: inner, .. }
        | Stmt::ForOf { body: inner, .. }
        | Stmt::ForIn { body: inner, .. } = &mut body
        {
            for (name, _, continued) in labels.iter().rev() {
                if *continued {
                    let statement = std::mem::replace(inner.as_mut(), Stmt::Empty);
                    **inner = Stmt::Labeled(continue_label(name), Box::new(statement));
                }
            }
        }
        for name in names.into_iter().rev() {
            body = Stmt::Labeled(name, Box::new(body));
        }
        Ok(body)
    }
    fn statement_inner(&mut self) -> Result<Stmt, SyntaxError> {
        if self.current() == &Token::Punct('{') {
            return Ok(Stmt::Block(self.block()?));
        }
        if let Some(label) = self.label_start() {
            return self.labelled(label);
        }
        if self.word("break") {
            let label = self.jump_label()?;
            if let Some(label) = &label {
                if !self.labels.iter().any(|(name, ..)| name == label) {
                    return Err(self.error("undefined break label"));
                }
            }
            self.semi()?;
            return Ok(Stmt::Break(label));
        }
        if self.word("continue") {
            let Some(label) = self.jump_label()? else {
                self.semi()?;
                return Ok(Stmt::Continue);
            };
            match self
                .labels
                .iter_mut()
                .rev()
                .find(|(name, ..)| *name == label)
            {
                Some((_, true, continued)) => *continued = true,
                Some(_) => return Err(self.error("continue label is not a loop")),
                None => return Err(self.error("undefined continue label")),
            }
            self.semi()?;
            return Ok(Stmt::Break(Some(continue_label(&label))));
        }
        if self.punct(';') {
            return Ok(Stmt::Empty);
        }
        if self.word("debugger") {
            self.semi()?;
            return Ok(Stmt::Empty);
        }
        if self.word("while") {
            self.expect('(')?;
            let test = self.sequence()?;
            self.expect(')')?;
            return Ok(Stmt::While {
                test,
                body: Box::new(self.statement()?),
                post: false,
            });
        }
        if self.word("do") {
            let body = Box::new(self.statement()?);
            if !self.word("while") {
                return Err(self.error("do requires while"));
            }
            self.expect('(')?;
            let test = self.sequence()?;
            self.expect(')')?;
            self.punct(';');
            return Ok(Stmt::While {
                test,
                body,
                post: true,
            });
        }
        if self.word("for") {
            let asynchronous = self.word("await");
            self.expect('(')?;
            let kind = if self.word("const") {
                Some("const")
            } else if self.word("let") {
                Some("let")
            } else if self.word("var") {
                Some("var")
            } else {
                None
            };
            let init = if let Some(kind) = kind {
                let binding = self.binding_pattern()?;
                if self.word("in") {
                    let iterable = self.sequence()?;
                    self.expect(')')?;
                    return Ok(Stmt::ForIn {
                        kind: kind.into(),
                        binding,
                        iterable,
                        body: Box::new(self.statement()?),
                    });
                }
                if self.word("of") {
                    let iterable = self.expression()?;
                    self.expect(')')?;
                    return Ok(Stmt::ForOf {
                        kind: kind.into(),
                        binding,
                        iterable,
                        body: Box::new(self.statement()?),
                        asynchronous,
                    });
                }
                let stmt = self.declaration_tail(kind, binding)?;
                self.expect(';')?;
                Some(Box::new(stmt))
            } else if self.punct(';') {
                None
            } else if let Some((binding, enumeration)) = self.iteration_assignment_head()? {
                let iterable = if enumeration {
                    self.sequence()?
                } else {
                    self.expression()?
                };
                self.expect(')')?;
                let body = Box::new(self.statement()?);
                return Ok(if enumeration {
                    Stmt::ForIn {
                        kind: "assignment".into(),
                        binding,
                        iterable,
                        body,
                    }
                } else {
                    Stmt::ForOf {
                        kind: "assignment".into(),
                        binding,
                        iterable,
                        body,
                        asynchronous,
                    }
                });
            } else {
                let value = self.sequence()?;
                self.expect(';')?;
                Some(Box::new(Stmt::Expr(value)))
            };
            let test = if self.punct(';') {
                None
            } else {
                let value = self.sequence()?;
                self.expect(';')?;
                Some(value)
            };
            let update = if self.punct(')') {
                None
            } else {
                let value = self.sequence()?;
                self.expect(')')?;
                Some(value)
            };
            return Ok(Stmt::For {
                init,
                test,
                update,
                body: Box::new(self.statement()?),
            });
        }
        if self.word("switch") {
            self.expect('(')?;
            let value = self.sequence()?;
            self.expect(')')?;
            self.expect('{')?;
            let mut cases = vec![];
            let mut default = false;
            while !self.punct('}') {
                let test = if self.word("case") {
                    Some(self.sequence()?)
                } else if self.word("default") {
                    if default {
                        return Err(self.error("duplicate switch default"));
                    }
                    default = true;
                    None
                } else {
                    return Err(self.error("expected switch case"));
                };
                self.expect(':')?;
                let mut body = vec![];
                while !matches!(self.current(), Token::Punct('}') | Token::End)
                    && !matches!(self.current(),Token::Name(n) if n=="case"||n=="default")
                {
                    body.push(self.statement()?);
                }
                cases.push((test, body));
            }
            return Ok(Stmt::Switch(value, cases));
        }
        if self.current() == &Token::Name("async".into())
            && self.tokens.get(self.cursor + 1).is_some_and(|(t, offset)| {
                t == &Token::Name("function".into())
                    && !self.source[self.tokens[self.cursor].1..*offset]
                        .contains(['\n', '\r', '\u{2028}', '\u{2029}'])
            })
        {
            self.take();
            self.take();
            let generator = self.op("*");
            let name = self.name()?;
            let (params, body) = self.function_rest(false, false)?;
            if generator {
                return Ok(Stmt::GeneratorFunction(name, params, body, true));
            }
            return Ok(Stmt::AsyncFunction(name, params, body));
        }
        if self.word("class") {
            let name = self.name()?;
            let class = self.class_tail(Some(name.clone()))?;
            return Ok(Stmt::Let(name, Expr::Class(Box::new(class))));
        }
        if self.word("function") {
            let generator = self.op("*");
            let n = self.name()?;
            let (p, body) = self.function_rest(false, false)?;
            return Ok(if generator {
                Stmt::GeneratorFunction(n, p, body, false)
            } else {
                Stmt::Function(n, p, body)
            });
        }
        if self.current() == &Token::Name("with".into()) {
            if self.strict {
                return Err(self.error("with in strict mode"));
            }
            self.take();
            self.expect('(')?;
            let object = self.sequence()?;
            self.expect(')')?;
            return Ok(Stmt::With(object, Box::new(self.statement()?)));
        }
        if self.word("if") {
            self.expect('(')?;
            let c = self.sequence()?;
            self.expect(')')?;
            let a = Box::new(self.statement()?);
            let b = if self.word("else") {
                Some(Box::new(self.statement()?))
            } else {
                None
            };
            return Ok(Stmt::If(c, a, b));
        }
        if self.word("try") {
            let body = self.block()?;
            let catch = if self.word("catch") {
                let binding = if self.punct('(') {
                    let pattern = self.binding_pattern()?;
                    self.expect(')')?;
                    Some(pattern)
                } else {
                    None
                };
                Some((binding, self.block()?))
            } else {
                None
            };
            let finally = if self.word("finally") {
                Some(self.block()?)
            } else {
                None
            };
            if catch.is_none() && finally.is_none() {
                return Err(self.error("try requires catch or finally"));
            }
            return Ok(Stmt::Try {
                body,
                catch,
                finally,
            });
        }
        let s = if self.current() == &Token::Name("const".into())
            || self.current() == &Token::Name("let".into())
            || self.current() == &Token::Name("var".into())
        {
            let Token::Name(kind) = self.take() else {
                unreachable!()
            };
            let pattern = self.binding_pattern()?;
            self.declaration_tail(&kind, pattern)?
        } else if self.word("return") {
            let e = if self.newline()
                || matches!(
                    self.current(),
                    Token::End | Token::Punct(';') | Token::Punct('}')
                ) {
                Expr::Undefined
            } else {
                self.sequence()?
            };
            Stmt::Return(e)
        } else if self.word("throw") {
            if self.newline() {
                return Err(self.error("newline after throw"));
            }
            Stmt::Throw(self.sequence()?)
        } else {
            Stmt::Expr(self.sequence()?)
        };
        self.semi()?;
        Ok(s)
    }
    fn is_arrow(&self) -> bool {
        let mut depth = 0usize;
        for index in self.cursor..self.tokens.len() {
            match &self.tokens[index].0 {
                Token::Punct('(') => depth += 1,
                Token::Punct(')') => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return self.tokens.get(index + 1).is_some_and(|(t, offset)| {
                            matches!(t,Token::Op(op) if op=="=>")
                                && !self.source[self.tokens[index].1..*offset]
                                    .contains(['\n', '\r', '\u{2028}', '\u{2029}'])
                        });
                    }
                }
                Token::End => break,
                _ => {}
            }
        }
        false
    }
    fn sequence(&mut self) -> Result<Expr, SyntaxError> {
        let first = self.expression()?;
        if !self.punct(',') {
            return Ok(first);
        }
        let mut items = vec![first];
        loop {
            items.push(self.expression()?);
            if !self.punct(',') {
                break;
            }
        }
        Ok(Expr::Sequence(items))
    }
    fn expression(&mut self) -> Result<Expr, SyntaxError> {
        if self.depth >= 64 {
            return Err(self.error("syntax nesting limit"));
        }
        self.depth += 1;
        let r = self.assignment();
        self.depth -= 1;
        r
    }
    fn assignment(&mut self) -> Result<Expr, SyntaxError> {
        if self.word("yield") {
            if self.newline()
                || matches!(
                    self.current(),
                    Token::End | Token::Punct(';' | '}' | ')' | ']' | ',' | ':')
                )
            {
                return Ok(Expr::Yield(None, false));
            }
            let delegate = self.op("*");
            return Ok(Expr::Yield(Some(Box::new(self.expression()?)), delegate));
        }

        let covered = matches!(self.current(), Token::Punct('('));
        let mut a = self.binary(1)?;
        if self.punct('?') {
            let yes = self.expression()?;
            self.expect(':')?;
            let no = self.expression()?;
            a = Expr::Conditional(Box::new(a), Box::new(yes), Box::new(no));
        }
        let assignment = match self.current() {
            Token::Op(op)
                if matches!(
                    op.as_str(),
                    "=" | "+="
                        | "-="
                        | "*="
                        | "/="
                        | "%="
                        | "**="
                        | "<<="
                        | ">>="
                        | ">>>="
                        | "&="
                        | "|="
                        | "^="
                        | "&&="
                        | "||="
                        | "??="
                ) =>
            {
                Some(op.clone())
            }
            _ => None,
        };
        if let Some(op) = assignment {
            self.take();
            if op == "=" && matches!(a, Expr::Array(_) | Expr::Object(_)) {
                let pattern = self.assignment_pattern(a)?;
                return Ok(Expr::Destructure(
                    Box::new(pattern),
                    Box::new(self.expression()?),
                ));
            }
            if !matches!(a, Expr::Name(_) | Expr::Member(..) | Expr::Computed(..)) {
                return Err(self.error("invalid assignment target"));
            }
            let rhs = self.expression()?;
            // A parenthesized name is not an IdentifierRef: no function-name inference.
            let rhs = if covered && matches!(a, Expr::Name(_)) && rhs.anonymous_function() {
                Expr::Sequence(vec![rhs])
            } else {
                rhs
            };
            Ok(if op == "=" {
                Expr::Assign(Box::new(a), Box::new(rhs))
            } else {
                Expr::Compound(op[..op.len() - 1].into(), Box::new(a), Box::new(rhs))
            })
        } else {
            Ok(a)
        }
    }

    fn assignment_pattern(&self, expression: Expr) -> Result<Pattern, SyntaxError> {
        Ok(match expression {
            Expr::Name(name) => Pattern::Name(name),
            expression @ (Expr::Member(..) | Expr::Computed(..)) => {
                Pattern::Target(Box::new(expression))
            }
            Expr::Assign(target, default) => {
                Pattern::Default(Box::new(self.assignment_pattern(*target)?), *default)
            }
            Expr::Destructure(pattern, default) => Pattern::Default(pattern, *default),
            Expr::Array(items) => {
                let length = items.len();
                let mut patterns = vec![];
                let mut rest = None;
                for (index, item) in items.into_iter().enumerate() {
                    match item {
                        Some(Expr::Spread(expression)) => {
                            if index + 1 != length {
                                return Err(self.error("assignment rest must be last"));
                            }
                            let pattern = self.assignment_pattern(*expression)?;
                            if matches!(pattern, Pattern::Default(..)) {
                                return Err(self.error("rest cannot have a default"));
                            }
                            rest = Some(Box::new(pattern));
                        }
                        Some(expression) => {
                            patterns.push(Some(self.assignment_pattern(expression)?))
                        }
                        None => patterns.push(None),
                    }
                }
                Pattern::Array(patterns, rest)
            }
            Expr::Object(properties) => {
                let length = properties.len();
                let mut patterns = vec![];
                let mut rest = None;
                for (index, (key, property)) in properties.into_iter().enumerate() {
                    let (key, property) = match property {
                        Property::Computed(key, property) => (PatternKey::Computed(key), *property),
                        property => (PatternKey::Static(key), property),
                    };
                    match property {
                        Property::CoverDefault(name, default) => patterns.push((
                            key,
                            Pattern::Default(Box::new(Pattern::Name(name)), default),
                        )),
                        Property::Value(expression) | Property::Prototype(expression) => {
                            patterns.push((key, self.assignment_pattern(expression)?))
                        }
                        Property::Spread(expression) => {
                            if index + 1 != length
                                || !matches!(
                                    expression,
                                    Expr::Name(_) | Expr::Member(..) | Expr::Computed(..)
                                )
                            {
                                return Err(self.error("invalid object assignment rest"));
                            }
                            rest = Some(Box::new(self.assignment_pattern(expression)?));
                        }
                        _ => return Err(self.error("invalid assignment property")),
                    }
                }
                Pattern::Object(patterns, rest)
            }
            _ => return Err(self.error("invalid destructuring target")),
        })
    }
    fn binary(&mut self, min: u8) -> Result<Expr, SyntaxError> {
        if self.depth >= 64 {
            return Err(self.error("syntax nesting limit"));
        }
        self.depth += 1;
        let result = self.binary_inner(min);
        self.depth -= 1;
        result
    }
    fn binary_inner(&mut self, min: u8) -> Result<Expr, SyntaxError> {
        let start = self.cursor;
        let unary_left = matches!(self.current(), Token::Op(s) if matches!(s.as_str(), "+"|"-"|"!"|"~"))
            || matches!(self.current(), Token::Name(s) if matches!(s.as_str(), "typeof"|"void"|"delete"|"await"));
        let mut a = self.unary()?;
        loop {
            let op = match self.current() {
                Token::Op(s) => s.clone(),
                Token::Name(s) if s == "in" || s == "instanceof" => s.clone(),
                _ => break,
            };
            let p = match op.as_str() {
                "||" | "??" => 1,
                "&&" => 2,
                "|" => 3,
                "^" => 4,
                "&" => 5,
                "==" | "!=" | "===" | "!==" => 6,
                "<" | ">" | "<=" | ">=" | "in" | "instanceof" => 7,
                "<<" | ">>" | ">>>" => 8,
                "+" | "-" => 9,
                "*" | "/" | "%" => 10,
                "**" => 11,
                _ => break,
            };
            if p < min {
                break;
            }
            if op == "**" && unary_left {
                return Err(self.error("unparenthesized unary exponentiation operand"));
            }
            self.take();
            let b = self.binary(if op == "**" { p } else { p + 1 })?;
            a = Expr::Binary(op, Box::new(a), Box::new(b));
        }
        let mut nesting = 0usize;
        let mut nullish = false;
        let mut logical = false;
        for (token, _) in &self.tokens[start..self.cursor] {
            match token {
                Token::Punct('(' | '[' | '{') => nesting += 1,
                Token::Punct(')' | ']' | '}') => nesting = nesting.saturating_sub(1),
                Token::Op(op) if nesting == 0 => {
                    nullish |= op == "??";
                    logical |= op == "&&" || op == "||";
                }
                _ => {}
            }
        }
        if nullish && logical {
            return Err(self.error("nullish and logical operators require parentheses"));
        }
        Ok(a)
    }
    fn unary(&mut self) -> Result<Expr, SyntaxError> {
        if matches!(self.current(),Token::Op(op) if op=="++"||op=="--") {
            let increment = self.op("++");
            if !increment {
                self.op("--");
            }
            if self.depth >= 64 {
                return Err(self.error("syntax nesting limit"));
            }
            self.depth += 1;
            let target = self.unary();
            self.depth -= 1;
            let target = target?;
            if !matches!(
                target,
                Expr::Name(_) | Expr::Member(..) | Expr::Computed(..)
            ) {
                return Err(self.error("invalid update target"));
            }
            return Ok(Expr::Update(Box::new(target), increment, true));
        }
        if self.word("await") {
            if self.depth >= 64 {
                return Err(self.error("syntax nesting limit"));
            }
            self.depth += 1;
            let value = self.unary();
            self.depth -= 1;
            return Ok(Expr::Await(Box::new(value?)));
        }

        if matches!(self.current(),Token::Op(s)if s=="!"||s=="+"||s=="-"||s=="~")
            || matches!(self.current(),Token::Name(s)if s=="typeof"||s=="delete"||s=="void")
        {
            let op = match self.take() {
                Token::Op(s) | Token::Name(s) => s,
                _ => unreachable!(),
            };
            if self.depth >= 64 {
                return Err(self.error("syntax nesting limit"));
            }
            self.depth += 1;
            let e = self.unary();
            self.depth -= 1;
            let e = e?;
            if op == "delete" && self.strict && matches!(e, Expr::Name(_)) {
                return Err(self.error("delete binding in strict mode"));
            }
            if op == "delete"
                && matches!(&e, Expr::Computed(_, key) if matches!(**key, Expr::PrivateName(_)))
            {
                return Err(self.error("delete of private field"));
            }
            return Ok(Expr::Unary(op, Box::new(e)));
        }
        if self.word("new") {
            if self.punct('.') {
                if !self.word("target") {
                    return Err(self.error("expected new.target"));
                }
                return self.postfix(Expr::NewTarget);
            }
            let mut f = self.primary()?;
            loop {
                if self.punct('.') {
                    f = self.member_after_dot(f)?;
                } else if self.punct('[') {
                    let k = self.sequence()?;
                    self.expect(']')?;
                    f = Expr::Computed(Box::new(f), Box::new(k));
                } else {
                    break;
                }
            }
            let a = if self.punct('(') {
                self.arguments()?
            } else {
                vec![]
            };
            return self.postfix(Expr::New(Box::new(f), a));
        }
        let a = self.primary()?;
        self.postfix(a)
    }
    fn arrow_body(&mut self) -> Result<Vec<Stmt>, SyntaxError> {
        if self.current() == &Token::Punct('{') {
            self.function_body()
        } else {
            let body = Stmt::Return(self.expression()?);
            Ok(if self.strict {
                vec![body]
            } else {
                vec![Stmt::Sloppy, body]
            })
        }
    }
    fn template(&mut self, tag: Option<Expr>) -> Result<Expr, SyntaxError> {
        let site = super::templates::site().map_err(|_| self.error("template identity limit"))?;
        let mut parts = vec![];
        let mut values = vec![];
        let mut first = true;
        loop {
            let Token::Template {
                start,
                cooked,
                raw,
                tail,
            } = self.take()
            else {
                return Err(self.error("missing template continuation"));
            };
            if start != first {
                return Err(self.error("invalid template boundary"));
            }
            if cooked.is_none() && tag.is_none() {
                return Err(self.error("invalid template escape"));
            }
            parts.push((cooked, raw));
            if tail {
                break;
            }
            first = false;
            if self.depth >= 64 {
                return Err(self.error("template nesting limit"));
            }
            self.depth += 1;
            let value = self.sequence();
            self.depth -= 1;
            values.push(value?);
        }
        Ok(Expr::Template(Box::new(super::templates::Template {
            site,
            parts,
            values,
            tag: tag.map(Box::new),
        })))
    }
    fn primary(&mut self) -> Result<Expr, SyntaxError> {
        if matches!(self.current(), Token::Template { start: true, .. }) {
            return self.template(None);
        }
        if self.current() == &Token::Name("async".into())
            && self
                .tokens
                .get(self.cursor + 1)
                .is_some_and(|(token, offset)| {
                    matches!(token,Token::Name(name) if !reserved(name))
                        && !self.source[self.tokens[self.cursor].1..*offset]
                            .contains(['\n', '\r', '\u{2028}', '\u{2029}'])
                })
            && self
                .tokens
                .get(self.cursor + 2)
                .is_some_and(|(token, offset)| {
                    matches!(token,Token::Op(op) if op=="=>")
                        && !self.source[self.tokens[self.cursor + 1].1..*offset]
                            .contains(['\n', '\r', '\u{2028}', '\u{2029}'])
                })
        {
            self.take();
            let name = self.name()?;
            self.op("=>");
            let body = self.arrow_body()?;
            return Ok(Expr::AsyncArrow {
                params: vec![name.into()],
                body,
            });
        }
        if matches!(self.current(),Token::Name(name) | Token::EscapedName(name) if !reserved(name))
            && self
                .tokens
                .get(self.cursor + 1)
                .is_some_and(|(token, offset)| {
                    matches!(token,Token::Op(op) if op=="=>")
                        && !self.source[self.tokens[self.cursor].1..*offset]
                            .contains(['\n', '\r', '\u{2028}', '\u{2029}'])
                })
        {
            let name = self.name()?;
            self.op("=>");
            let body = self.arrow_body()?;
            return Ok(Expr::Arrow {
                params: vec![name.into()],
                body,
            });
        }

        if self.current() == &Token::Name("async".into())
            && self.tokens.get(self.cursor + 1).is_some_and(|(t, offset)| {
                !self.source[self.tokens[self.cursor].1..*offset]
                    .contains(['\n', '\r', '\u{2028}', '\u{2029}'])
                    && (t == &Token::Name("function".into()) || t == &Token::Punct('('))
            })
        {
            let save = self.cursor;
            self.take();
            if self.word("function") {
                let generator = self.op("*");
                let name = if self.current() == &Token::Punct('(') {
                    None
                } else {
                    Some(self.name()?)
                };
                let (params, body) = self.function_rest(false, false)?;
                if generator {
                    return Ok(Expr::GeneratorFunction {
                        name,
                        params,
                        body,
                        method: false,
                        asynchronous: true,
                    });
                }
                return Ok(Expr::AsyncFunction {
                    name,
                    params,
                    body,
                    method: false,
                });
            }
            if self.is_arrow() {
                let params = self.params()?;
                if std::mem::take(&mut self.duplicates) {
                    return Err(self.error("duplicate parameter"));
                }
                self.op("=>");
                let body = self.arrow_body()?;
                return Ok(Expr::AsyncArrow { params, body });
            }
            self.cursor = save;
        }

        if self.word("function") {
            let generator = self.op("*");
            let name = if self.current() == &Token::Punct('(') {
                None
            } else {
                Some(self.name()?)
            };
            let (params, body) = self.function_rest(false, false)?;
            return Ok(if generator {
                Expr::GeneratorFunction {
                    name,
                    params,
                    body,
                    method: false,
                    asynchronous: false,
                }
            } else {
                Expr::Function {
                    name,
                    params,
                    body,
                    method: false,
                }
            });
        }
        if self.current() == &Token::Punct('(') && self.is_arrow() {
            let params = self.params()?;
            if std::mem::take(&mut self.duplicates) {
                return Err(self.error("duplicate parameter"));
            }
            self.op("=>");
            let body = self.arrow_body()?;
            return Ok(Expr::Arrow { params, body });
        }
        if self.word("class") {
            let name = match self.current() {
                Token::Name(n) if n != "extends" => Some(self.name()?),
                _ => None,
            };
            return Ok(Expr::Class(Box::new(self.class_tail(name)?)));
        }
        if self.word("super") {
            if self.current() == &Token::Punct('(') {
                if !self.super_call {
                    return Err(self.error("super call outside derived constructor"));
                }
                self.take();
                return Ok(Expr::SuperCall(self.arguments()?));
            }
            if !self.super_property {
                return Err(self.error("super property outside method"));
            }
            if self.punct('.') {
                return Ok(Expr::SuperMember(self.key()?));
            }
            if self.punct('[') {
                let key = self.sequence()?;
                self.expect(']')?;
                return Ok(Expr::SuperComputed(Box::new(key)));
            }
            return Err(self.error("invalid super expression"));
        }
        if let Token::PrivateName(name) = self.current().clone() {
            if self.tokens.get(self.cursor + 1).map(|t| &t.0) == Some(&Token::Name("in".into())) {
                self.take();
                self.private_reference(&name)?;
                return Ok(Expr::PrivateName(name));
            }
            return Err(self.error("unexpected private name"));
        }
        match self.take() {
            Token::Name(n) => match n.as_str() {
                "true" => Ok(Expr::Bool(true)),
                "false" => Ok(Expr::Bool(false)),
                "this" => Ok(Expr::This),
                "null" => Ok(Expr::Null),
                _ if reserved(&n) => Err(self.error("unsupported reserved expression")),
                _ => Ok(Expr::Name(n)),
            },
            Token::EscapedName(n) if !reserved(&n) => Ok(Expr::Name(n)),
            Token::EscapedName(_) => Err(self.error("escaped reserved word")),
            Token::RegExp { pattern, flags } => {
                if !super::regexp_builtins::literal_ok(&pattern, &flags) {
                    return Err(self.error("invalid regular expression"));
                }
                Ok(Expr::RegExp { pattern, flags })
            }
            Token::String(s) => Ok(Expr::String(s)),
            Token::Number(n) => Ok(Expr::Number(n)),
            Token::BigInt(n) => Ok(Expr::BigInt(n)),
            Token::Punct('(') => {
                let e = self.sequence()?;
                self.expect(')')?;
                Ok(e)
            }
            Token::Punct('[') => {
                let mut elements = vec![];
                while !self.punct(']') {
                    if self.punct(',') {
                        elements.push(None);
                        continue;
                    }
                    elements.push(Some(if self.op("...") {
                        Expr::Spread(Box::new(self.expression()?))
                    } else {
                        self.expression()?
                    }));
                    if self.punct(']') {
                        break;
                    }
                    self.expect(',')?;
                }
                Ok(Expr::Array(elements))
            }
            Token::Punct('{') => {
                let mut props = vec![];
                let mut prototype = false;
                if !self.punct('}') {
                    loop {
                        props.push(self.object_property(&mut prototype)?);
                        if self.punct('}') {
                            break;
                        }
                        self.expect(',')?;
                        if self.punct('}') {
                            break;
                        }
                    }
                }
                Ok(Expr::Object(props))
            }
            _ => Err(self.error("unsupported or missing expression")),
        }
    }
    fn arguments(&mut self) -> Result<Vec<Expr>, SyntaxError> {
        let mut a = vec![];
        if !self.punct(')') {
            loop {
                a.push(if self.op("...") {
                    Expr::Spread(Box::new(self.expression()?))
                } else {
                    self.expression()?
                });
                if self.punct(')') {
                    break;
                }
                self.expect(',')?;
                if self.punct(')') {
                    break;
                }
            }
        }
        Ok(a)
    }
    fn postfix(&mut self, mut e: Expr) -> Result<Expr, SyntaxError> {
        for _ in 0..64 {
            if self.punct('.') {
                e = self.member_after_dot(e)?;
            } else if self.punct('[') {
                let k = self.sequence()?;
                self.expect(']')?;
                e = Expr::Computed(Box::new(e), Box::new(k));
            } else if matches!(self.current(), Token::Template { start: true, .. }) {
                e = self.template(Some(e))?;
            } else if self.punct('(') {
                e = Expr::Call(Box::new(e), self.arguments()?);
            } else if !self.newline()
                && matches!(self.current(),Token::Op(op) if op=="++"||op=="--")
            {
                if !matches!(e, Expr::Name(_) | Expr::Member(..) | Expr::Computed(..)) {
                    return Err(self.error("invalid update target"));
                }
                let increment = self.op("++");
                if !increment {
                    self.op("--");
                }
                return Ok(Expr::Update(Box::new(e), increment, false));
            } else {
                return Ok(e);
            }
        }
        Err(self.error("postfix nesting limit"))
    }
}
/// Words the grammar recognises by spelling outside the reserved set.
fn contextual_keyword(n: &str) -> bool {
    matches!(
        n,
        "get" | "set" | "static" | "async" | "of" | "as" | "from" | "target" | "meta"
    )
}
/// Whether a `/` after this token starts a RegularExpressionLiteral rather than a division.
/// Heuristic for the lexer: whether a `{` after this token opens an object literal.
/// True when a `{` follows the parameter list of a function expression, so the
/// matching `}` ends an expression and a following `/` is division.
fn function_expression_body(tokens: &[(Token, usize)]) -> bool {
    if !matches!(tokens.last(), Some((Token::Punct(')'), _))) {
        return false;
    }
    let mut depth = 0usize;
    let mut i = tokens.len();
    loop {
        if i == 0 {
            return false;
        }
        i -= 1;
        match &tokens[i].0 {
            Token::Punct(')') => depth += 1,
            Token::Punct('(') => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
    }
    let at = |j: usize| tokens.get(j).map(|(t, _)| t);
    if i > 0 && matches!(at(i - 1), Some(Token::Name(n)) if n != "function") {
        i -= 1;
    }
    if i > 0 && matches!(at(i - 1), Some(Token::Op(op)) if op == "*") {
        i -= 1;
    }
    if i == 0 || !matches!(at(i - 1), Some(Token::Name(n)) if n == "function") {
        return false;
    }
    i -= 1;
    if i > 0 && matches!(at(i - 1), Some(Token::Name(n)) if n == "async") {
        i -= 1;
    }
    i > 0 && brace_opens_expression(at(i - 1))
}
fn brace_opens_expression(previous: Option<&Token>) -> bool {
    match previous {
        Some(Token::Punct(c)) => matches!(c, '(' | '[' | ',' | ':' | '?'),
        Some(Token::Op(op)) => op != "=>",
        Some(Token::Template { tail, .. }) => !tail,
        Some(Token::Name(n)) => matches!(
            n.as_str(),
            "return"
                | "typeof"
                | "void"
                | "delete"
                | "in"
                | "instanceof"
                | "new"
                | "throw"
                | "yield"
                | "await"
                | "of"
        ),
        _ => false,
    }
}
fn regexp_allowed(previous: Option<&Token>, closed_expression: bool) -> bool {
    match previous {
        None => true,
        Some(Token::Punct('}')) => !closed_expression,
        Some(Token::Punct(c)) => !matches!(c, ')' | ']'),
        Some(Token::Op(op)) => op != "++" && op != "--",
        Some(Token::Template { tail, .. }) => !tail,
        Some(Token::Name(n)) => matches!(
            n.as_str(),
            "return"
                | "typeof"
                | "instanceof"
                | "in"
                | "new"
                | "delete"
                | "void"
                | "throw"
                | "case"
                | "do"
                | "else"
                | "yield"
                | "await"
                | "extends"
        ),
        Some(_) => false,
    }
}
fn reserved(n: &str) -> bool {
    matches!(
        n,
        "break"
            | "case"
            | "catch"
            | "class"
            | "continue"
            | "debugger"
            | "default"
            | "delete"
            | "do"
            | "else"
            | "enum"
            | "export"
            | "extends"
            | "false"
            | "finally"
            | "for"
            | "function"
            | "if"
            | "import"
            | "in"
            | "instanceof"
            | "new"
            | "null"
            | "super"
            | "switch"
            | "this"
            | "throw"
            | "true"
            | "try"
            | "typeof"
            | "var"
            | "void"
            | "while"
            | "with"
            | "yield"
            | "let"
            | "const"
            | "return"
            | "await"
    )
}
fn validate_function(
    body: &[Stmt],
    params: &[Param],
    depth: usize,
    asynchronous: bool,
    generator: bool,
) -> Result<(), SyntaxError> {
    let names: Vec<_> = params.iter().flat_map(|p| p.binding.names()).collect();
    if params
        .iter()
        .any(|p| p.default.is_some() || p.rest || !matches!(p.binding, Pattern::Name(_)))
    {
        for statement in body {
            match statement {
                Stmt::Expr(Expr::String(text)) if text == "use strict" => {
                    return Err(SyntaxError {
                        offset: 0,
                        message: "strict directive with non-simple parameters",
                    })
                }
                Stmt::Expr(Expr::String(_)) => {}
                _ => break,
            }
        }
    }
    for param in params {
        for expression in param.binding.expressions() {
            validate(&[Stmt::Expr(expression.clone())], &names, false, depth + 1)?;
        }
        if let Some(default) = &param.default {
            validate(&[Stmt::Expr(default.clone())], &names, false, depth + 1)?;
        }
    }
    for statement in direct_statements(body) {
        let bindings = match statement {
            Stmt::Let(name, _) | Stmt::Const(name, _) => vec![name.clone()],
            Stmt::Bind { kind, pattern, .. } if kind != "var" => pattern.names(),
            _ => vec![],
        };
        if bindings.iter().any(|name| names.contains(name)) {
            return Err(SyntaxError {
                offset: 0,
                message: "parameter conflicts with lexical binding",
            });
        }
    }
    validate_loop(
        body,
        &[],
        true,
        depth,
        false,
        asynchronous,
        false,
        generator,
    )
}
fn validate(
    body: &[Stmt],
    params: &[String],
    can_return: bool,
    depth: usize,
) -> Result<(), SyntaxError> {
    validate_loop(body, params, can_return, depth, false, false, false, false)
}
fn validate_loop(
    body: &[Stmt],
    params: &[String],
    can_return: bool,
    depth: usize,
    in_loop: bool,
    in_async: bool,
    in_switch: bool,
    in_generator: bool,
) -> Result<(), SyntaxError> {
    let err = |message| SyntaxError { offset: 0, message };
    if depth >= 64 {
        return Err(err("AST nesting limit"));
    }
    let statements = direct_statements(body);
    let mut names: std::collections::HashSet<String> = params.iter().cloned().collect();
    for &s in &statements {
        let bindings = match s {
            Stmt::Const(name, _) | Stmt::Let(name, _) => vec![name.clone()],
            Stmt::Bind { kind, pattern, .. } if kind != "var" => pattern.names(),
            _ => vec![],
        };
        for name in bindings {
            if !names.insert(name) {
                return Err(err("duplicate lexical binding"));
            }
        }
    }
    let mut work = vec![];
    for &s in &statements {
        match s {
            Stmt::Declarations(_) => unreachable!("declaration list was expanded"),
            Stmt::Bind {
                kind,
                pattern,
                init,
            } => {
                if kind == "var" && pattern.names().iter().any(|name| names.contains(name)) {
                    return Err(err("var conflicts with lexical binding"));
                }
                work.extend(
                    pattern
                        .expressions()
                        .into_iter()
                        .map(|expression| (expression, depth)),
                );
                if let Some(init) = init {
                    work.push((init, depth));
                }
            }
            Stmt::Const(_, e) | Stmt::Let(_, e) | Stmt::Expr(e) | Stmt::Throw(e) => {
                work.push((e, depth))
            }
            Stmt::Var(n, e) => {
                if names.contains(n.as_str()) {
                    return Err(err("var conflicts with lexical binding"));
                }
                if let Some(e) = e {
                    work.push((e, depth));
                }
            }
            Stmt::Function(n, p, b)
            | Stmt::AsyncFunction(n, p, b)
            | Stmt::GeneratorFunction(n, p, b, ..) => {
                if names.contains(n.as_str()) {
                    return Err(err("function conflicts with lexical binding"));
                }
                validate_function(
                    b,
                    p,
                    depth + 1,
                    matches!(
                        s,
                        Stmt::AsyncFunction(..) | Stmt::GeneratorFunction(.., true)
                    ),
                    matches!(s, Stmt::GeneratorFunction(..)),
                )?;
            }
            Stmt::Return(e) => {
                if !can_return {
                    return Err(err("return outside function"));
                }
                work.push((e, depth));
            }
            Stmt::Set(a, _, b) => {
                work.push((a, depth));
                work.push((b, depth));
            }
            Stmt::Labeled(..) => unreachable!("labels were stripped"),
            Stmt::Break(Some(_)) => {}
            Stmt::Break(None) => {
                if !in_loop && !in_switch {
                    return Err(err("break outside loop or switch"));
                }
            }
            Stmt::Continue => {
                if !in_loop {
                    return Err(err("continue outside loop"));
                }
            }
            Stmt::Switch(value, cases) => {
                work.push((value, depth));
                let mut all = vec![];
                for (test, body) in cases {
                    if let Some(test) = test {
                        work.push((test, depth));
                    }
                    all.extend(body.iter().cloned());
                }
                validate_loop(
                    &all,
                    &[],
                    can_return,
                    depth + 1,
                    in_loop,
                    in_async,
                    true,
                    in_generator,
                )?;
            }
            Stmt::Empty | Stmt::Sloppy => {}
            Stmt::With(object, body) => {
                work.push((object, depth));
                validate_loop(
                    std::slice::from_ref(body),
                    &[],
                    can_return,
                    depth + 1,
                    in_loop,
                    in_async,
                    in_switch,
                    in_generator,
                )?;
            }
            Stmt::While { test, body, .. } => {
                work.push((test, depth));
                validate_loop(
                    std::slice::from_ref(body),
                    &[],
                    can_return,
                    depth + 1,
                    true,
                    in_async,
                    in_switch,
                    in_generator,
                )?;
            }
            Stmt::For {
                init,
                test,
                update,
                body,
            } => {
                if let Some(init) = init {
                    validate_loop(
                        std::slice::from_ref(init),
                        &[],
                        false,
                        depth + 1,
                        false,
                        in_async,
                        false,
                        in_generator,
                    )?;
                    let bindings = init.lexical_names();
                    let mut vars = vec![];
                    var_names(std::slice::from_ref(body), &mut vars);
                    if bindings.iter().any(|name| vars.contains(name)) {
                        return Err(err("loop var conflicts with lexical initializer"));
                    }
                }
                if let Some(test) = test {
                    work.push((test, depth));
                }
                if let Some(update) = update {
                    work.push((update, depth));
                }
                validate_loop(
                    std::slice::from_ref(body),
                    &[],
                    can_return,
                    depth + 1,
                    true,
                    in_async,
                    in_switch,
                    in_generator,
                )?;
            }
            Stmt::ForOf {
                kind,
                binding,
                iterable,
                body,
                ..
            }
            | Stmt::ForIn {
                kind,
                binding,
                iterable,
                body,
            } => {
                work.extend(
                    binding
                        .expressions()
                        .into_iter()
                        .map(|expression| (expression, depth)),
                );
                if kind == "let" || kind == "const" {
                    let mut names = std::collections::HashSet::new();
                    for name in binding.names() {
                        if !names.insert(name) {
                            return Err(err("duplicate loop binding"));
                        }
                    }
                    let mut vars = vec![];
                    var_names(std::slice::from_ref(body), &mut vars);
                    if vars.iter().any(|name| names.contains(name)) {
                        return Err(err("var conflicts with loop binding"));
                    }
                }
                work.push((iterable, depth));
                validate_loop(
                    std::slice::from_ref(body),
                    &[],
                    can_return,
                    depth + 1,
                    true,
                    in_async,
                    in_switch,
                    in_generator,
                )?;
            }
            Stmt::Block(b) => validate_loop(
                b,
                &[],
                can_return,
                depth + 1,
                in_loop,
                in_async,
                in_switch,
                in_generator,
            )?,
            Stmt::If(e, a, b) => {
                work.push((e, depth));
                validate_loop(
                    std::slice::from_ref(a),
                    &[],
                    can_return,
                    depth + 1,
                    in_loop,
                    in_async,
                    in_switch,
                    in_generator,
                )?;
                if let Some(b) = b {
                    validate_loop(
                        std::slice::from_ref(b),
                        &[],
                        can_return,
                        depth + 1,
                        in_loop,
                        in_async,
                        in_switch,
                        in_generator,
                    )?;
                }
            }
            Stmt::Try {
                body,
                catch,
                finally,
            } => {
                validate_loop(
                    body,
                    &[],
                    can_return,
                    depth + 1,
                    in_loop,
                    in_async,
                    in_switch,
                    in_generator,
                )?;
                if let Some((pattern, b)) = catch {
                    let bindings = pattern.as_ref().map(Pattern::names).unwrap_or_default();
                    let mut unique = std::collections::HashSet::new();
                    if bindings.iter().any(|name| !unique.insert(name)) {
                        return Err(err("duplicate catch binding"));
                    }
                    if let Some(pattern) = pattern {
                        work.extend(pattern.expressions().into_iter().map(|e| (e, depth)));
                    }
                    validate_loop(
                        b,
                        &bindings,
                        can_return,
                        depth + 1,
                        in_loop,
                        in_async,
                        in_switch,
                        in_generator,
                    )?;
                }
                if let Some(b) = finally {
                    validate_loop(
                        b,
                        &[],
                        can_return,
                        depth + 1,
                        in_loop,
                        in_async,
                        in_switch,
                        in_generator,
                    )?;
                }
            }
        }
    }
    while let Some((e, d)) = work.pop() {
        if d >= 64 {
            return Err(err("AST nesting limit"));
        }
        match e {
            Expr::GeneratorFunction {
                params,
                body,
                asynchronous,
                ..
            } => validate_function(body, params, d + 1, *asynchronous, true)?,
            Expr::Yield(value, _) => {
                if !in_generator {
                    return Err(err("yield outside generator"));
                }
                if let Some(value) = value {
                    work.push((value, d + 1));
                }
            }
            Expr::Await(value) => {
                if !in_async {
                    return Err(err("await outside async function"));
                }
                work.push((value, d + 1));
            }
            Expr::AsyncArrow { params, body } | Expr::AsyncFunction { params, body, .. } => {
                validate_function(body, params, d + 1, true, false)?
            }
            Expr::Arrow { params, body } | Expr::Function { params, body, .. } => {
                validate_function(body, params, d + 1, false, false)?
            }
            Expr::Template(t) => {
                if let Some(tag) = &t.tag {
                    work.push((tag, d + 1));
                }
                work.extend(t.values.iter().map(|e| (e, d + 1)));
            }
            Expr::Sequence(elements) => work.extend(elements.iter().map(|x| (x, d + 1))),
            Expr::Array(elements) => work.extend(elements.iter().flatten().map(|x| (x, d + 1))),
            Expr::Object(p) => {
                let mut properties = p.iter().map(|(_, p)| p).collect::<Vec<_>>();
                while let Some(p) = properties.pop() {
                    match p {
                        Property::CoverDefault(..) => {
                            return Err(err("initialized shorthand requires an assignment pattern"))
                        }
                        Property::Computed(key, property) => {
                            work.push((key, d + 1));
                            properties.push(property);
                        }
                        Property::Value(e) | Property::Prototype(e) | Property::Spread(e) => {
                            work.push((e, d + 1))
                        }
                        Property::Getter(b) => validate(b, &[], true, d + 1)?,
                        Property::Setter(n, b) => {
                            validate_function(b, std::slice::from_ref(n), d + 1, false, false)?
                        }
                    }
                }
            }
            Expr::Class(class) => {
                if let Some(heritage) = &class.heritage {
                    work.push((heritage, d + 1));
                }
                if let Some((params, body)) = &class.constructor {
                    validate_function(body, params, d + 1, false, false)?;
                }
                for member in &class.members {
                    if let ClassKey::Computed(key) = &member.key {
                        work.push((key, d + 1));
                    }
                    match &member.kind {
                        MemberKind::Method(e) => work.push((e, d + 1)),
                        MemberKind::Getter(b) => validate_function(b, &[], d + 1, false, false)?,
                        MemberKind::Setter(n, b) => {
                            validate_function(b, std::slice::from_ref(n), d + 1, false, false)?
                        }
                        MemberKind::Field(Some(e)) => {
                            validate(&[Stmt::Expr(e.clone())], &[], false, d + 1)?
                        }
                        MemberKind::Field(None) => {}
                        MemberKind::Block(b) => validate(b, &[], false, d + 1)?,
                    }
                }
            }
            Expr::SuperComputed(a) => work.push((a, d + 1)),
            Expr::SuperCall(b) => work.extend(b.iter().map(|e| (e, d + 1))),
            Expr::Call(a, b) | Expr::New(a, b) => {
                work.push((a, d + 1));
                work.extend(b.iter().map(|e| (e, d + 1)));
            }
            Expr::Member(a, _) | Expr::Unary(_, a) | Expr::Update(a, ..) | Expr::Spread(a) => {
                work.push((a, d + 1))
            }
            Expr::Conditional(a, b, c) => {
                work.push((a, d + 1));
                work.push((b, d + 1));
                work.push((c, d + 1));
            }
            Expr::Destructure(pattern, value) => {
                work.push((value, d + 1));
                work.extend(pattern.expressions().into_iter().map(|e| (e, d + 1)));
            }
            Expr::Computed(a, b)
            | Expr::Assign(a, b)
            | Expr::Binary(_, a, b)
            | Expr::Compound(_, a, b) => {
                work.push((a, d + 1));
                work.push((b, d + 1));
            }
            _ => {}
        }
    }
    Ok(())
}

#[path = "module_parser.rs"]
mod module_syntax;
pub use module_syntax::{parse_module, Export, Import, Module};

pub(super) fn var_names(body: &[Stmt], out: &mut Vec<String>) {
    for s in direct_statements(body) {
        match s {
            Stmt::Bind { kind, pattern, .. } if kind == "var" => out.extend(pattern.names()),
            Stmt::Switch(_, cases) => {
                for (_, body) in cases {
                    var_names(body, out);
                }
            }
            Stmt::While { body, .. } => var_names(std::slice::from_ref(body), out),
            Stmt::With(_, body) => var_names(std::slice::from_ref(body), out),
            Stmt::For { init, body, .. } => {
                if let Some(init) = init {
                    var_names(std::slice::from_ref(init), out);
                }
                var_names(std::slice::from_ref(body), out);
            }
            Stmt::Var(n, _) => out.push(n.clone()),
            Stmt::ForOf {
                kind,
                binding,
                body,
                ..
            }
            | Stmt::ForIn {
                kind,
                binding,
                body,
                ..
            } => {
                if kind == "var" {
                    out.extend(binding.names());
                }
                var_names(std::slice::from_ref(body), out);
            }
            Stmt::Block(b) => var_names(b, out),
            Stmt::If(_, a, b) => {
                var_names(std::slice::from_ref(a), out);
                if let Some(b) = b {
                    var_names(std::slice::from_ref(b), out);
                }
            }
            Stmt::Try {
                body,
                catch,
                finally,
            } => {
                var_names(body, out);
                if let Some((_, b)) = catch {
                    var_names(b, out);
                }
                if let Some(b) = finally {
                    var_names(b, out);
                }
            }
            _ => {}
        }
    }
}
