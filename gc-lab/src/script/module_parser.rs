//! Module syntax shares tokenization, expressions and strict validation with scripts.
//! ParseModule (ECMA-262 §16.2.1.7.1) builds the Source Text Module Record entry
//! lists; MODULE_RECORDS.md describes how modules.rs consumes them.
use super::{tokenize_legacy, validate_loop, Expr, Parser, Stmt, SyntaxError, Token};
use std::collections::HashSet;

/// [[ImportName]] of an ImportEntry (§16.2.1.7, Table 59).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportName {
    Name(String),
    /// `import * as ns` (namespace-object).
    Namespace,
}
#[derive(Clone, Debug)]
pub struct Import {
    pub request: String,
    pub import_name: ImportName,
    pub local: String,
}
/// [[ImportName]] of an ExportEntry (§16.2.1.7, Table 60).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportImport {
    Name(String),
    /// `export * as ns from` (all).
    All,
    /// `export * from` (all-but-default).
    AllButDefault,
}
#[derive(Clone, Debug)]
pub struct Export {
    pub export_name: Option<String>,
    pub request: Option<String>,
    pub import_name: Option<ExportImport>,
    pub local: Option<String>,
}
#[derive(Clone, Debug)]
pub struct Module {
    pub body: Vec<Stmt>,
    /// [[RequestedModules]] in source order, without duplicates.
    pub requests: Vec<String>,
    pub imports: Vec<Import>,
    pub local_exports: Vec<Export>,
    pub indirect_exports: Vec<Export>,
    pub star_exports: Vec<Export>,
}
impl Module {
    pub fn dependencies(&self) -> Vec<&str> {
        self.requests.iter().map(String::as_str).collect()
    }
}

