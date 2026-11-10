//! Resumable control flow; await-free operations reuse the existing evaluator.
use super::{
    parser::{Expr, Param, Property, Stmt},
    vm::{Engine, FunctionKind, ScriptError, Value},
};
use crate::dual::Id;
use std::sync::Arc;
type Result<T> = std::result::Result<T, ScriptError>;
const ASYNC_ITERATOR: u64 = 12;
#[derive(Clone)]
pub(super) enum Op {
    Eval(Expr, usize),
    /// NamedEvaluation of an anonymous function or class initializer (§8.4.5).
    EvalNamed(Expr, String, usize),
    Enumerate(usize, usize, usize),
    Iterator(usize, usize, usize),
    IteratorNext(usize, usize, usize, usize, bool),
    AsyncIterator(usize, usize, usize, usize),
    AsyncNext(usize, usize, usize, usize),
    IteratorStep(usize, usize, usize, usize),
    Loop(usize, usize),
    /// A labelled statement: a handler that only stops `break label`.
    Label(usize, String),
    EndLoop,
    CloneIteration(Vec<String>),
    CheckBase(usize),
    PropertyKey(usize, usize, usize),
    Invoke {
        callee: usize,
        receiver: Option<usize>,
        args: Vec<usize>,
        new: bool,
        out: usize,
    },
    InvokeList {
        callee: usize,
        receiver: Option<usize>,
        args: usize,
        new: bool,
        out: usize,
    },
    Append(usize, Option<usize>, bool),
    Init(String, usize, bool),
    Bind(super::parser::Pattern, usize, bool),
    Enter(Vec<Stmt>),
    Exit,
    Hoist(Vec<Stmt>),
    Branch(usize, usize, usize),
    Jump(usize),
    Await(usize, usize),
    Yield(usize, usize),
    Delegate(usize, usize),
    Return(usize),
    Throw(usize),
    Object(usize),
    TemplateObject(super::templates::Template, usize),
    TemplateStep(usize, usize, String, usize),
    Array(usize, u32),
    Property(usize, String, Property),
    Try {
        catch: Option<usize>,
        finally: Option<usize>,
        end: usize,
    },
    Catch(Option<super::parser::Pattern>, Vec<Stmt>),
    EndTry,
    EndFinally,
    Sync(Stmt),
    /// Explicit resource management (RESOURCE_MANAGEMENT.md): bind a fresh dispose
    /// capability as `%dispose` in the current scope, into a register.
    DisposeStart(usize),
    /// AddDisposableResource for the value register; `true` = async-dispose hint.
    Using(usize, bool),
    /// In the scope's finally: DisposeResources synchronously, folding the pending
    /// completion (§9.11.6).
    DisposeSync(usize),
    /// In the scope's finally: start async DisposeResources; the promise goes to the
    /// second register and a pending throw moves into the disposal.
    DisposeAsync(usize, usize, usize),
    /// Catch target for a rejected async disposal: it becomes the pending throw.
    DisposeRejected,
}
pub(super) struct Program {
    ops: Vec<Op>,
    registers: usize,
}
fn name(r: usize) -> String {
    format!("\0async{r}")
}
fn read(r: usize) -> Expr {
    Expr::Name(name(r))
}
fn expr_await(e: &Expr) -> bool {
    match e {
        Expr::Await(_) | Expr::Yield(..) => true,
        Expr::Template(t) => {
            t.tag.as_ref().is_some_and(|tag| expr_await(tag)) || t.values.iter().any(expr_await)
        }
        Expr::Sequence(items) => items.iter().any(expr_await),
        Expr::GeneratorFunction { .. }
        | Expr::Function { .. }
        | Expr::Arrow { .. }
        | Expr::AsyncFunction { .. }
        | Expr::AsyncArrow { .. } => false,
        Expr::Conditional(a, b, c) => expr_await(a) || expr_await(b) || expr_await(c),
        Expr::ImportCall(a, b, _) => expr_await(a) || b.as_deref().is_some_and(expr_await),
        Expr::Member(a, _)
        | Expr::Unary(_, a)
        | Expr::Update(a, ..)
        | Expr::Spread(a)
        | Expr::SuperComputed(a)
        | Expr::OptionalChain(a)
        | Expr::Optional(a) => expr_await(a),
        Expr::Destructure(pattern, value) => {
            expr_await(value) || pattern.expressions().into_iter().any(expr_await)
        }
        Expr::Computed(a, b)
        | Expr::Binary(_, a, b)
        | Expr::Assign(a, b)
        | Expr::Compound(_, a, b) => expr_await(a) || expr_await(b),
        Expr::Call(a, b) | Expr::New(a, b) => expr_await(a) || b.iter().any(expr_await),
        Expr::Array(e) => e.iter().flatten().any(expr_await),
        Expr::Object(props) => props.iter().any(|(_, p)| property_await(p)),
        // ClassTail: the heritage and computed keys are evaluated in the enclosing context
        // (§15.7.15); field initializers and methods are functions of their own.
        Expr::Class(c) => {
            c.heritage.as_ref().is_some_and(expr_await)
                || c.members.iter().any(
                    |m| matches!(&m.key, super::parser::ClassKey::Computed(k) if expr_await(k)),
                )
        }
        _ => false,
    }
}
fn property_await(p: &Property) -> bool {
    match p {
        Property::CoverDefault(_, value) => expr_await(value),
        Property::Computed(key, property) => expr_await(key) || property_await(property),
        Property::Value(e) | Property::Prototype(e) | Property::Spread(e) => expr_await(e),
        _ => false,
    }
}
pub(super) fn stmt_await(s: &Stmt) -> bool {
    match s {
        Stmt::Declarations(items) => items.iter().any(stmt_await),
        Stmt::Bind { kind, .. } if kind == "await using" => true,
        Stmt::Bind { pattern, init, .. } => {
            init.as_ref().is_some_and(expr_await)
                || pattern.expressions().into_iter().any(expr_await)
        }
        Stmt::Const(_, e) | Stmt::Let(_, e) | Stmt::Return(e) | Stmt::Throw(e) | Stmt::Expr(e) => {
            expr_await(e)
        }
        Stmt::Var(_, e) => e.as_ref().is_some_and(expr_await),
        Stmt::Set(a, _, b) => expr_await(a) || expr_await(b),
        Stmt::Block(b) => b.iter().any(stmt_await),
        Stmt::If(e, a, b) => {
            expr_await(e) || stmt_await(a) || b.as_ref().is_some_and(|b| stmt_await(b))
        }
        Stmt::Try {
            body,
            catch,
            finally,
        } => {
            body.iter().any(stmt_await)
                || catch.as_ref().is_some_and(|(pattern, b)| {
                    pattern.iter().flat_map(|p| p.expressions()).any(expr_await)
                        || b.iter().any(stmt_await)
                })
                || finally.as_ref().is_some_and(|b| b.iter().any(stmt_await))
        }
        Stmt::Switch(value, cases) => {
            expr_await(value)
                || cases.iter().any(|(test, body)| {
                    test.as_ref().is_some_and(expr_await) || body.iter().any(stmt_await)
                })
        }
        Stmt::While { test, body, .. } => expr_await(test) || stmt_await(body),
        Stmt::Labeled(_, body) => stmt_await(body),
        Stmt::With(e, body) => expr_await(e) || stmt_await(body),
        Stmt::For {
            init,
            test,
            update,
            body,
        } => {
            init.as_ref().is_some_and(|s| stmt_await(s))
                || test.as_ref().is_some_and(expr_await)
                || update.as_ref().is_some_and(expr_await)
                || stmt_await(body)
        }
        Stmt::ForOf {
            asynchronous: true, ..
        } => true,
        Stmt::ForOf { kind, .. } if kind == "await using" => true,
        Stmt::ForOf {
            binding,
            iterable,
            body,
            ..
        }
        | Stmt::ForIn {
            binding,
            iterable,
            body,
            ..
        } => {
            expr_await(iterable)
                || stmt_await(body)
                || binding.expressions().into_iter().any(expr_await)
        }
        _ => false,
    }
}
struct Compiler {
    ops: Vec<Op>,
    next: usize,
    async_generator: bool,
}
impl Compiler {
    fn emit(&mut self, op: Op) -> usize {
        let at = self.ops.len();
        self.ops.push(op);
        at
    }
    fn reg(&mut self) -> usize {
        let r = self.next;
        self.next += 1;
        r
    }
    /// An initializer for `name`: anonymous functions and classes get NamedEvaluation.
    fn named(&mut self, e: &Expr, name: &str) -> Result<usize> {
        if e.anonymous_function() && !expr_await(e) {
            let r = self.reg();
            self.emit(Op::EvalNamed(e.clone(), name.into(), r));
            return Ok(r);
        }
        self.expression(e)
    }
    fn eval(&mut self, e: Expr) -> usize {
        let r = self.reg();
        self.emit(Op::Eval(e, r));
        r
    }
    fn nullish(&mut self, value: usize) -> usize {
        self.eval(Expr::Binary(
            "||".into(),
            Box::new(Expr::Binary(
                "===".into(),
                Box::new(read(value)),
                Box::new(Expr::Null),
            )),
            Box::new(Expr::Binary(
                "===".into(),
                Box::new(read(value)),
                Box::new(Expr::Undefined),
            )),
        ))
    }
    /// An assignment or update target with its sub-expressions saved in registers.
    /// A super reference reads `this` and [[HomeObject]] when it is used.
    fn assignment_target(&mut self, target: &Expr) -> Result<Expr> {
        match target {
            Expr::Name(_) | Expr::SuperMember(_) => Ok(target.clone()),
            Expr::SuperComputed(key) => {
                let key = self.expression(key)?;
                Ok(Expr::SuperComputed(Box::new(read(key))))
            }
            _ => Ok(self.reference(target)?.1),
        }
    }
    fn reference(&mut self, e: &Expr) -> Result<(usize, Expr)> {
        match e {
            Expr::Member(base, key) => {
                let base = self.expression(base)?;
                self.emit(Op::CheckBase(base));
                Ok((base, Expr::Member(Box::new(read(base)), key.clone())))
            }
            Expr::Computed(base, key) => {
                let base = self.expression(base)?;
                // The key value is saved but ToPropertyKey (and the base's
                // ToObject) wait for GetValue/PutValue, after any suspension.
                let key = self.expression(key)?;
                Ok((
                    base,
                    Expr::Computed(Box::new(read(base)), Box::new(read(key))),
                ))
            }
            _ => Err(ScriptError::Unsupported("async reference")),
        }
    }
    fn expression(&mut self, e: &Expr) -> Result<usize> {
        if !expr_await(e) {
            return Ok(self.eval(e.clone()));
        }
        Ok(match e {
            Expr::Sequence(items) => {
                let mut out = self.eval(Expr::Undefined);
                for item in items {
                    out = self.expression(item)?;
                }
                out
            }
            Expr::Template(t) => {
                if let Some(tag) = &t.tag {
                    let (receiver, callee) =
                        if matches!(tag.as_ref(), Expr::Member(..) | Expr::Computed(..)) {
                            let (base, expression) = self.reference(tag)?;
                            (Some(base), self.eval(expression))
                        } else {
                            (None, self.expression(tag)?)
                        };
                    let object = self.reg();
                    self.emit(Op::TemplateObject(t.as_ref().clone(), object));
                    let mut args = vec![object];
                    for value in &t.values {
                        args.push(self.expression(value)?);
                    }
                    let out = self.reg();
                    self.emit(Op::Invoke {
                        callee,
                        receiver,
                        args,
                        new: false,
                        out,
                    });
                    out
                } else {
                    let first = t.parts[0]
                        .0
                        .clone()
                        .ok_or(ScriptError::Runtime("invalid untagged template escape"))?;
                    let mut text = self.eval(Expr::String(first));
                    for (index, value) in t.values.iter().enumerate() {
                        let value = self.expression(value)?;
                        let out = self.reg();
                        let part = t.parts[index + 1]
                            .0
                            .clone()
                            .ok_or(ScriptError::Runtime("invalid untagged template escape"))?;
                        self.emit(Op::TemplateStep(text, value, part, out));
                        text = out;
                    }
                    text
                }
            }
            Expr::Yield(value, delegate) => {
                let value = if let Some(value) = value {
                    self.expression(value)?
                } else {
                    self.eval(Expr::Undefined)
                };
                let value = if self.async_generator && !*delegate {
                    let awaited = self.reg();
                    self.emit(Op::Await(value, awaited));
                    awaited
                } else {
                    value
                };
                let out = self.reg();
                self.emit(if *delegate {
                    Op::Delegate(value, out)
                } else {
                    Op::Yield(value, out)
                });
                out
            }
            Expr::Await(e) => {
                let value = self.expression(e)?;
                let out = self.reg();
                self.emit(Op::Await(value, out));
                out
            }
            Expr::Call(callee, args) | Expr::New(callee, args) => {
                let new = matches!(e, Expr::New(..));
                let (receiver, callee) =
                    if !new && matches!(callee.as_ref(), Expr::Member(..) | Expr::Computed(..)) {
                        let (base, expr) = self.reference(callee)?;
                        (Some(base), self.eval(expr))
                    } else {
                        (None, self.expression(callee)?)
                    };
                let out = self.reg();
                if args.iter().any(|e| matches!(e, Expr::Spread(_))) {
                    let list = self.reg();
                    self.emit(Op::Array(list, 0));
                    for expression in args {
                        let (expression, spread) = match expression {
                            Expr::Spread(inner) => (inner.as_ref(), true),
                            expression => (expression, false),
                        };
                        let value = self.expression(expression)?;
                        self.emit(Op::Append(list, Some(value), spread));
                    }
                    self.emit(Op::InvokeList {
                        callee,
                        receiver,
                        args: list,
                        new,
                        out,
                    });
                } else {
                    let args = args
                        .iter()
                        .map(|e| self.expression(e))
                        .collect::<Result<Vec<_>>>()?;
                    self.emit(Op::Invoke {
                        callee,
                        receiver,
                        args,
                        new,
                        out,
                    });
                }
                out
            }
            Expr::Member(..) | Expr::Computed(..) => {
                let (_, reference) = self.reference(e)?;
                self.eval(reference)
            }
            Expr::SuperComputed(_) => {
                let reference = self.assignment_target(e)?;
                self.eval(reference)
            }
            Expr::Update(target, increment, prefix) => {
                let target = self.assignment_target(target)?;
                self.eval(Expr::Update(Box::new(target), *increment, *prefix))
            }
            Expr::Compound(op, target, value) => {
                let target = self.assignment_target(target)?;
                let old = self.eval(target.clone());
                let logical = matches!(op.as_str(), "&&" | "||" | "??");
                let out = self.reg();
                let branch = if logical {
                    let test = if op == "??" { self.nullish(old) } else { old };
                    Some(self.emit(Op::Branch(test, 0, 0)))
                } else {
                    None
                };
                let write = self.ops.len();
                let right = self.expression(value)?;
                let value = if logical {
                    read(right)
                } else {
                    Expr::Binary(op.clone(), Box::new(read(old)), Box::new(read(right)))
                };
                self.emit(Op::Eval(
                    Expr::Assign(Box::new(target), Box::new(value)),
                    out,
                ));
                if let Some(branch) = branch {
                    let jump = self.emit(Op::Jump(0));
                    let short = self.ops.len();
                    self.emit(Op::Eval(read(old), out));
                    let end = self.ops.len();
                    self.ops[jump] = Op::Jump(end);
                    self.ops[branch] = if op == "||" {
                        let Op::Branch(test, _, _) = self.ops[branch] else {
                            unreachable!()
                        };
                        Op::Branch(test, short, write)
                    } else {
                        let Op::Branch(test, _, _) = self.ops[branch] else {
                            unreachable!()
                        };
                        Op::Branch(test, write, short)
                    };
                }
                out
            }
            Expr::Destructure(pattern, value) => {
                if pattern.expressions().into_iter().any(expr_await) {
                    return Err(ScriptError::Unsupported(
                        "resumable assignment pattern expressions",
                    ));
                }
                let value = self.expression(value)?;
                self.eval(Expr::Destructure(pattern.clone(), Box::new(read(value))))
            }
            Expr::Assign(target, value) => {
                let target = self.assignment_target(target)?;
                let value = self.expression(value)?;
                self.eval(Expr::Assign(Box::new(target), Box::new(read(value))))
            }
            Expr::Unary(op, e) if op == "delete" => {
                let reference = if matches!(e.as_ref(), Expr::Member(..) | Expr::Computed(..)) {
                    self.reference(e)?.1
                } else {
                    read(self.expression(e)?)
                };
                self.eval(Expr::Unary(op.clone(), Box::new(reference)))
            }
            Expr::Unary(op, e) => {
                let value = self.expression(e)?;
                self.eval(Expr::Unary(op.clone(), Box::new(read(value))))
            }
            Expr::ImportCall(e, options, phase) => {
                let value = self.expression(e)?;
                let options = match options {
                    Some(o) => Some(Box::new(read(self.expression(o)?))),
                    None => None,
                };
                self.eval(Expr::ImportCall(Box::new(read(value)), options, *phase))
            }
            Expr::Conditional(condition, yes, no) => {
                let condition = self.expression(condition)?;
                let out = self.reg();
                let branch = self.emit(Op::Branch(condition, 0, 0));
                let yes_start = self.ops.len();
                let yes = self.expression(yes)?;
                self.emit(Op::Eval(read(yes), out));
                let jump = self.emit(Op::Jump(0));
                let no_start = self.ops.len();
                let no = self.expression(no)?;
                self.emit(Op::Eval(read(no), out));
                let end = self.ops.len();
                self.ops[branch] = Op::Branch(condition, yes_start, no_start);
                self.ops[jump] = Op::Jump(end);
                out
            }
            Expr::Binary(op, a, b) if matches!(op.as_str(), "&&" | "||" | "??") => {
                let left = self.expression(a)?;
                let test = if op == "??" { self.nullish(left) } else { left };
                let out = self.reg();
                let branch = self.emit(Op::Branch(test, 0, 0));
                let right_start = self.ops.len();
                let right = self.expression(b)?;
                self.emit(Op::Eval(read(right), out));
                let jump = self.emit(Op::Jump(0));
                let short = self.ops.len();
                self.emit(Op::Eval(read(left), out));
                let end = self.ops.len();
                self.ops[jump] = Op::Jump(end);
                self.ops[branch] = if op == "&&" || op == "??" {
                    Op::Branch(test, right_start, short)
                } else {
                    Op::Branch(test, short, right_start)
                };
                out
            }
            Expr::Binary(op, a, b) => {
                let a = self.expression(a)?;
                let b = self.expression(b)?;
                self.eval(Expr::Binary(
                    op.clone(),
                    Box::new(read(a)),
                    Box::new(read(b)),
                ))
            }
            Expr::Array(elements) => {
                let out = self.reg();
                self.emit(Op::Array(out, 0));
                for expression in elements {
                    let (value, spread) = match expression {
                        None => (None, false),
                        Some(Expr::Spread(inner)) => (Some(self.expression(inner)?), true),
                        Some(expression) => (Some(self.expression(expression)?), false),
                    };
                    self.emit(Op::Append(out, value, spread));
                }
                out
            }
            Expr::Object(props) => {
                let out = self.reg();
                self.emit(Op::Object(out));
                for (key, p) in props {
                    let p = self.property(out, p)?;
                    self.emit(Op::Property(out, key.clone(), p));
                }
                out
            }
            _ => return Err(ScriptError::Unsupported("await expression shape")),
        })
    }
    fn property(&mut self, object: usize, p: &Property) -> Result<Property> {
        Ok(match p {
            Property::Computed(key, property) => {
                let key = self.expression(key)?;
                let canonical = self.reg();
                self.emit(Op::PropertyKey(object, key, canonical));
                Property::Computed(read(canonical), Box::new(self.property(object, property)?))
            }
            Property::Value(e) if e.anonymous_function() => p.clone(),
            Property::Value(e) => Property::Value(read(self.expression(e)?)),
            Property::Prototype(e) => Property::Prototype(read(self.expression(e)?)),
            Property::Spread(e) => Property::Spread(read(self.expression(e)?)),
            p => p.clone(),
        })
    }
    /// Wrap code for a scope declaring `using` bindings in a try/finally that disposes
    /// its resources (§14.2.2 step 5, §15.2.x FunctionBody, ForIn/OfBodyEvaluation).
    fn using_scope(
        &mut self,
        declarations: &[Stmt],
        inner: impl FnOnce(&mut Self) -> Result<()>,
    ) -> Result<()> {
        if !super::parser::has_direct_using(declarations) {
            return inner(self);
        }
        let asynchronous = super::parser::direct_statements(declarations)
            .iter()
            .any(|s| matches!(s, Stmt::Bind { kind, .. } if kind == "await using"));
        let scope = self.reg();
        self.emit(Op::DisposeStart(scope));
        let start = self.emit(Op::Try {
            catch: None,
            finally: None,
            end: 0,
        });
        inner(self)?;
        self.emit(Op::EndTry);
        let finally = self.ops.len();
        if asynchronous {
            let promise = self.reg();
            let start_async = self.emit(Op::DisposeAsync(scope, promise, 0));
            let guard = self.emit(Op::Try {
                catch: None,
                finally: None,
                end: 0,
            });
            let ignored = self.reg();
            self.emit(Op::Await(promise, ignored));
            self.emit(Op::EndTry);
            let catch = self.emit(Op::DisposeRejected);
            self.emit(Op::EndTry);
            let end = self.ops.len();
            self.ops[guard] = Op::Try {
                catch: Some(catch),
                finally: None,
                end,
            };
            self.ops[start_async] = Op::DisposeAsync(scope, promise, end);
        } else {
            self.emit(Op::DisposeSync(scope));
        }
        self.emit(Op::EndFinally);
        let end = self.ops.len();
        self.ops[start] = Op::Try {
            catch: None,
            finally: Some(finally),
            end,
        };
        Ok(())
    }
    fn body(&mut self, body: &[Stmt], scope: bool) -> Result<()> {
        if scope {
            self.emit(Op::Enter(body.to_vec()));
        } else {
            self.emit(Op::Hoist(body.to_vec()));
        }
        self.using_scope(body, |c| {
            for s in body {
                c.statement(s)?;
            }
            Ok(())
        })?;
        if scope {
            self.emit(Op::Exit);
        }
        Ok(())
    }
    fn for_rest(
        &mut self,
        init: Option<&Stmt>,
        test: Option<&Expr>,
        update: Option<&Expr>,
        body: &Stmt,
    ) -> Result<()> {
        let names = init
            .as_ref()
            .map(|s| s.iteration_names())
            .unwrap_or_default();
        if !names.is_empty() {
            self.emit(Op::CloneIteration(names.clone()));
        }
        let handler = self.emit(Op::Loop(0, 0));
        let start = self.ops.len();
        let branch = if let Some(test) = test {
            let test = self.expression(test)?;
            Some(self.emit(Op::Branch(test, 0, 0)))
        } else {
            None
        };
        let body_start = self.ops.len();
        self.statement(body)?;
        let continue_to = self.ops.len();
        if !names.is_empty() {
            self.emit(Op::CloneIteration(names));
        }
        if let Some(update) = update {
            self.expression(update)?;
        }
        self.emit(Op::Jump(start));
        let exit = self.emit(Op::EndLoop);
        let end = self.ops.len();
        if let Some(branch) = branch {
            let Op::Branch(test, _, _) = self.ops[branch] else {
                unreachable!()
            };
            self.ops[branch] = Op::Branch(test, body_start, exit);
        }
        self.ops[handler] = Op::Loop(end, continue_to);
        Ok(())
    }
    fn statement(&mut self, s: &Stmt) -> Result<()> {
        match s {
            Stmt::Sloppy => {}
            Stmt::With(..) => {
                return Err(ScriptError::Unsupported(
                    "with in generator or async function",
                ))
            }
            Stmt::Declarations(items) => {
                for item in items {
                    self.statement(item)?;
                }
            }
            Stmt::Bind {
                kind,
                pattern,
                init,
            } => {
                if pattern.expressions().into_iter().any(expr_await) {
                    return Err(ScriptError::Unsupported(
                        "resumable binding pattern expressions",
                    ));
                }
                if let Some(init) = init {
                    let value = if let super::parser::Pattern::Name(n) = pattern {
                        self.named(init, n)?
                    } else {
                        self.expression(init)?
                    };
                    self.emit(Op::Bind(pattern.clone(), value, kind == "var"));
                    if super::parser::is_using(kind) {
                        self.emit(Op::Using(value, kind == "await using"));
                    }
                }
            }
            Stmt::Labeled(label, body) => {
                let handler = self.emit(Op::Label(0, String::new()));
                self.statement(body)?;
                let exit = self.emit(Op::EndLoop);
                self.ops[handler] = Op::Label(exit + 1, label.clone());
            }
            Stmt::Switch(expression, cases) => {
                let value = self.expression(expression)?;
                let all = cases
                    .iter()
                    .flat_map(|(_, body)| body.iter().cloned())
                    .collect();
                self.emit(Op::Enter(all));
                let handler = self.emit(Op::Loop(0, usize::MAX));
                let mut branches = vec![];
                let mut default = None;
                for (index, (test, _)) in cases.iter().enumerate() {
                    if let Some(test) = test {
                        let test = self.expression(test)?;
                        let equal = self.eval(Expr::Binary(
                            "===".into(),
                            Box::new(read(value)),
                            Box::new(read(test)),
                        ));
                        let branch = self.emit(Op::Branch(equal, 0, 0));
                        branches.push((branch, index, equal));
                    } else {
                        default = Some(index);
                    }
                }
                let fallback = self.emit(Op::Jump(0));
                let mut bodies = vec![];
                for (_, body) in cases {
                    bodies.push(self.ops.len());
                    for statement in body {
                        self.statement(statement)?;
                    }
                }
                let exit = self.emit(Op::EndLoop);
                let end = self.ops.len();
                self.emit(Op::Exit);
                for (branch, index, test) in branches {
                    self.ops[branch] = Op::Branch(test, bodies[index], branch + 1);
                }
                self.ops[fallback] = Op::Jump(default.map(|index| bodies[index]).unwrap_or(exit));
                self.ops[handler] = Op::Loop(end, usize::MAX);
            }
            Stmt::While { test, body, post } => {
                let handler = self.emit(Op::Loop(0, 0));
                let start = self.ops.len();
                let branch = if !post {
                    let test = self.expression(test)?;
                    Some(self.emit(Op::Branch(test, 0, 0)))
                } else {
                    None
                };
                let body_start = self.ops.len();
                self.statement(body)?;
                let continue_to = if *post { self.ops.len() } else { start };
                if *post {
                    let test = self.expression(test)?;
                    let branch = self.emit(Op::Branch(test, body_start, 0));
                    let end = self.ops.len();
                    self.ops[branch] = Op::Branch(test, body_start, end);
                } else {
                    self.emit(Op::Jump(start));
                }
                let exit = self.emit(Op::EndLoop);
                let end = self.ops.len();
                if let Some(branch) = branch {
                    let Op::Branch(test, _, _) = self.ops[branch] else {
                        unreachable!()
                    };
                    self.ops[branch] = Op::Branch(test, body_start, exit);
                }
                self.ops[handler] = Op::Loop(end, continue_to);
            }
            Stmt::For {
                init,
                test,
                update,
                body,
            } => {
                let lexical = init.as_ref().is_some_and(|s| !s.lexical_names().is_empty());
                if lexical {
                    self.emit(Op::Enter(vec![init.as_ref().unwrap().as_ref().clone()]));
                }
                let head: Vec<Stmt> = init.iter().map(|s| s.as_ref().clone()).collect();
                self.using_scope(&head, |c| {
                    if let Some(init) = init {
                        c.statement(init)?;
                    }
                    c.for_rest(init.as_deref(), test.as_ref(), update.as_ref(), body)
                })?;
                if lexical {
                    self.emit(Op::Exit);
                }
            }
            Stmt::Function(..) | Stmt::AsyncFunction(..) | Stmt::GeneratorFunction(..) => {}
            Stmt::Const(n, e) | Stmt::Let(n, e) => {
                let value = self.named(e, n)?;
                self.emit(Op::Init(n.clone(), value, false));
            }
            Stmt::Var(n, e) => {
                if let Some(e) = e {
                    let value = self.named(e, n)?;
                    self.emit(Op::Init(n.clone(), value, true));
                }
            }
            Stmt::Expr(e) => {
                self.expression(e)?;
            }
            Stmt::Return(e) => {
                let r = self.expression(e)?;
                self.emit(Op::Return(r));
            }
            Stmt::Throw(e) => {
                let r = self.expression(e)?;
                self.emit(Op::Throw(r));
            }
            Stmt::Set(a, k, b) => {
                self.expression(&Expr::Assign(
                    Box::new(Expr::Member(Box::new(a.clone()), k.clone())),
                    Box::new(b.clone()),
                ))?;
            }
            Stmt::Block(b) => self.body(b, true)?,
            Stmt::If(c, a, b) => {
                let test = self.expression(c)?;
                let branch = self.emit(Op::Branch(test, 0, 0));
                let yes = self.ops.len();
                self.statement(a)?;
                let jump = self.emit(Op::Jump(0));
                let no = self.ops.len();
                if let Some(b) = b {
                    self.statement(b)?;
                }
                let end = self.ops.len();
                self.ops[jump] = Op::Jump(end);
                self.ops[branch] = Op::Branch(test, yes, no);
            }
            Stmt::Try {
                body,
                catch,
                finally,
            } => {
                let start = self.emit(Op::Try {
                    catch: None,
                    finally: None,
                    end: 0,
                });
                self.body(body, true)?;
                self.emit(Op::EndTry);
                let catch_ip = if let Some((pattern, body)) = catch {
                    if pattern.iter().flat_map(|p| p.expressions()).any(expr_await) {
                        return Err(ScriptError::Unsupported(
                            "resumable catch binding expressions",
                        ));
                    }
                    let at = self.emit(Op::Catch(pattern.clone(), body.clone()));
                    for s in body {
                        self.statement(s)?;
                    }
                    self.emit(Op::EndTry);
                    Some(at)
                } else {
                    None
                };
                let finally_ip = if let Some(body) = finally {
                    let at = self.ops.len();
                    self.body(body, true)?;
                    self.emit(Op::EndFinally);
                    Some(at)
                } else {
                    None
                };
                let end = self.ops.len();
                self.ops[start] = Op::Try {
                    catch: catch_ip,
                    finally: finally_ip,
                    end,
                };
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
                let enumeration = matches!(s, Stmt::ForIn { .. });
                let asynchronous = matches!(
                    s,
                    Stmt::ForOf {
                        asynchronous: true,
                        ..
                    }
                );
                let lexical = kind == "let" || kind == "const" || super::parser::is_using(kind);
                if binding.expressions().into_iter().any(expr_await) {
                    return Err(ScriptError::Unsupported(
                        "resumable loop binding expressions",
                    ));
                }
                let declaration = Stmt::Bind {
                    kind: if kind == "var" {
                        "let".into()
                    } else {
                        kind.clone()
                    },
                    pattern: binding.clone(),
                    init: Some(Expr::Undefined),
                };
                if lexical {
                    self.emit(Op::Enter(vec![declaration.clone()]));
                }
                let source = self.expression(iterable)?;
                let iterator = self.reg();
                let next = self.reg();
                let sync = self.reg();
                self.emit(if asynchronous {
                    Op::AsyncIterator(source, iterator, next, sync)
                } else if enumeration {
                    Op::Enumerate(source, iterator, next)
                } else {
                    Op::Iterator(source, iterator, next)
                });
                if lexical {
                    self.emit(Op::Exit);
                }
                let handler = self.emit(Op::Loop(0, 0));
                let again = self.ops.len();
                let value = self.reg();
                let done = self.reg();
                if asynchronous {
                    let raw = self.reg();
                    self.emit(Op::AsyncNext(iterator, next, sync, raw));
                    let result = self.reg();
                    self.emit(Op::Await(raw, result));
                    self.emit(Op::IteratorStep(result, iterator, value, done));
                } else {
                    self.emit(Op::IteratorNext(iterator, next, value, done, !enumeration));
                }
                let branch = self.emit(Op::Branch(done, 0, 0));
                let body_start = self.ops.len();
                if lexical {
                    self.emit(Op::Enter(vec![declaration.clone()]));
                }
                let declarations = [declaration];
                self.using_scope(&declarations, |c| {
                    if kind == "assignment" {
                        c.eval(Expr::Destructure(
                            Box::new(binding.clone()),
                            Box::new(read(value)),
                        ));
                    } else {
                        c.emit(Op::Bind(binding.clone(), value, kind == "var"));
                        if super::parser::is_using(kind) {
                            c.emit(Op::Using(value, kind == "await using"));
                        }
                    }
                    c.statement(body)
                })?;
                if lexical {
                    self.emit(Op::Exit);
                }
                self.emit(Op::Jump(again));
                let exit = self.emit(Op::EndLoop);
                let end = self.ops.len();
                self.ops[branch] = Op::Branch(done, exit, body_start);
                self.ops[handler] = Op::Loop(end, again);
            }
            s => {
                self.emit(Op::Sync(s.clone()));
            }
        }
        Ok(())
    }
}
pub(super) fn compile(body: &[Stmt]) -> Result<Arc<Program>> {
    compile_kind(body, false)
}
pub(super) fn compile_kind(body: &[Stmt], async_generator: bool) -> Result<Arc<Program>> {
    let mut c = Compiler {
        ops: vec![],
        next: 0,
        async_generator,
    };
    c.body(body, false)?;
    if c.ops.len() > 16384 {
        return Err(ScriptError::Runtime("async instruction limit"));
    }
    Ok(Arc::new(Program {
        ops: c.ops,
        registers: c.next,
    }))
}
pub(super) fn compile_module(body: &[Stmt]) -> Result<Arc<Program>> {
    let mut program = compile(body)?;
    // Module functions were instantiated before any dependency executes.
    if let Op::Hoist(_) = &program.ops[0] {
        Arc::get_mut(&mut program).unwrap().ops[0] = Op::Jump(1);
    }
    Ok(program)
}
#[derive(Clone)]
enum Completion {
    Normal,
    Return(Value),
    Throw(Value),
    Break(Option<String>),
    Continue,
}
#[derive(Clone, PartialEq)]
enum Phase {
    Try,
    Catch,
    Finally,
}
#[derive(Clone)]
struct Handler {
    loop_to: Option<(usize, usize)>,
    label: Option<String>,
    iterator: Option<usize>,
    catch: Option<usize>,
    finally: Option<usize>,
    end: usize,
    depth: usize,
    phase: Phase,
    pending: Completion,
}
#[derive(Clone)]
pub(super) enum Target {
    Promise(super::promises::Capability),
    Generator(Id),
    AsyncGenerator(Id),
}
/// Why an async-generator frame is suspended on an await that is not an `await` expression.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Pending {
    Return,
    Delegate(super::generators::Resume),
}
#[derive(Clone)]
pub(super) struct Activation {
    program: Arc<Program>,
    ip: usize,
    envs: Vec<Id>,
    root: Id,
    target: Target,
    handlers: Vec<Handler>,
    incoming: Option<Value>,
    awaiting: Option<usize>,
    delegate: Option<super::delegation::Delegation>,
    pending: Option<Pending>,
}
impl Activation {
    pub fn async_generator(&self) -> Option<Id> {
        match self.target {
            Target::AsyncGenerator(id) => Some(id),
            _ => None,
        }
    }
    pub fn edges(&self) -> Vec<Id> {
        let mut ids = self.envs.clone();
        ids.push(self.root);
        match &self.target {
            Target::Promise(cap) => ids.extend(cap.values().iter().filter_map(Value::id)),
            Target::Generator(id) | Target::AsyncGenerator(id) => ids.push(*id),
        }
        if let Some(delegate) = &self.delegate {
            ids.extend(delegate.iterator.id());
            ids.extend(delegate.next.id());
        }
        ids.extend(self.incoming.as_ref().and_then(Value::id));
        for h in &self.handlers {
            match &h.pending {
                Completion::Return(v) | Completion::Throw(v) => ids.extend(v.id()),
                _ => {}
            }
        }
        ids
    }
}
impl Engine<'_> {
    pub(super) fn start_async(
        &mut self,
        function: Id,
        env: Id,
        params: std::rc::Rc<Vec<Param>>,
        body: std::rc::Rc<Vec<Stmt>>,
        kind: FunctionKind,
        receiver: Value,
        args: Vec<Value>,
        program: Arc<Program>,
    ) -> Result<Value> {
        let cap = self.capability(Value::Object(
            self.intrinsics.as_ref().unwrap().functions["Promise"],
        ))?;
        let local =
            match self.function_environment(function, env, &params, &body, kind, receiver, &args) {
                Ok(local) => local,
                Err(ScriptError::Thrown(error)) => {
                    self.protect(&error.value)?;
                    self.invoke(cap.reject, vec![error.value])?;
                    return Ok(cap.promise);
                }
                Err(error) => return Err(error),
            };
        self.start_program(local, program, cap)
    }
    pub(super) fn start_program(
        &mut self,
        local: Id,
        program: Arc<Program>,
        cap: super::promises::Capability,
    ) -> Result<Value> {
        let id = self.new_activation(local, program, Target::Promise(cap.clone()))?;
        self.resume_async(id, Value::Undefined, false)?;
        Ok(cap.promise)
    }
    pub(super) fn new_activation(
        &mut self,
        local: Id,
        program: Arc<Program>,
        target: Target,
    ) -> Result<Id> {
        let id = self.new_object(None)?;
        self.protect(&Value::Object(id))?;
        let mut data = self.data(id)?.clone();
        data.activation = Some(Activation {
            program,
            ip: 0,
            envs: vec![local],
            root: local,
            target,
            handlers: vec![],
            incoming: None,
            awaiting: None,
            delegate: None,
            pending: None,
        });
        self.replace_data(id, data)?;
        Ok(id)
    }
    fn register(&mut self, frame: &Activation, r: usize) -> Result<Value> {
        self.lookup(frame.root, &name(r))
    }
    fn store(&mut self, frame: &Activation, r: usize, value: Value) -> Result<()> {
        self.initialize(frame.root, &name(r), value)
    }
    fn save_activation(&mut self, id: Id, frame: Activation) -> Result<()> {
        let mut data = self.data(id)?.clone();
        data.activation = Some(frame);
        self.replace_data(id, data)
    }
    fn finish_activation(
        &mut self,
        id: Id,
        frame: &Activation,
        completion: Completion,
    ) -> Result<()> {
        let (value, reject) = match completion {
            Completion::Return(v) => (v, false),
            Completion::Throw(v) => (v, true),
            Completion::Normal => (Value::Undefined, false),
            _ => return Err(ScriptError::Runtime("loop control escaped async function")),
        };
        self.protect(&value)?;
        for r in 0..frame.program.registers {
            self.initialize(frame.root, &name(r), Value::Undefined)?;
        }
        let mut data = self.data(id)?.clone();
        data.activation = None;
        self.replace_data(id, data)?;
        match &frame.target {
            Target::Promise(cap) => {
                let resolve = if reject {
                    cap.reject.clone()
                } else {
                    cap.resolve.clone()
                };
                self.invoke(resolve, vec![value])?;
            }
            Target::Generator(generator) => {
                self.generator_output(*generator, value, reject, true)?
            }
            Target::AsyncGenerator(generator) => {
                self.async_generator_settle(*generator, value, reject, true)?
            }
        }

        Ok(())
    }
    fn route_async(
        &mut self,
        id: Id,
        frame: &mut Activation,
        mut completion: Completion,
    ) -> Result<bool> {
        if let Completion::Return(v) | Completion::Throw(v) = &completion {
            self.protect(v)?;
        }
        loop {
            let Some(h) = frame.handlers.last_mut() else {
                self.finish_activation(id, frame, completion)?;
                return Ok(true);
            };
            frame.envs.truncate(h.depth);
            if let Some((stop, again)) = h.loop_to {
                if let Some(label) = &h.label {
                    let hit = matches!(&completion, Completion::Break(Some(l)) if l == label);
                    frame.handlers.pop();
                    if hit {
                        frame.ip = stop;
                        return Ok(false);
                    }
                    continue;
                }
                if matches!(completion, Completion::Continue) && again != usize::MAX {
                    frame.ip = again;
                    return Ok(false);
                }
                let iterator = h.iterator;
                frame.handlers.pop();
                if let Some(iterator) = iterator {
                    let iterator = self.register(frame, iterator)?;
                    let pending = if let Completion::Throw(value) = &completion {
                        Err(self.thrown(value.clone()))
                    } else {
                        Ok(None)
                    };
                    match self.close_iterator(iterator, pending) {
                        Ok(_) => {}
                        Err(ScriptError::Thrown(error)) => {
                            self.protect(&error.value)?;
                            completion = Completion::Throw(error.value);
                        }
                        Err(error) => return Err(error),
                    }
                }
                if matches!(completion, Completion::Break(None)) {
                    frame.ip = stop;
                    return Ok(false);
                }
                continue;
            }
            if matches!(completion, Completion::Throw(_))
                && h.phase == Phase::Try
                && h.catch.is_some()
            {
                h.phase = Phase::Catch;
                frame.incoming = if let Completion::Throw(v) = completion {
                    Some(v)
                } else {
                    None
                };
                frame.ip = h.catch.unwrap();
                return Ok(false);
            }
            if h.phase != Phase::Finally && h.finally.is_some() {
                h.phase = Phase::Finally;
                h.pending = completion;
                frame.ip = h.finally.unwrap();
                return Ok(false);
            }
            frame.handlers.pop();
        }
    }
    fn process_delegate(
        &mut self,
        id: Id,
        frame: &mut Activation,
        input: Value,
        mode: super::generators::Resume,
    ) -> Result<Option<Completion>> {
        let delegate = frame.delegate.clone().unwrap();
        let result = self.delegate_call(&delegate, input, mode);
        match result {
            Ok(super::delegation::Delegated::Yield(result)) => {
                let Target::Generator(generator) = frame.target.clone() else {
                    return Err(ScriptError::Unsupported("yield-star outside generator"));
                };
                self.save_activation(id, frame.clone())?;
                self.forward_generator_result(generator, result)?;
                Ok(Some(Completion::Normal))
            }
            Ok(super::delegation::Delegated::Next(value)) => {
                frame.delegate = None;
                self.store(frame, delegate.output, value)?;
                Ok(None)
            }
            Ok(super::delegation::Delegated::Return(value)) => {
                frame.delegate = None;
                Ok(Some(Completion::Return(value)))
            }
            Err(error) => {
                frame.delegate = None;
                Err(error)
            }
        }
    }
    /// Applies one step outcome; true when the activation suspended or finished.
    fn settle_step(
        &mut self,
        id: Id,
        frame: &mut Activation,
        step: Result<Option<Completion>>,
    ) -> Result<bool> {
        match step {
            Ok(Some(Completion::Normal)) => Ok(true),
            Ok(Some(completion)) => self.route_async(id, frame, completion),
            Err(ScriptError::Thrown(error)) => {
                self.route_async(id, frame, Completion::Throw(error.value))
            }
            Err(error) => Err(error),
            Ok(None) => Ok(false),
        }
    }
    fn await_value(
        &mut self,
        id: Id,
        frame: &mut Activation,
        value: Value,
        out: Option<usize>,
        pending: Option<Pending>,
    ) -> Result<Option<Completion>> {
        let promise = self.promise_resolve(
            Value::Object(self.intrinsics.as_ref().unwrap().functions["Promise"]),
            value,
        )?;
        self.protect(&promise)?;
        frame.awaiting = Some(out.unwrap_or(usize::MAX));
        frame.pending = pending;
        self.save_activation(id, frame.clone())?;
        let yes = self.internal_function(
            super::promises::Internal::AsyncResume {
                activation: id,
                reject: false,
            },
            1,
        )?;
        let no = self.internal_function(
            super::promises::Internal::AsyncResume {
                activation: id,
                reject: true,
            },
            1,
        )?;
        let Value::Object(p) = promise else {
            unreachable!()
        };
        self.attach_reaction(p, Some(yes), Some(no), None)?;
        Ok(Some(Completion::Normal))
    }
    /// `return v` in an async generator awaits v before completing.
    fn return_value(
        &mut self,
        id: Id,
        frame: &mut Activation,
        value: Value,
    ) -> Result<Option<Completion>> {
        if frame.async_generator().is_some() {
            self.await_value(id, frame, value, None, Some(Pending::Return))
        } else {
            Ok(Some(Completion::Return(value)))
        }
    }
    /// CreateAsyncFromSyncIterator step: a sync result becomes a promise for {value, done}.
    pub(super) fn async_from_sync(&mut self, result: Value) -> Result<Value> {
        if !matches!(result, Value::Object(_)) {
            return self.throw_error("TypeError", "iterator result is not an object");
        }
        let done = self.get_member(result.clone(), "done")?.truthy();
        let value = self.get_member(result, "value")?;
        self.protect(&value)?;
        let ctor = Value::Object(self.intrinsics.as_ref().unwrap().functions["Promise"]);
        let promise = self.promise_resolve(ctor.clone(), value)?;
        self.protect(&promise)?;
        let cap = self.capability(ctor)?;
        let wrap = self.internal_function(super::promises::Internal::IterResult { done }, 1)?;
        let Value::Object(p) = promise else {
            unreachable!()
        };
        self.attach_reaction(p, Some(wrap), None, Some(cap.clone()))?;
        Ok(cap.promise)
    }
    fn async_delegate_call(
        &mut self,
        id: Id,
        frame: &mut Activation,
        input: Value,
        mode: super::generators::Resume,
    ) -> Result<Option<Completion>> {
        let result = self.async_delegate_call_inner(id, frame, input, mode);
        if result.is_err() {
            frame.delegate = None;
        }
        result
    }
    fn async_delegate_call_inner(
        &mut self,
        id: Id,
        frame: &mut Activation,
        input: Value,
        mode: super::generators::Resume,
    ) -> Result<Option<Completion>> {
        use super::generators::Resume;
        let delegate = frame.delegate.clone().unwrap();
        let method = if mode == Resume::Next {
            delegate.next.clone()
        } else {
            let method = self.get_member(
                delegate.iterator.clone(),
                if mode == Resume::Throw {
                    "throw"
                } else {
                    "return"
                },
            )?;
            self.protect(&method)?;
            if matches!(method, Value::Null | Value::Undefined) {
                frame.delegate = None;
                if mode == Resume::Return {
                    return self.await_value(id, frame, input, None, Some(Pending::Return));
                }
                self.close_iterator(delegate.iterator.clone(), Ok(None))?;
                return self.throw_error("TypeError", "delegated iterator has no throw method");
            }
            method
        };
        let result = self.invoke_this(method, delegate.iterator.clone(), vec![input])?;
        self.protect(&result)?;
        let result = if delegate.sync {
            self.async_from_sync(result)?
        } else {
            result
        };
        self.await_value(id, frame, result, None, Some(Pending::Delegate(mode)))
    }
    fn async_delegate_result(
        &mut self,
        id: Id,
        frame: &mut Activation,
        result: Value,
        mode: super::generators::Resume,
    ) -> Result<Option<Completion>> {
        let step = (|| -> Result<Option<Completion>> {
            if !matches!(result, Value::Object(_)) {
                return self.throw_error("TypeError", "delegated iterator result is not an object");
            }
            let done = self.get_member(result.clone(), "done")?.truthy();
            let value = self.get_member(result, "value")?;
            self.protect(&value)?;
            if done {
                let output = frame.delegate.take().unwrap().output;
                if mode == super::generators::Resume::Return {
                    return Ok(Some(Completion::Return(value)));
                }
                self.store(frame, output, value)?;
                return Ok(None);
            }
            self.save_activation(id, frame.clone())?;
            let generator = frame.async_generator().unwrap();
            self.async_generator_settle(generator, value, false, false)?;
            Ok(Some(Completion::Normal))
        })();
        if step.is_err() {
            frame.delegate = None;
        }
        step
    }
    pub(super) fn resume_async(&mut self, id: Id, value: Value, reject: bool) -> Result<Value> {
        self.resume_program(
            id,
            value,
            if reject {
                super::generators::Resume::Throw
            } else {
                super::generators::Resume::Next
            },
        )
    }
    pub(super) fn resume_program(
        &mut self,
        id: Id,
        value: Value,
        mode: super::generators::Resume,
    ) -> Result<Value> {
        self.frame(|e| {
            e.protect(&Value::Object(id))?;
            e.protect(&value)?;
            let Some(mut frame) = e.data(id)?.activation.clone() else {
                return Ok(Value::Undefined);
            };
            if let Some(pending) = frame.pending.take() {
                frame.awaiting = None;
                let step = match (pending, mode) {
                    (_, super::generators::Resume::Throw) => {
                        frame.delegate = None;
                        Ok(Some(Completion::Throw(value)))
                    }
                    (Pending::Return, _) => Ok(Some(Completion::Return(value))),
                    (Pending::Delegate(m), _) => e.async_delegate_result(id, &mut frame, value, m),
                };
                if e.settle_step(id, &mut frame, step)? {
                    return Ok(Value::Undefined);
                }
            } else if frame.delegate.is_some() {
                let step = if frame.async_generator().is_some() {
                    e.async_delegate_call(id, &mut frame, value, mode)
                } else {
                    e.process_delegate(id, &mut frame, value, mode)
                };
                if e.settle_step(id, &mut frame, step)? {
                    return Ok(Value::Undefined);
                }
            } else if let Some(register) = frame.awaiting.take() {
                if mode == super::generators::Resume::Return && frame.async_generator().is_some() {
                    let step = e.await_value(id, &mut frame, value, None, Some(Pending::Return));
                    if e.settle_step(id, &mut frame, step)? {
                        return Ok(Value::Undefined);
                    }
                } else if mode != super::generators::Resume::Next {
                    let completion = if mode == super::generators::Resume::Throw {
                        Completion::Throw(value)
                    } else {
                        Completion::Return(value)
                    };
                    if e.route_async(id, &mut frame, completion)? {
                        return Ok(Value::Undefined);
                    }
                } else {
                    e.store(&frame, register, value)?;
                }
            }
            loop {
                e.tick()?;
                if frame.ip >= frame.program.ops.len() {
                    e.finish_activation(id, &frame, Completion::Normal)?;
                    return Ok(Value::Undefined);
                }
                let op = frame.program.ops[frame.ip].clone();
                frame.ip += 1;
                let env = *frame.envs.last().unwrap();
                let step = (|| -> Result<Option<Completion>> {
                    match op {
                        Op::Enumerate(source, iterator, next) => {
                            let source = e.register(&frame, source)?;
                            let value = e.enumeration(source)?;
                            e.store(&frame, iterator, value)?;
                            let method = e.intrinsics.as_ref().unwrap().functions
                                ["%ForInIteratorPrototype%.next"];
                            e.store(&frame, next, Value::Object(method))?;
                        }
                        Op::Iterator(source, iterator, next) => {
                            let source = e.register(&frame, source)?;
                            let method =
                                e.get_member(source.clone(), super::objects::Key::Symbol(0))?;
                            e.protect(&method)?;
                            let (value, method) = e.open_iterator(source, method)?;
                            e.store(&frame, iterator, value)?;
                            e.store(&frame, next, method)?;
                        }
                        Op::IteratorNext(iterator, next, value, done, close) => {
                            frame.handlers.last_mut().unwrap().iterator = None;
                            let target = e.register(&frame, iterator)?;
                            let next = e.register(&frame, next)?;
                            let step = e.invoke_this(next, target, vec![])?;
                            e.protect(&step)?;
                            if !matches!(step, Value::Object(_)) {
                                return e
                                    .throw_error("TypeError", "iterator result is not an object");
                            }
                            let finished = e.get_member(step.clone(), "done")?.truthy();
                            let result = if finished {
                                Value::Undefined
                            } else {
                                e.get_member(step, "value")?
                            };
                            e.store(&frame, value, result)?;
                            e.store(&frame, done, Value::Bool(finished))?;
                            if !finished && close {
                                frame.handlers.last_mut().unwrap().iterator = Some(iterator);
                            }
                        }
                        Op::AsyncIterator(source, iterator, next, sync) => {
                            let source = e.register(&frame, source)?;
                            let method = e.get_member(
                                source.clone(),
                                super::objects::Key::Symbol(ASYNC_ITERATOR),
                            )?;
                            e.protect(&method)?;
                            let (method, wrapped) =
                                if matches!(method, Value::Null | Value::Undefined) {
                                    let method = e.get_member(
                                        source.clone(),
                                        super::objects::Key::Symbol(0),
                                    )?;
                                    e.protect(&method)?;
                                    (method, true)
                                } else {
                                    (method, false)
                                };
                            let (value, method) = e.open_iterator(source, method)?;
                            e.store(&frame, iterator, value)?;
                            e.store(&frame, next, method)?;
                            e.store(&frame, sync, Value::Bool(wrapped))?;
                        }
                        Op::AsyncNext(iterator, next, sync, out) => {
                            frame.handlers.last_mut().unwrap().iterator = None;
                            let target = e.register(&frame, iterator)?;
                            let next = e.register(&frame, next)?;
                            let result = e.invoke_this(next, target, vec![])?;
                            e.protect(&result)?;
                            let result = if e.register(&frame, sync)?.truthy() {
                                e.async_from_sync(result)?
                            } else {
                                result
                            };
                            e.store(&frame, out, result)?;
                        }
                        Op::IteratorStep(result, iterator, value, done) => {
                            let step = e.register(&frame, result)?;
                            if !matches!(step, Value::Object(_)) {
                                return e
                                    .throw_error("TypeError", "iterator result is not an object");
                            }
                            let finished = e.get_member(step.clone(), "done")?.truthy();
                            let result = if finished {
                                Value::Undefined
                            } else {
                                e.get_member(step, "value")?
                            };
                            e.store(&frame, value, result)?;
                            e.store(&frame, done, Value::Bool(finished))?;
                            if !finished {
                                frame.handlers.last_mut().unwrap().iterator = Some(iterator);
                            }
                        }
                        Op::Loop(stop, again) => frame.handlers.push(Handler {
                            loop_to: Some((stop, again)),
                            label: None,
                            iterator: None,
                            catch: None,
                            finally: None,
                            end: stop,
                            depth: frame.envs.len(),
                            phase: Phase::Try,
                            pending: Completion::Normal,
                        }),
                        Op::Label(stop, label) => frame.handlers.push(Handler {
                            loop_to: Some((stop, usize::MAX)),
                            label: Some(label),
                            iterator: None,
                            catch: None,
                            finally: None,
                            end: stop,
                            depth: frame.envs.len(),
                            phase: Phase::Try,
                            pending: Completion::Normal,
                        }),
                        Op::EndLoop => {
                            frame.handlers.pop();
                        }
                        Op::CloneIteration(names) => {
                            let next = e.clone_iteration(env, &names)?;
                            *frame.envs.last_mut().unwrap() = next;
                        }
                        Op::CheckBase(r) => {
                            let base = e.register(&frame, r)?;
                            if matches!(base, Value::Null | Value::Undefined) {
                                return e.throw_error("TypeError", "nullish property receiver");
                            }
                        }
                        Op::PropertyKey(base, key, out) => {
                            let base = e.register(&frame, base)?;
                            let key = e.register(&frame, key)?;
                            let key = e.member_key(&base, key)?;
                            e.store(
                                &frame,
                                out,
                                match key {
                                    super::objects::Key::String(s) => Value::String(s.into()),
                                    super::objects::Key::Symbol(s) => Value::Symbol(s),
                                },
                            )?;
                        }
                        Op::Eval(expr, out) => {
                            let v = e.eval(&expr, env)?;
                            e.store(&frame, out, v)?;
                        }
                        Op::EvalNamed(expr, name, out) => {
                            let v = e.eval_named(&expr, env, &name)?;
                            e.store(&frame, out, v)?;
                        }
                        Op::Append(array, value, spread) => {
                            let Value::Object(array) = e.register(&frame, array)? else {
                                unreachable!()
                            };
                            let value = value.map(|r| e.register(&frame, r)).transpose()?;
                            e.append_array(array, value, spread)?;
                        }
                        Op::InvokeList {
                            callee,
                            receiver,
                            args,
                            new,
                            out,
                        } => {
                            let f = e.register(&frame, callee)?;
                            let this = if let Some(r) = receiver {
                                e.register(&frame, r)?
                            } else {
                                Value::Undefined
                            };
                            let Value::Object(array) = e.register(&frame, args)? else {
                                unreachable!()
                            };
                            let args = e.argument_values(array)?;
                            let value = if new {
                                e.construct(f, args)?
                            } else {
                                e.invoke_this(f, this, args)?
                            };
                            e.store(&frame, out, value)?;
                        }
                        Op::Invoke {
                            callee,
                            receiver,
                            args,
                            new,
                            out,
                        } => {
                            let f = e.register(&frame, callee)?;
                            let this = if let Some(r) = receiver {
                                e.register(&frame, r)?
                            } else {
                                Value::Undefined
                            };
                            let args = args
                                .into_iter()
                                .map(|r| e.register(&frame, r))
                                .collect::<Result<Vec<_>>>()?;
                            let v = if new {
                                e.construct(f, args)?
                            } else {
                                e.invoke_this(f, this, args)?
                            };
                            e.store(&frame, out, v)?;
                        }
                        Op::Bind(pattern, value, var) => {
                            let value = e.register(&frame, value)?;
                            let target = if var { e.var_environment(env)? } else { env };
                            e.bind_pattern(&pattern, value, env, target)?;
                        }
                        Op::Init(n, r, var) => {
                            let v = e.register(&frame, r)?;
                            let target = if var { e.var_environment(env)? } else { env };
                            e.initialize(target, &n, v)?;
                        }
                        Op::Enter(body) => {
                            let scope = e.new_environment(Some(env), &body, false)?;
                            e.protect(&Value::Object(scope))?;
                            e.hoist_functions(&body, scope)?;
                            frame.envs.push(scope);
                        }
                        Op::Exit => {
                            frame.envs.pop();
                        }
                        Op::Hoist(body) => e.hoist_functions(&body, env)?,
                        Op::Branch(r, a, b) => {
                            frame.ip = if e.register(&frame, r)?.truthy() {
                                a
                            } else {
                                b
                            }
                        }
                        Op::Jump(ip) => frame.ip = ip,
                        Op::Delegate(source, out) => {
                            let source = e.register(&frame, source)?;
                            let asynchronous = frame.async_generator().is_some();
                            let mut sync = false;
                            let mut method = Value::Undefined;
                            if asynchronous {
                                method = e.get_member(
                                    source.clone(),
                                    super::objects::Key::Symbol(ASYNC_ITERATOR),
                                )?;
                                e.protect(&method)?;
                            }
                            if matches!(method, Value::Null | Value::Undefined) {
                                sync = asynchronous;
                                method =
                                    e.get_member(source.clone(), super::objects::Key::Symbol(0))?;
                                e.protect(&method)?;
                            }
                            let (iterator, next) = e.open_iterator(source, method)?;
                            frame.delegate = Some(super::delegation::Delegation {
                                iterator,
                                next,
                                output: out,
                                sync,
                            });
                            if asynchronous {
                                return e.async_delegate_call(
                                    id,
                                    &mut frame,
                                    Value::Undefined,
                                    super::generators::Resume::Next,
                                );
                            }
                            return e.process_delegate(
                                id,
                                &mut frame,
                                Value::Undefined,
                                super::generators::Resume::Next,
                            );
                        }
                        Op::Yield(r, out) => {
                            if let Some(generator) = frame.async_generator() {
                                let value = e.register(&frame, r)?;
                                frame.awaiting = Some(out);
                                e.save_activation(id, frame.clone())?;
                                e.async_generator_settle(generator, value, false, false)?;
                                return Ok(Some(Completion::Normal));
                            }
                            let Target::Generator(generator) = frame.target.clone() else {
                                return Err(ScriptError::Unsupported(
                                    "yield outside generator activation",
                                ));
                            };
                            let value = e.register(&frame, r)?;
                            frame.awaiting = Some(out);
                            e.save_activation(id, frame.clone())?;
                            e.generator_output(generator, value, false, false)?;
                            return Ok(Some(Completion::Normal));
                        }
                        Op::Await(r, out) => {
                            let value = e.register(&frame, r)?;
                            return e.await_value(id, &mut frame, value, Some(out), None);
                        }
                        Op::Return(r) => {
                            let value = e.register(&frame, r)?;
                            return e.return_value(id, &mut frame, value);
                        }
                        Op::Throw(r) => return Ok(Some(Completion::Throw(e.register(&frame, r)?))),
                        Op::TemplateObject(template, out) => {
                            let value = e.template_object(&template, env)?;
                            e.store(&frame, out, value)?;
                        }
                        Op::TemplateStep(text, value, part, out) => {
                            let Value::String(text) = e.register(&frame, text)? else {
                                return Err(ScriptError::Runtime("invalid template accumulator"));
                            };
                            let value = e.register(&frame, value)?;
                            let text = e.template_text(text, value, &part)?;
                            e.store(&frame, out, Value::String(text.into()))?;
                        }
                        Op::Object(out) => {
                            let object =
                                e.new_object(Some(e.intrinsics.as_ref().unwrap().object))?;
                            e.store(&frame, out, Value::Object(object))?;
                        }
                        Op::Array(out, len) => {
                            let object = e.new_array(len)?;
                            e.store(&frame, out, Value::Object(object))?;
                        }
                        Op::Property(r, key, p) => {
                            let Value::Object(object) = e.register(&frame, r)? else {
                                unreachable!()
                            };
                            e.literal_property(object, &key, &p, env)?;
                        }
                        Op::Try {
                            catch,
                            finally,
                            end,
                        } => frame.handlers.push(Handler {
                            label: None,
                            loop_to: None,
                            iterator: None,
                            catch,
                            finally,
                            end,
                            depth: frame.envs.len(),
                            phase: Phase::Try,
                            pending: Completion::Normal,
                        }),
                        Op::Catch(pattern, body) => {
                            let value = frame.incoming.take().unwrap();
                            e.protect(&value)?;
                            let declarations = pattern
                                .iter()
                                .map(|pattern| Stmt::Bind {
                                    kind: "let".into(),
                                    pattern: pattern.clone(),
                                    init: Some(Expr::Undefined),
                                })
                                .collect::<Vec<_>>();
                            let scope = e.new_environment(Some(env), &declarations, false)?;
                            e.protect(&Value::Object(scope))?;
                            if let Some(pattern) = pattern {
                                e.bind_pattern(&pattern, value, scope, scope)?;
                            }
                            frame.envs.push(scope);
                            let body_scope = e.new_environment(Some(scope), &body, false)?;
                            e.protect(&Value::Object(body_scope))?;
                            e.hoist_functions(&body, body_scope)?;
                            frame.envs.push(body_scope);
                        }
                        Op::EndTry => {
                            let h = frame.handlers.last_mut().unwrap();
                            frame.envs.truncate(h.depth);
                            if let Some(ip) = h.finally {
                                h.phase = Phase::Finally;
                                h.pending = Completion::Normal;
                                frame.ip = ip;
                            } else {
                                frame.ip = h.end;
                                frame.handlers.pop();
                            }
                        }
                        Op::EndFinally => {
                            let h = frame.handlers.pop().unwrap();
                            frame.envs.truncate(h.depth);
                            if matches!(h.pending, Completion::Normal) {
                                frame.ip = h.end;
                            } else {
                                return Ok(Some(h.pending));
                            }
                        }
                        Op::DisposeStart(r) => {
                            let scope = e.new_using_scope()?;
                            e.initialize(env, "%dispose", scope.clone())?;
                            e.store(&frame, r, scope)?;
                        }
                        Op::Using(r, asynchronous) => {
                            let value = e.register(&frame, r)?;
                            let scope = e.lookup(env, "%dispose")?;
                            e.add_using_resource(&scope, value, asynchronous)?;
                        }
                        Op::DisposeSync(r) => {
                            let scope = e.register(&frame, r)?;
                            let resources = e.take_using_resources(&scope)?;
                            let error = match &frame.handlers.last().unwrap().pending {
                                Completion::Throw(v) => Some(v.clone()),
                                _ => None,
                            };
                            if let Some(error) = e.dispose_sync(resources, error)? {
                                e.protect(&error)?;
                                frame.handlers.last_mut().unwrap().pending =
                                    Completion::Throw(error);
                            }
                        }
                        Op::DisposeAsync(r, out, skip) => {
                            let scope = e.register(&frame, r)?;
                            let resources = e.take_using_resources(&scope)?;
                            // §9.11.6 DisposeResources: no resource recorded, no Await.
                            if resources.is_empty() {
                                frame.ip = skip;
                                return Ok(None);
                            }
                            let h = frame.handlers.last_mut().unwrap();
                            let error = match std::mem::replace(&mut h.pending, Completion::Normal)
                            {
                                Completion::Throw(v) => Some(v),
                                other => {
                                    h.pending = other;
                                    None
                                }
                            };
                            let promise = e.dispose_async(resources, error)?;
                            e.store(&frame, out, promise)?;
                        }
                        Op::DisposeRejected => {
                            let value = frame.incoming.take().unwrap();
                            e.protect(&value)?;
                            let n = frame.handlers.len();
                            frame.handlers[n - 2].pending = Completion::Throw(value);
                        }
                        Op::Sync(s) => match e.statement(&s, env, true) {
                            Ok(Some(v)) => return e.return_value(id, &mut frame, v),
                            Err(ScriptError::LoopBreak) => {
                                return Ok(Some(Completion::Break(None)))
                            }
                            Err(ScriptError::LabelBreak(label)) => {
                                return Ok(Some(Completion::Break(Some(label))))
                            }
                            Err(ScriptError::LoopContinue) => {
                                return Ok(Some(Completion::Continue))
                            }
                            other => {
                                other?;
                            }
                        },
                    }
                    Ok(None)
                })();
                match step {
                    Ok(Some(Completion::Normal)) => return Ok(Value::Undefined),
                    Ok(Some(c)) => {
                        if e.route_async(id, &mut frame, c)? {
                            return Ok(Value::Undefined);
                        }
                    }
                    Err(ScriptError::Thrown(error)) => {
                        if e.route_async(id, &mut frame, Completion::Throw(error.value))? {
                            return Ok(Value::Undefined);
                        }
                    }
                    Err(error) => return Err(error),
                    Ok(None) => {}
                }
                e.save_activation(id, frame.clone())?;
            }
        })
    }
}
