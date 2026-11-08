//! Bounded grammar for strict scripts; unsupported forms are rejected explicitly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxError {
    pub offset: usize,
    pub message: &'static str,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    Name(String),
    String(String),
    Number(String),
    Bool(bool),
    Null,
    Undefined,
    This,
    Arrow {
        params: Vec<String>,
        body: Vec<Stmt>,
    },
    Function {
        name: Option<String>,
        params: Vec<String>,
        body: Vec<Stmt>,
        method: bool,
    },
    AsyncFunction {
        name: Option<String>,
        params: Vec<String>,
        body: Vec<Stmt>,
        method: bool,
    },
    AsyncArrow {
        params: Vec<String>,
        body: Vec<Stmt>,
    },
    Await(Box<Expr>),
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
    Compound(String, Box<Expr>, Box<Expr>),
    Update(Box<Expr>, bool, bool),
    Conditional(Box<Expr>, Box<Expr>, Box<Expr>),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Property {
    Value(Expr),
    Prototype(Expr),
    Getter(Vec<Stmt>),
    Setter(String, Vec<Stmt>),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stmt {
    Empty,
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
    Function(String, Vec<String>, Vec<Stmt>),
    AsyncFunction(String, Vec<String>, Vec<Stmt>),
    Set(Expr, String, Expr),
    Expr(Expr),
    Return(Expr),
    Throw(Expr),
    Block(Vec<Stmt>),
    If(Expr, Box<Stmt>, Option<Box<Stmt>>),
    Break,
    Continue,
    ForIn {
        kind: String,
        name: String,
        iterable: Expr,
        body: Box<Stmt>,
    },
    ForOf {
        kind: String,
        name: String,
        iterable: Expr,
        body: Box<Stmt>,
    },
    Try {
        body: Vec<Stmt>,
        catch: Option<(String, Vec<Stmt>)>,
        finally: Option<Vec<Stmt>>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    Name(String),
    String(String),
    Number(String),
    Punct(char),
    Op(String),
    End,
}
struct Parser<'a> {
    tokens: Vec<(Token, usize)>,
    cursor: usize,
    depth: usize,
    source: &'a str,
}
fn tokenize(source: &str) -> Result<Vec<(Token, usize)>, SyntaxError> {
    if source.len() > 65536 {
        return Err(SyntaxError {
            offset: 0,
            message: "source limit",
        });
    }
    let mut tokens = vec![];
    let mut chars = source.char_indices().peekable();
    while let Some((offset, c)) = chars.next() {
        if c.is_whitespace() {
            continue;
        }
        let error = |message| SyntaxError { offset, message };
        let token = match c {
            '/' if chars.peek().is_some_and(|(_, c)| *c == '/' || *c == '*') => {
                let style = chars.next().unwrap().1;
                if style == '/' {
                    for (_, c) in chars.by_ref() {
                        if c == '\n' {
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
            'a'..='z' | 'A'..='Z' | '_' | '$' => {
                let mut n = c.to_string();
                while chars
                    .peek()
                    .is_some_and(|(_, c)| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
                {
                    n.push(chars.next().unwrap().1);
                }
                Token::Name(n)
            }
            '0'..='9' => {
                let mut n = c.to_string();
                while chars.peek().is_some_and(|(_, c)| {
                    c.is_ascii_digit()
                        || *c == '.'
                        || *c == 'e'
                        || *c == 'E'
                        || ((*c == '+' || *c == '-') && n.ends_with(['e', 'E']))
                }) {
                    n.push(chars.next().unwrap().1);
                }
                if n.parse::<f64>().is_err() {
                    return Err(error("unsupported numeric literal"));
                }
                if n.len() > 1 && n.starts_with('0') && n.as_bytes()[1].is_ascii_digit() {
                    return Err(error("legacy numeric literal in strict mode"));
                }
                Token::Number(n)
            }
            '\'' | '"' => {
                let mut s = String::new();
                let mut closed = false;
                while let Some((_, ch)) = chars.next() {
                    if ch == c {
                        closed = true;
                        break;
                    }
                    if ch == '\n' || ch == '\r' {
                        return Err(error("newline in string"));
                    }
                    if ch != '\\' {
                        s.push(ch);
                        continue;
                    }
                    let escaped = chars.next().ok_or(error("unterminated escape"))?.1;
                    s.push(match escaped {
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        'b' => '\x08',
                        'f' => '\x0c',
                        'v' => '\x0b',
                        '0' => '\0',
                        '\\' => '\\',
                        '\'' => '\'',
                        '"' => '"',
                        'u' | 'x' => {
                            let mut hex = String::new();
                            for _ in 0..if escaped == 'u' { 4 } else { 2 } {
                                hex.push(chars.next().ok_or(error("unterminated escape"))?.1);
                            }
                            let n = u32::from_str_radix(&hex, 16)
                                .map_err(|_| error("invalid escape"))?;
                            char::from_u32(n).ok_or(error("surrogate strings unsupported"))?
                        }
                        _ => return Err(error("unsupported escape")),
                    });
                }
                if !closed {
                    return Err(error("unterminated string"));
                }
                Token::String(s)
            }
            '(' | ')' | '{' | '}' | '[' | ']' | '.' | ',' | ';' | ':' => Token::Punct(c),
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
                    && tokens
                        .last()
                        .is_some_and(|(_, last)| source[*last..offset].contains(['\n', '\r']))
                {
                    return Err(error("newline before arrow"));
                }
                Token::Op(op)
            }
            _ => return Err(error("unsupported character")),
        };
        tokens.push((token, offset));
        if tokens.len() > 8192 {
            return Err(error("token limit"));
        }
    }
    tokens.push((Token::End, source.len()));
    Ok(tokens)
}
pub fn parse(source: &str) -> Result<Vec<Stmt>, SyntaxError> {
    let tokens = tokenize(source)?;
    let mut p = Parser {
        tokens,
        cursor: 0,
        depth: 0,
        source,
    };
    let body = p.statements(false)?;
    validate(&body, &[], false, 0)?;
    Ok(body)
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
        } else {
            Err(self.error("expected identifier"))
        }
    }
    fn key(&mut self) -> Result<String, SyntaxError> {
        match self.take() {
            Token::Name(s) | Token::String(s) => Ok(s),
            Token::Number(s) => Ok(super::execute::number_string(s.parse().unwrap())),
            _ => Err(self.error("unsupported property key")),
        }
    }
    fn newline(&self) -> bool {
        self.cursor > 0
            && self.source[self.tokens[self.cursor - 1].1..self.tokens[self.cursor].1]
                .contains(['\n', '\r'])
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
    fn params(&mut self) -> Result<Vec<String>, SyntaxError> {
        self.expect('(')?;
        let mut p = vec![];
        if !self.punct(')') {
            loop {
                let n = self.name()?;
                if p.contains(&n) {
                    return Err(self.error("duplicate parameter"));
                }
                p.push(n);
                if self.punct(')') {
                    break;
                }
                self.expect(',')?;
            }
        }
        Ok(p)
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
    fn statement_inner(&mut self) -> Result<Stmt, SyntaxError> {
        if self.current() == &Token::Punct('{') {
            return Ok(Stmt::Block(self.block()?));
        }
        if self.word("break") {
            self.semi()?;
            return Ok(Stmt::Break);
        }
        if self.word("continue") {
            self.semi()?;
            return Ok(Stmt::Continue);
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
                let name = self.name()?;
                if self.word("in") {
                    let iterable = self.sequence()?;
                    self.expect(')')?;
                    return Ok(Stmt::ForIn {
                        kind: kind.into(),
                        name,
                        iterable,
                        body: Box::new(self.statement()?),
                    });
                }
                if self.word("of") {
                    let iterable = self.expression()?;
                    self.expect(')')?;
                    return Ok(Stmt::ForOf {
                        kind: kind.into(),
                        name,
                        iterable,
                        body: Box::new(self.statement()?),
                    });
                }
                let value = if self.op("=") {
                    Some(self.expression()?)
                } else {
                    None
                };
                let stmt = match kind {
                    "const" => Stmt::Const(
                        name,
                        value.ok_or_else(|| self.error("const requires initializer"))?,
                    ),
                    "let" => Stmt::Let(name, value.unwrap_or(Expr::Undefined)),
                    _ => Stmt::Var(name, value),
                };
                self.expect(';')?;
                Some(Box::new(stmt))
            } else if self.punct(';') {
                None
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
        if self.current() == &Token::Name("async".into())
            && self.tokens.get(self.cursor + 1).is_some_and(|(t, offset)| {
                t == &Token::Name("function".into())
                    && !self.source[self.tokens[self.cursor].1..*offset].contains(['\n', '\r'])
            })
        {
            self.take();
            self.take();
            let name = self.name()?;
            let params = self.params()?;
            return Ok(Stmt::AsyncFunction(name, params, self.block()?));
        }
        if self.word("function") {
            let n = self.name()?;
            let p = self.params()?;
            return Ok(Stmt::Function(n, p, self.block()?));
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
                self.expect('(')?;
                let n = self.name()?;
                self.expect(')')?;
                Some((n, self.block()?))
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
        let s = if self.word("const") {
            let n = self.name()?;
            if !self.op("=") {
                return Err(self.error("const requires initializer"));
            }
            Stmt::Const(n, self.expression()?)
        } else if self.word("let") {
            let n = self.name()?;
            let e = if self.op("=") {
                self.expression()?
            } else {
                Expr::Undefined
            };
            Stmt::Let(n, e)
        } else if self.word("var") {
            let n = self.name()?;
            let e = if self.op("=") {
                Some(self.expression()?)
            } else {
                None
            };
            Stmt::Var(n, e)
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
        let mut i = self.cursor + 1;
        if self.tokens.get(i).map(|t| &t.0) != Some(&Token::Punct(')')) {
            loop {
                if !matches!(self.tokens.get(i), Some((Token::Name(_), _))) {
                    return false;
                }
                i += 1;
                if self.tokens.get(i).map(|t| &t.0) == Some(&Token::Punct(',')) {
                    i += 1;
                } else {
                    break;
                }
            }
        }
        self.tokens.get(i).map(|t| &t.0) == Some(&Token::Punct(')'))
            && self.tokens.get(i + 1).map(|t| &t.0) == Some(&Token::Op("=>".into()))
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
            if !matches!(a, Expr::Name(_) | Expr::Member(..) | Expr::Computed(..)) {
                return Err(self.error("invalid assignment target"));
            }
            let rhs = self.expression()?;
            Ok(if op == "=" {
                Expr::Assign(Box::new(a), Box::new(rhs))
            } else {
                Expr::Compound(op[..op.len() - 1].into(), Box::new(a), Box::new(rhs))
            })
        } else {
            Ok(a)
        }
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
            if op == "delete" && matches!(e, Expr::Name(_)) {
                return Err(self.error("delete binding in strict mode"));
            }
            return Ok(Expr::Unary(op, Box::new(e)));
        }
        if self.word("new") {
            let mut f = self.primary()?;
            while self.punct('.') {
                f = Expr::Member(Box::new(f), self.key()?);
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
    fn primary(&mut self) -> Result<Expr, SyntaxError> {
        if self.current() == &Token::Name("async".into())
            && self.tokens.get(self.cursor + 1).is_some_and(|(t, offset)| {
                !self.source[self.tokens[self.cursor].1..*offset].contains(['\n', '\r'])
                    && (t == &Token::Name("function".into()) || t == &Token::Punct('('))
            })
        {
            let save = self.cursor;
            self.take();
            if self.word("function") {
                let name = if self.current() == &Token::Punct('(') {
                    None
                } else {
                    Some(self.name()?)
                };
                let params = self.params()?;
                return Ok(Expr::AsyncFunction {
                    name,
                    params,
                    body: self.block()?,
                    method: false,
                });
            }
            if self.is_arrow() {
                let params = self.params()?;
                self.op("=>");
                let body = if self.current() == &Token::Punct('{') {
                    self.block()?
                } else {
                    vec![Stmt::Return(self.expression()?)]
                };
                return Ok(Expr::AsyncArrow { params, body });
            }
            self.cursor = save;
        }

        if self.word("function") {
            let name = if self.current() == &Token::Punct('(') {
                None
            } else {
                Some(self.name()?)
            };
            let params = self.params()?;
            return Ok(Expr::Function {
                name,
                params,
                body: self.block()?,
                method: false,
            });
        }
        if self.current() == &Token::Punct('(') && self.is_arrow() {
            let params = self.params()?;
            self.op("=>");
            let body = if self.current() == &Token::Punct('{') {
                self.block()?
            } else {
                vec![Stmt::Return(self.expression()?)]
            };
            return Ok(Expr::Arrow { params, body });
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
            Token::String(s) => Ok(Expr::String(s)),
            Token::Number(n) => Ok(Expr::Number(n)),
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
                    elements.push(Some(self.expression()?));
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
                        let key = self.key()?;
                        if key == "async"
                            && matches!(
                                self.current(),
                                Token::Name(_) | Token::String(_) | Token::Number(_)
                            )
                        {
                            let key = self.key()?;
                            let params = self.params()?;
                            let body = self.block()?;
                            props.push((
                                key,
                                Property::Value(Expr::AsyncFunction {
                                    name: None,
                                    params,
                                    body,
                                    method: true,
                                }),
                            ));
                            if self.punct('}') {
                                break;
                            }
                            self.expect(',')?;
                            if self.punct('}') {
                                break;
                            }
                            continue;
                        }
                        let (key, p) = if self.punct(':') {
                            let e = self.expression()?;
                            if key == "__proto__" {
                                if prototype {
                                    return Err(self.error("duplicate prototype literal"));
                                }
                                prototype = true;
                                (key, Property::Prototype(e))
                            } else {
                                (key, Property::Value(e))
                            }
                        } else if (key == "get" || key == "set")
                            && self.current() != &Token::Punct('(')
                        {
                            let n = self.key()?;
                            let p = self.params()?;
                            let b = self.block()?;
                            match (key.as_str(), p.as_slice()) {
                                ("get", []) => (n, Property::Getter(b)),
                                ("set", [p]) => (n, Property::Setter(p.clone(), b)),
                                _ => return Err(self.error("invalid accessor parameters")),
                            }
                        } else if self.current() == &Token::Punct('(') {
                            let params = self.params()?;
                            let body = self.block()?;
                            (
                                key,
                                Property::Value(Expr::Function {
                                    name: None,
                                    params,
                                    body,
                                    method: true,
                                }),
                            )
                        } else {
                            (key.clone(), Property::Value(Expr::Name(key)))
                        };
                        props.push((key, p));
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
                a.push(self.expression()?);
                if self.punct(')') {
                    break;
                }
                self.expect(',')?;
            }
        }
        Ok(a)
    }
    fn postfix(&mut self, mut e: Expr) -> Result<Expr, SyntaxError> {
        for _ in 0..64 {
            if self.punct('.') {
                e = Expr::Member(Box::new(e), self.key()?);
            } else if self.punct('[') {
                let k = self.sequence()?;
                self.expect(']')?;
                e = Expr::Computed(Box::new(e), Box::new(k));
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
fn validate(
    body: &[Stmt],
    params: &[String],
    can_return: bool,
    depth: usize,
) -> Result<(), SyntaxError> {
    validate_loop(body, params, can_return, depth, false, false)
}
fn validate_loop(
    body: &[Stmt],
    params: &[String],
    can_return: bool,
    depth: usize,
    in_loop: bool,
    in_async: bool,
) -> Result<(), SyntaxError> {
    let err = |message| SyntaxError { offset: 0, message };
    if depth >= 64 {
        return Err(err("AST nesting limit"));
    }
    let mut names: std::collections::HashSet<&str> = params.iter().map(String::as_str).collect();
    for s in body {
        if let Stmt::Const(n, _) | Stmt::Let(n, _) = s {
            if !names.insert(n) {
                return Err(err("duplicate lexical binding"));
            }
        }
    }
    let mut work = vec![];
    for s in body {
        match s {
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
            Stmt::Function(n, p, b) | Stmt::AsyncFunction(n, p, b) => {
                if names.contains(n.as_str()) {
                    return Err(err("function conflicts with lexical binding"));
                }
                validate_loop(
                    b,
                    p,
                    true,
                    depth + 1,
                    false,
                    matches!(s, Stmt::AsyncFunction(..)),
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
            Stmt::Break | Stmt::Continue => {
                if !in_loop {
                    return Err(err("loop control outside loop"));
                }
            }
            Stmt::Empty => {}
            Stmt::While { test, body, .. } => {
                work.push((test, depth));
                validate_loop(
                    std::slice::from_ref(body),
                    &[],
                    can_return,
                    depth + 1,
                    true,
                    in_async,
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
                    )?;
                    if let Stmt::Let(name, _) | Stmt::Const(name, _) = init.as_ref() {
                        let mut vars = vec![];
                        var_names(std::slice::from_ref(body), &mut vars);
                        if vars.contains(name) {
                            return Err(err("loop var conflicts with lexical initializer"));
                        }
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
                )?;
            }
            Stmt::ForOf { iterable, body, .. } | Stmt::ForIn { iterable, body, .. } => {
                work.push((iterable, depth));
                validate_loop(
                    std::slice::from_ref(body),
                    &[],
                    can_return,
                    depth + 1,
                    true,
                    in_async,
                )?;
            }
            Stmt::Block(b) => validate_loop(b, &[], can_return, depth + 1, in_loop, in_async)?,
            Stmt::If(e, a, b) => {
                work.push((e, depth));
                validate_loop(
                    std::slice::from_ref(a),
                    &[],
                    can_return,
                    depth + 1,
                    in_loop,
                    in_async,
                )?;
                if let Some(b) = b {
                    validate_loop(
                        std::slice::from_ref(b),
                        &[],
                        can_return,
                        depth + 1,
                        in_loop,
                        in_async,
                    )?;
                }
            }
            Stmt::Try {
                body,
                catch,
                finally,
            } => {
                validate_loop(body, &[], can_return, depth + 1, in_loop, in_async)?;
                if let Some((n, b)) = catch {
                    validate_loop(
                        b,
                        std::slice::from_ref(n),
                        can_return,
                        depth + 1,
                        in_loop,
                        in_async,
                    )?;
                }
                if let Some(b) = finally {
                    validate_loop(b, &[], can_return, depth + 1, in_loop, in_async)?;
                }
            }
        }
    }
    while let Some((e, d)) = work.pop() {
        if d >= 64 {
            return Err(err("AST nesting limit"));
        }
        match e {
            Expr::Await(value) => {
                if !in_async {
                    return Err(err("await outside async function"));
                }
                work.push((value, d + 1));
            }
            Expr::AsyncArrow { params, body } | Expr::AsyncFunction { params, body, .. } => {
                validate_loop(body, params, true, d + 1, false, true)?
            }
            Expr::Arrow { params, body } | Expr::Function { params, body, .. } => {
                validate(body, params, true, d + 1)?
            }
            Expr::Sequence(elements) => work.extend(elements.iter().map(|x| (x, d + 1))),
            Expr::Array(elements) => work.extend(elements.iter().flatten().map(|x| (x, d + 1))),
            Expr::Object(p) => {
                for (_, p) in p {
                    match p {
                        Property::Value(e) | Property::Prototype(e) => work.push((e, d + 1)),
                        Property::Getter(b) => validate(b, &[], true, d + 1)?,
                        Property::Setter(n, b) => {
                            validate(b, std::slice::from_ref(n), true, d + 1)?
                        }
                    }
                }
            }
            Expr::Call(a, b) | Expr::New(a, b) => {
                work.push((a, d + 1));
                work.extend(b.iter().map(|e| (e, d + 1)));
            }
            Expr::Member(a, _) | Expr::Unary(_, a) | Expr::Update(a, ..) => work.push((a, d + 1)),
            Expr::Conditional(a, b, c) => {
                work.push((a, d + 1));
                work.push((b, d + 1));
                work.push((c, d + 1));
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
    for s in body {
        match s {
            Stmt::While { body, .. } => var_names(std::slice::from_ref(body), out),
            Stmt::For { init, body, .. } => {
                if let Some(init) = init {
                    var_names(std::slice::from_ref(init), out);
                }
                var_names(std::slice::from_ref(body), out);
            }
            Stmt::Var(n, _) => out.push(n.clone()),
            Stmt::ForOf {
                kind, name, body, ..
            }
            | Stmt::ForIn {
                kind, name, body, ..
            } => {
                if kind == "var" {
                    out.push(name.clone());
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
