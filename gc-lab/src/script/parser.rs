//! Deliberately restricted JavaScript syntax; unsupported constructs fail closed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxError {
    pub offset: usize,
    pub message: &'static str,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    Name(String),
    String(String),
    Arrow {
        params: Vec<String>,
        body: Vec<Stmt>,
    },
    Member(Box<Expr>, String),
    Call(Box<Expr>, Vec<Expr>),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stmt {
    Const(String, Expr),
    Set(Expr, String, Expr),
    Expr(Expr),
    Return(Expr),
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    Name(String),
    String(String),
    Const,
    Return,
    Punct(char),
    Arrow,
    End,
}
struct Parser {
    tokens: Vec<(Token, usize)>,
    cursor: usize,
    depth: usize,
}

pub fn parse(source: &str) -> Result<Vec<Stmt>, SyntaxError> {
    if source.len() > 65_536 {
        return Err(SyntaxError {
            offset: 0,
            message: "source limit",
        });
    }
    let mut tokens: Vec<(Token, usize)> = Vec::new();
    let mut chars = source.char_indices().peekable();
    while let Some((offset, ch)) = chars.next() {
        if ch.is_whitespace() {
            continue;
        }
        let error = |message| SyntaxError { offset, message };
        if let Some((Token::Return, previous)) = tokens.last() {
            if source[*previous..offset].contains(['\n', '\r']) {
                return Err(error("automatic semicolon insertion is unsupported"));
            }
        }
        let token = match ch {
            '/' => match chars.next() {
                Some((_, '/')) => {
                    for (_, c) in chars.by_ref() {
                        if c == '\n' {
                            break;
                        }
                    }
                    continue;
                }
                Some((_, '*')) => {
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
                    continue;
                }
                _ => return Err(error("unsupported slash operator")),
            },
            'a'..='z' | 'A'..='Z' | '_' | '$' => {
                let mut name = String::from(ch);
                while chars
                    .peek()
                    .is_some_and(|(_, c)| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
                {
                    name.push(chars.next().unwrap().1);
                }
                match name.as_str() {
                    "const" => Token::Const,
                    "return" => Token::Return,
                    _ => Token::Name(name),
                }
            }
            '\'' | '"' => {
                let mut value = String::new();
                let mut closed = false;
                while let Some((_, c)) = chars.next() {
                    if c == ch {
                        closed = true;
                        break;
                    }
                    if c == '\n' || c == '\r' {
                        return Err(error("newline in string"));
                    }
                    if c == '\\' {
                        let escaped = chars.next().ok_or(error("unterminated escape"))?.1;
                        value.push(match escaped {
                            'n' => '\n',
                            'r' => '\r',
                            't' => '\t',
                            '\\' => '\\',
                            '\'' => '\'',
                            '"' => '"',
                            _ => return Err(error("unsupported escape")),
                        });
                    } else {
                        value.push(c);
                    }
                }
                if !closed {
                    return Err(error("unterminated string"));
                }
                Token::String(value)
            }
            '=' if chars.peek().is_some_and(|(_, c)| *c == '>') => {
                if let Some((_, previous)) = tokens.last() {
                    if source[*previous..offset].contains(['\n', '\r']) {
                        return Err(error("newline before arrow"));
                    }
                }
                chars.next();
                Token::Arrow
            }
            '(' | ')' | '{' | '}' | '.' | ',' | ';' | '=' => Token::Punct(ch),
            _ => return Err(error("unsupported character")),
        };
        tokens.push((token, offset));
        if tokens.len() > 8192 {
            return Err(error("token limit"));
        }
    }
    tokens.push((Token::End, source.len()));
    let mut p = Parser {
        tokens,
        cursor: 0,
        depth: 0,
    };
    let body = p.statements(false)?;
    validate(&body)?;
    Ok(body)
}

impl Parser {
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
    fn expect(&mut self, c: char) -> Result<(), SyntaxError> {
        if self.punct(c) {
            Ok(())
        } else {
            Err(self.error("expected punctuation"))
        }
    }
    fn name(&mut self) -> Result<String, SyntaxError> {
        if let Token::Name(name) = self.current().clone() {
            if reserved(&name) {
                return Err(self.error("unsupported reserved identifier"));
            }
            self.cursor += 1;
            Ok(name)
        } else {
            Err(self.error("expected identifier"))
        }
    }
    fn statements(&mut self, block: bool) -> Result<Vec<Stmt>, SyntaxError> {
        let mut body = Vec::new();
        while self.current() != &Token::End {
            if self.current() == &Token::Punct('}') {
                if block {
                    self.cursor += 1;
                    return Ok(body);
                }
                return Err(self.error("unexpected closing brace"));
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
    fn statement(&mut self) -> Result<Stmt, SyntaxError> {
        let statement = if self.current() == &Token::Const {
            self.cursor += 1;
            let name = self.name()?;
            self.expect('=')?;
            Stmt::Const(name, self.expression()?)
        } else if self.current() == &Token::Return {
            self.cursor += 1;
            Stmt::Return(self.expression()?)
        } else {
            let expression = self.expression()?;
            if self.punct('=') {
                let Expr::Member(object, property) = expression else {
                    return Err(self.error("only member assignment is supported"));
                };
                Stmt::Set(*object, property, self.expression()?)
            } else {
                Stmt::Expr(expression)
            }
        };
        self.expect(';')?;
        Ok(statement)
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
            && self.tokens.get(i + 1).map(|t| &t.0) == Some(&Token::Arrow)
    }
    fn expression(&mut self) -> Result<Expr, SyntaxError> {
        if self.depth >= 64 {
            return Err(self.error("syntax nesting limit"));
        }
        self.depth += 1;
        let result = self.expression_inner();
        self.depth -= 1;
        result
    }
    fn expression_inner(&mut self) -> Result<Expr, SyntaxError> {
        let mut expression = match self.current().clone() {
            Token::Name(name) => {
                self.take();
                Expr::Name(name)
            }
            Token::String(value) => {
                self.take();
                Expr::String(value)
            }
            Token::Punct('(') if self.is_arrow() => {
                self.take();
                let mut params = Vec::new();
                if !self.punct(')') {
                    loop {
                        let name = self.name()?;
                        if params.contains(&name) {
                            return Err(self.error("duplicate parameter"));
                        }
                        params.push(name);
                        if self.punct(')') {
                            break;
                        }
                        self.expect(',')?;
                    }
                }
                if self.take() != Token::Arrow {
                    return Err(self.error("expected arrow"));
                }
                self.expect('{')?;
                Expr::Arrow {
                    params,
                    body: self.statements(true)?,
                }
            }
            Token::Punct('(') => {
                self.take();
                let e = self.expression()?;
                self.expect(')')?;
                e
            }
            _ => return Err(self.error("expected expression")),
        };
        let mut postfix_depth = 0;
        loop {
            if postfix_depth >= 64 {
                return Err(self.error("postfix nesting limit"));
            }
            postfix_depth += 1;
            if self.punct('.') {
                expression = Expr::Member(Box::new(expression), self.name()?);
            } else if self.punct('(') {
                let mut args = Vec::new();
                if !self.punct(')') {
                    loop {
                        args.push(self.expression()?);
                        if self.punct(')') {
                            break;
                        }
                        self.expect(',')?;
                    }
                }
                expression = Expr::Call(Box::new(expression), args);
            } else {
                break;
            }
        }
        Ok(expression)
    }
}

fn reserved(name: &str) -> bool {
    matches!(
        name,
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
            | "await"
    )
}

/// Early checks also cover closures that are never invoked. AST has no spans yet;
/// these structural diagnostics use offset zero rather than an invented position.
fn validate(body: &[Stmt]) -> Result<(), SyntaxError> {
    let error = |message| SyntaxError { offset: 0, message };
    let mut bodies: Vec<(&[Stmt], &[String], bool)> = vec![(body, &[], false)];
    while let Some((body, params, can_return)) = bodies.pop() {
        let mut names = std::collections::HashSet::new();
        names.extend(params.iter());
        let mut expressions = Vec::new();
        for statement in body {
            match statement {
                Stmt::Const(name, expr) => {
                    if !names.insert(name) {
                        return Err(error("duplicate lexical binding"));
                    }
                    expressions.push(expr);
                }
                Stmt::Set(receiver, _, expr) => {
                    expressions.push(receiver);
                    expressions.push(expr);
                }
                Stmt::Expr(expr) => expressions.push(expr),
                Stmt::Return(expr) => {
                    if !can_return {
                        return Err(error("return outside function"));
                    }
                    expressions.push(expr);
                }
            }
        }
        while let Some(expr) = expressions.pop() {
            match expr {
                Expr::Arrow { params, body } => bodies.push((body, params, true)),
                Expr::Call(callee, args) => {
                    expressions.push(callee);
                    expressions.extend(args);
                }
                Expr::Member(receiver, _) => expressions.push(receiver),
                _ => {}
            }
        }
    }
    Ok(())
}