/// A ModuleExportName: an IdentifierName or a string literal (§16.2.2).
#[derive(Clone)]
enum ExportName {
    Ident(String, bool),
    Str(String),
}
impl ExportName {
    fn text(&self) -> &str {
        match self {
            ExportName::Ident(s, _) | ExportName::Str(s) => s,
        }
    }
}
fn export_name(p: &mut Parser<'_>) -> Result<ExportName, SyntaxError> {
    match p.take() {
        Token::Name(s) => Ok(ExportName::Ident(s, false)),
        Token::EscapedName(s) => Ok(ExportName::Ident(s, true)),
        Token::String(s) => {
            // IsStringWellFormedUnicode: the lexer keeps lone surrogates as U+FFFD
            // only when it cannot represent them; reject the escape outright.
            if s.contains('\u{FFFD}') && !p.source.contains('\u{FFFD}') {
                return Err(p.error("module export name is not well-formed Unicode"));
            }
            Ok(ExportName::Str(s))
        }
        _ => Err(p.error("expected module export name")),
    }
}
fn specifier(p: &mut Parser<'_>) -> Result<String, SyntaxError> {
    let s = match p.take() {
        Token::String(s) => s,
        _ => return Err(p.error("module requires string specifier")),
    };
    // WithClause (import attributes) is not implemented: never accept it.
    if p.current() == &Token::Name("with".into()) && !p.newline()
        || p.current() == &Token::Name("assert".into()) && !p.newline()
    {
        return Err(p.error("unsupported import attributes"));
    }
    Ok(s)
}
fn request(module: &mut Module, specifier: &str) {
    if !module.requests.iter().any(|r| r == specifier) {
        module.requests.push(specifier.into());
    }
}
/// The ImportedBinding after `as` or a bare import specifier: a BindingIdentifier.
fn imported_binding(p: &mut Parser<'_>) -> Result<String, SyntaxError> {
    if let Token::Name(n) | Token::EscapedName(n) = p.current() {
        if n == "await" {
            return Err(p.error("await is a reserved word in modules"));
        }
    }
    p.name()
}
/// ImportDeclaration after `import` (§16.2.2).
fn import_declaration(p: &mut Parser<'_>, module: &mut Module) -> Result<(), SyntaxError> {
    if let Token::String(_) = p.current() {
        let s = specifier(p)?;
        p.semi()?;
        request(module, &s);
        return Ok(());
    }
    // Import phases (`import defer`, `import source`) are not implemented.
    if let Token::Name(n) = p.current() {
        if matches!(n.as_str(), "defer" | "source") {
            let next = p.tokens.get(p.cursor + 1).map(|t| &t.0);
            if matches!(next, Some(Token::Op(o)) if o == "*")
                || matches!(next, Some(Token::Name(m)) if m != "from")
            {
                return Err(p.error("unsupported import phase"));
            }
        }
    }
    let mut entries: Vec<(ImportName, String)> = vec![];
    let named = |p: &mut Parser<'_>, entries: &mut Vec<(ImportName, String)>| {
        if p.op("*") {
            if !p.word("as") {
                return Err(p.error("expected as"));
            }
            entries.push((ImportName::Namespace, imported_binding(p)?));
            return Ok(());
        }
        p.expect('{')?;
        while !p.punct('}') {
            let name = export_name(p)?;
            let local = if p.word("as") {
                imported_binding(p)?
            } else {
                match name {
                    ExportName::Ident(..) => {
                        p.cursor -= 1;
                        imported_binding(p)?
                    }
                    ExportName::Str(_) => return Err(p.error("string import requires as")),
                }
            };
            entries.push((ImportName::Name(name.text().into()), local));
            if p.punct('}') {
                break;
            }
            p.expect(',')?;
        }
        Ok(())
    };
    if matches!(p.current(), Token::Name(_) | Token::EscapedName(_)) {
        entries.push((ImportName::Name("default".into()), imported_binding(p)?));
        if p.punct(',') {
            named(p, &mut entries)?;
        }
    } else {
        named(p, &mut entries)?;
    }
    if !p.word("from") {
        return Err(p.error("expected from"));
    }
    let s = specifier(p)?;
    p.semi()?;
    request(module, &s);
    module
        .imports
        .extend(entries.into_iter().map(|(import_name, local)| Import {
            request: s.clone(),
            import_name,
            local,
        }));
    Ok(())
}
/// ExportDeclaration after `export` (§16.2.3). Returns the raw ExportEntries.
fn export_declaration(
    p: &mut Parser<'_>,
    module: &mut Module,
    out: &mut Vec<Export>,
    referenced: &mut Vec<(String, usize)>,
) -> Result<(), SyntaxError> {
    if p.op("*") {
        let export_name = if p.word("as") {
            Some(export_name(p)?.text().to_string())
        } else {
            None
        };
        if !p.word("from") {
            return Err(p.error("expected from"));
        }
        let s = specifier(p)?;
        p.semi()?;
        request(module, &s);
        out.push(Export {
            import_name: Some(if export_name.is_some() {
                ExportImport::All
            } else {
                ExportImport::AllButDefault
            }),
            export_name,
            request: Some(s),
            local: None,
        });
        return Ok(());
    }
    if p.word("default") {
        return export_default(p, module, out);
    }
    if p.current() == &Token::Punct('{') {
        p.take();
        let mut pairs = vec![];
        while !p.punct('}') {
            let offset = p.tokens[p.cursor].1;
            let local = export_name(p)?;
            let exported = if p.word("as") {
                export_name(p)?
            } else {
                local.clone()
            };
            pairs.push((local, exported, offset));
            if p.punct('}') {
                break;
            }
            p.expect(',')?;
        }
        let from = if p.word("from") {
            Some(specifier(p)?)
        } else {
            None
        };
        p.semi()?;
        if let Some(s) = &from {
            request(module, s);
        }
        for (local, exported, offset) in pairs {
            if from.is_none() {
                // §16.2.3.1: without `from`, each local name is an IdentifierReference.
                match &local {
                    ExportName::Str(_) => {
                        return Err(SyntaxError {
                            offset,
                            message: "string export name requires from",
                        })
                    }
                    ExportName::Ident(n, escaped) => {
                        if super::reserved(n)
                            || super::strict_future_reserved(n)
                            || (*escaped && super::reserved(n))
                        {
                            return Err(SyntaxError {
                                offset,
                                message: "reserved word in export",
                            });
                        }
                        referenced.push((n.clone(), offset));
                    }
                }
                out.push(Export {
                    export_name: Some(exported.text().into()),
                    request: None,
                    import_name: None,
                    local: Some(local.text().into()),
                });
            } else {
                out.push(Export {
                    export_name: Some(exported.text().into()),
                    request: from.clone(),
                    import_name: Some(ExportImport::Name(local.text().into())),
                    local: None,
                });
            }
        }
        return Ok(());
    }
    // `export` VariableStatement / Declaration.
    let declaration = matches!(p.current(), Token::Name(n) if matches!(n.as_str(), "var" | "let" | "const" | "function" | "class" | "async"));
    if !declaration {
        return Err(p.error("expected export declaration"));
    }
    if p.current() == &Token::Name("async".into())
        && p.tokens.get(p.cursor + 1).map(|t| &t.0) != Some(&Token::Name("function".into()))
    {
        return Err(p.error("expected export declaration"));
    }
    let statement = p.statement()?;
    let mut names = vec![];
    for declaration in super::direct_statements(std::slice::from_ref(&statement)) {
        names.extend(match declaration {
            Stmt::Const(n, _)
            | Stmt::Let(n, _)
            | Stmt::Var(n, _)
            | Stmt::Function(n, ..)
            | Stmt::AsyncFunction(n, ..)
            | Stmt::GeneratorFunction(n, ..) => vec![n.clone()],
            Stmt::Bind { kind, pattern, .. } if !super::is_using(kind) => pattern.names(),
            _ => return Err(p.error("unsupported export declaration")),
        });
    }
    for name in names {
        out.push(Export {
            export_name: Some(name.clone()),
            request: None,
            import_name: None,
            local: Some(name),
        });
    }
    module.body.push(statement);
    Ok(())
}
/// `export default` HoistableDeclaration / ClassDeclaration / AssignmentExpression.
fn export_default(
    p: &mut Parser<'_>,
    module: &mut Module,
    out: &mut Vec<Export>,
) -> Result<(), SyntaxError> {
    // The anonymous forms bind "*default*" (§16.2.3.2); "\0default" cannot collide.
    let mut local = "\0default".to_string();
    let async_function = p.current() == &Token::Name("async".into())
        && p.tokens
            .get(p.cursor + 1)
            .is_some_and(|(t, _)| *t == Token::Name("function".into()))
        && !p.source[p.tokens[p.cursor].1..p.tokens[p.cursor + 1].1]
            .contains(['\n', '\r', '\u{2028}', '\u{2029}']);
    if p.current() == &Token::Name("function".into()) || async_function {
        if async_function {
            p.take();
        }
        p.take();
        let generator = p.op("*");
        if matches!(p.current(), Token::Name(_) | Token::EscapedName(_))
            && p.current() != &Token::Punct('(')
        {
            local = p.name()?;
        }
        let (params, body) = p.function_rest(false, false, generator, async_function)?;
        p.strict_function_name(Some(&local), &body)?;
        module.body.push(if generator {
            Stmt::GeneratorFunction(local.clone(), params, body, async_function)
        } else if async_function {
            Stmt::AsyncFunction(local.clone(), params, body)
        } else {
            Stmt::Function(local.clone(), params, body)
        });
    } else if p.word("class") {
        let name = match p.current() {
            Token::Name(n) if n != "extends" => Some(p.binding_name(true)?),
            Token::EscapedName(_) => Some(p.binding_name(true)?),
            _ => None,
        };
        // An anonymous ClassDeclaration gets the name "default" from NamedEvaluation
        // of the "\0default" binding (eval_named maps it), with no inner binding.
        let class = p.class_tail(name.clone())?;
        if let Some(n) = name {
            local = n;
        }
        module
            .body
            .push(Stmt::Let(local.clone(), Expr::Class(Box::new(class))));
    } else {
        let expression = p.assignment()?;
        p.semi()?;
        module.body.push(Stmt::Const(local.clone(), expression));
    }
    out.push(Export {
        export_name: Some("default".into()),
        request: None,
        import_name: None,
        local: Some(local),
    });
    Ok(())
}
pub fn parse_module(source: &str) -> Result<Module, SyntaxError> {
    let (tokens, legacy) = tokenize_legacy(source)?;
    let mut p = Parser::new(tokens, legacy, source, true);
    p.module = true;
    p.await_kw = true;
    p.new_target = false;
    let mut module = Module {
        body: vec![],
        requests: vec![],
        imports: vec![],
        local_exports: vec![],
        indirect_exports: vec![],
        star_exports: vec![],
    };
    let mut exports = vec![];
    let mut referenced = vec![];
    while p.current() != &Token::End {
        let import_declaration_ahead = p.current() == &Token::Name("import".into())
            && !matches!(
                p.tokens.get(p.cursor + 1).map(|t| &t.0),
                Some(Token::Punct('(' | '.'))
            );
        if import_declaration_ahead {
            p.take();
            import_declaration(&mut p, &mut module)?;
        } else if p.word("export") {
            export_declaration(&mut p, &mut module, &mut exports, &mut referenced)?;
        } else {
            module.body.push(p.statement()?);
        }
    }
    // Early errors (§16.2.1.1).
    let mut imported = HashSet::new();
    for import in &module.imports {
        if super::reserved(&import.local) || import.local == "eval" || import.local == "arguments" {
            return Err(p.error("invalid imported binding"));
        }
        if !imported.insert(import.local.clone()) {
            return Err(p.error("duplicate imported binding"));
        }
    }
    let mut lexical = imported.clone();
    for statement in super::direct_statements(&module.body) {
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
    for (name, offset) in &referenced {
        if !lexical.contains(name) && !vars.contains(name) {
            return Err(SyntaxError {
                offset: *offset,
                message: "exported binding is not declared",
            });
        }
    }
    let mut names = HashSet::new();
    for export in &exports {
        if let Some(n) = &export.export_name {
            if !names.insert(n.clone()) {
                return Err(p.error("duplicate export"));
            }
        }
    }
    let params = imported.iter().cloned().collect::<Vec<_>>();
    validate_loop(&module.body, &params, false, 0, false, true, false, false)?;
    p.legacy_check()?;
    // ParseModule steps 10-12: classify the export entries.
    for ee in exports {
        if ee.request.is_none() {
            let local = ee.local.clone().unwrap();
            match module.imports.iter().find(|ie| ie.local == local) {
                None => module.local_exports.push(ee),
                // Step 10.a.ii.2: re-export of an imported namespace object is an
                // indirect `~all~` entry, so two such paths resolve to the same binding.
                Some(ie) if ie.import_name == ImportName::Namespace => {
                    module.indirect_exports.push(Export {
                        export_name: ee.export_name,
                        request: Some(ie.request.clone()),
                        import_name: Some(ExportImport::All),
                        local: None,
                    })
                }
                Some(ie) => {
                    let ImportName::Name(n) = &ie.import_name else {
                        unreachable!()
                    };
                    module.indirect_exports.push(Export {
                        export_name: ee.export_name,
                        request: Some(ie.request.clone()),
                        import_name: Some(ExportImport::Name(n.clone())),
                        local: None,
                    });
                }
            }
        } else if ee.import_name == Some(ExportImport::AllButDefault) {
            module.star_exports.push(ee);
        } else {
            module.indirect_exports.push(ee);
        }
    }
    Ok(module)
}
