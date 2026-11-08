//! Module syntax shares tokenization, expressions and strict validation with scripts.
use super::{tokenize, validate_loop, Parser, Stmt, SyntaxError, Token};
use std::collections::HashSet;
#[derive(Clone, Debug)]
pub struct Import {
    pub specifier: String,
    pub bindings: Vec<(String, String)>,
}
#[derive(Clone, Debug)]
pub struct Export {
    pub exported: String,
    pub local: String,
    pub from: Option<String>,
}
#[derive(Clone, Debug)]
pub struct Module {
    pub body: Vec<Stmt>,
    pub requests: Vec<String>,
    pub imports: Vec<Import>,
    pub exports: Vec<Export>,
}
impl Module {
    pub fn dependencies(&self) -> Vec<&str> {
        self.requests.iter().map(String::as_str).collect()
    }
}
fn specifier(p: &mut Parser<'_>) -> Result<String, SyntaxError> {
    match p.take() {
        Token::String(s) => Ok(s),
        _ => Err(p.error("module requires string specifier")),
    }
}
fn pairs(p: &mut Parser<'_>, import: bool) -> Result<Vec<(String, String)>, SyntaxError> {
    p.expect('{')?;
    let mut bindings = vec![];
    while !p.punct('}') {
        let name = p.key()?;
        let alias = if p.word("as") {
            if import {
                p.name()?
            } else {
                p.key()?
            }
        } else {
            name.clone()
        };
        bindings.push((name, alias));
        if p.punct('}') {
            break;
        }
        p.expect(',')?;
    }
    Ok(bindings)
}
pub fn parse_module(source: &str) -> Result<Module, SyntaxError> {
    let mut p = Parser {
        tokens: tokenize(source)?,
        cursor: 0,
        depth: 0,
        source,
    };
    let mut module = Module {
        body: vec![],
        requests: vec![],
        imports: vec![],
        exports: vec![],
    };
    while p.current() != &Token::End {
        if p.punct(';') {
            continue;
        }
        if p.word("import") {
            if matches!(p.current(), Token::String(_)) {
                let specifier = specifier(&mut p)?;
                if !module.requests.contains(&specifier) {
                    module.requests.push(specifier.clone());
                }
                p.semi()?;
                module.imports.push(Import {
                    specifier,
                    bindings: vec![],
                });
                continue;
            }
            let mut bindings = vec![];
            if matches!(p.current(), Token::Name(_)) {
                bindings.push(("default".into(), p.name()?));
                if !p.word("from") {
                    p.expect(',')?;
                    bindings.extend(pairs(&mut p, true)?);
                    if !p.word("from") {
                        return Err(p.error("expected from"));
                    }
                }
            } else {
                bindings.extend(pairs(&mut p, true)?);
                if !p.word("from") {
                    return Err(p.error("expected from"));
                }
            }
            let specifier = specifier(&mut p)?;
            if !module.requests.contains(&specifier) {
                module.requests.push(specifier.clone());
            }
            p.semi()?;
            module.imports.push(Import {
                specifier,
                bindings,
            });
            continue;
        }
        if p.word("export") {
            if p.word("default") {
                let mut local = "\0default".to_string();
                let async_function = p.current() == &Token::Name("async".into())
                    && p.tokens
                        .get(p.cursor + 1)
                        .is_some_and(|(t, _)| *t == Token::Name("function".into()));
                if p.current() == &Token::Name("function".into()) || async_function {
                    if async_function {
                        p.take();
                        if p.newline() {
                            return Err(p.error("newline in async declaration"));
                        }
                    }
                    p.take();
                    let generator = p.op("*");
                    if generator && async_function {
                        return Err(p.error("async generator modules are not yet supported"));
                    }
                    if matches!(p.current(), Token::Name(_)) {
                        local = p.name()?;
                    }
                    let params = p.params()?;
                    let body = p.block()?;
                    module.body.push(if generator {
                        Stmt::GeneratorFunction(local.clone(), params, body)
                    } else if async_function {
                        Stmt::AsyncFunction(local.clone(), params, body)
                    } else {
                        Stmt::Function(local.clone(), params, body)
                    });
                } else {
                    let expression = p.expression()?;
                    p.semi()?;
                    module.body.push(Stmt::Const(local.clone(), expression));
                }
                module.exports.push(Export {
                    exported: "default".into(),
                    local,
                    from: None,
                });
            } else if p.current() == &Token::Punct('{') {
                let entries = pairs(&mut p, false)?;
                let from = if p.word("from") {
                    Some(specifier(&mut p)?)
                } else {
                    None
                };
                p.semi()?;
                if let Some(specifier) = &from {
                    if !module.requests.contains(specifier) {
                        module.requests.push(specifier.clone());
                    }
                }
                module
                    .exports
                    .extend(entries.into_iter().map(|(local, exported)| Export {
                        local,
                        exported,
                        from: from.clone(),
                    }));
            } else {
                let statement = p.statement()?;
                let names = match &statement {
                    Stmt::Const(n, _)
                    | Stmt::Let(n, _)
                    | Stmt::Var(n, _)
                    | Stmt::Function(n, ..)
                    | Stmt::AsyncFunction(n, ..)
                    | Stmt::GeneratorFunction(n, ..) => vec![n.clone()],
                    Stmt::Bind { pattern, .. } => pattern.names(),
                    _ => return Err(p.error("unsupported export declaration")),
                };
                for name in names {
                    module.exports.push(Export {
                        exported: name.clone(),
                        local: name,
                        from: None,
                    });
                }
                module.body.push(statement);
            }
        } else {
            module.body.push(p.statement()?);
        }
    }
    let mut imports = HashSet::new();
    for import in &module.imports {
        for (_, local) in &import.bindings {
            if super::reserved(local) || matches!(local.as_str(), "default" | "import" | "export") {
                return Err(p.error("invalid imported binding"));
            }
            if !imports.insert(local.clone()) {
                return Err(p.error("duplicate imported binding"));
            }
        }
    }
    let mut lexical = imports.clone();
    for statement in &module.body {
        let names = match statement {
            Stmt::Const(n, _)
            | Stmt::Let(n, _)
            | Stmt::Function(n, ..)
            | Stmt::AsyncFunction(n, ..)
            | Stmt::GeneratorFunction(n, ..) => vec![n.clone()],
            Stmt::Bind { kind, pattern, .. } if kind != "var" => pattern.names(),
            _ => vec![],
        };
        for name in names {
            if !lexical.insert(name) {
                return Err(p.error("duplicate module binding"));
            }
        }
    }
    let mut vars = vec![];
    super::var_names(&module.body, &mut vars);
    if vars.iter().any(|n| lexical.contains(n)) {
        return Err(p.error("var conflicts with module binding"));
    }
    let params = imports.into_iter().collect::<Vec<_>>();
    validate_loop(&module.body, &params, false, 0, false, true, false, false)?;
    let mut exports = HashSet::new();
    for export in &module.exports {
        if !exports.insert(&export.exported) {
            return Err(p.error("duplicate export"));
        }
    }
    Ok(module)
}
