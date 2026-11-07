//! Resumable control flow; await-free operations reuse the existing evaluator.
use super::{
    parser::{Expr, Property, Stmt},
    vm::{Engine, FunctionKind, ScriptError, Value},
};
use crate::dual::Id;
use std::sync::Arc;
type Result<T> = std::result::Result<T, ScriptError>;
#[derive(Clone)]
pub(super) enum Op {
    Eval(Expr, usize),
    Invoke {
        callee: usize,
        receiver: Option<usize>,
        args: Vec<usize>,
        new: bool,
        out: usize,
    },
    Init(String, usize, bool),
    Enter(Vec<Stmt>),
    Exit,
    Hoist(Vec<Stmt>),
    Branch(usize, usize, usize),
    Jump(usize),
    Await(usize, usize),
    Return(usize),
    Throw(usize),
    Object(usize),
    Array(usize, u32),
    Property(usize, String, Property),
    Item(usize, String, usize),
    Try {
        catch: Option<usize>,
        finally: Option<usize>,
        end: usize,
    },
    Catch(String, Vec<Stmt>),
    EndTry,
    EndFinally,
    Sync(Stmt),
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
        Expr::Await(_) => true,
        Expr::Function { .. }
        | Expr::Arrow { .. }
        | Expr::AsyncFunction { .. }
        | Expr::AsyncArrow { .. } => false,
        Expr::Member(a, _) | Expr::Unary(_, a) => expr_await(a),
        Expr::Computed(a, b) | Expr::Binary(_, a, b) | Expr::Assign(a, b) => {
            expr_await(a) || expr_await(b)
        }
        Expr::Call(a, b) | Expr::New(a, b) => expr_await(a) || b.iter().any(expr_await),
        Expr::Array(e) => e.iter().flatten().any(expr_await),
        Expr::Object(props) => props
            .iter()
            .any(|(_, p)| matches!(p,Property::Value(e)|Property::Prototype(e)if expr_await(e))),
        _ => false,
    }
}
pub(super) fn stmt_await(s: &Stmt) -> bool {
    match s {
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
                || catch
                    .as_ref()
                    .is_some_and(|(_, b)| b.iter().any(stmt_await))
                || finally.as_ref().is_some_and(|b| b.iter().any(stmt_await))
        }
        Stmt::ForOf { iterable, body, .. } => expr_await(iterable) || stmt_await(body),
        _ => false,
    }
}
struct Compiler {
    ops: Vec<Op>,
    next: usize,
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
    fn eval(&mut self, e: Expr) -> usize {
        let r = self.reg();
        self.emit(Op::Eval(e, r));
        r
    }
    fn reference(&mut self, e: &Expr) -> Result<(usize, Expr)> {
        match e {
            Expr::Member(base, key) => {
                let base = self.expression(base)?;
                Ok((base, Expr::Member(Box::new(read(base)), key.clone())))
            }
            Expr::Computed(base, key) => {
                let base = self.expression(base)?;
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
                let args = args
                    .iter()
                    .map(|e| self.expression(e))
                    .collect::<Result<Vec<_>>>()?;
                let out = self.reg();
                self.emit(Op::Invoke {
                    callee,
                    receiver,
                    args,
                    new,
                    out,
                });
                out
            }
            Expr::Member(..) | Expr::Computed(..) => {
                let (_, reference) = self.reference(e)?;
                self.eval(reference)
            }
            Expr::Assign(target, value) => {
                let target = if matches!(target.as_ref(), Expr::Name(_)) {
                    target.as_ref().clone()
                } else {
                    self.reference(target)?.1
                };
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
            Expr::Binary(op, a, b) if op == "&&" || op == "||" => {
                let left = self.expression(a)?;
                let out = self.reg();
                let branch = self.emit(Op::Branch(left, 0, 0));
                let right_start = self.ops.len();
                let right = self.expression(b)?;
                self.emit(Op::Eval(read(right), out));
                let jump = self.emit(Op::Jump(0));
                let short = self.ops.len();
                self.emit(Op::Eval(read(left), out));
                let end = self.ops.len();
                self.ops[jump] = Op::Jump(end);
                self.ops[branch] = if op == "&&" {
                    Op::Branch(left, right_start, short)
                } else {
                    Op::Branch(left, short, right_start)
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
                self.emit(Op::Array(out, elements.len() as u32));
                for (i, e) in elements.iter().enumerate() {
                    if let Some(e) = e {
                        let value = self.expression(e)?;
                        self.emit(Op::Item(out, i.to_string(), value));
                    }
                }
                out
            }
            Expr::Object(props) => {
                let out = self.reg();
                self.emit(Op::Object(out));
                for (key, p) in props {
                    let p = match p {
                        Property::Value(e) => Property::Value(read(self.expression(e)?)),
                        Property::Prototype(e) => Property::Prototype(read(self.expression(e)?)),
                        p => p.clone(),
                    };
                    self.emit(Op::Property(out, key.clone(), p));
                }
                out
            }
            _ => return Err(ScriptError::Unsupported("await expression shape")),
        })
    }
    fn body(&mut self, body: &[Stmt], scope: bool) -> Result<()> {
        if scope {
            self.emit(Op::Enter(body.to_vec()));
        } else {
            self.emit(Op::Hoist(body.to_vec()));
        }
        for s in body {
            self.statement(s)?;
        }
        if scope {
            self.emit(Op::Exit);
        }
        Ok(())
    }
    fn statement(&mut self, s: &Stmt) -> Result<()> {
        match s {
            Stmt::Function(..) | Stmt::AsyncFunction(..) => {}
            Stmt::Const(n, e) | Stmt::Let(n, e) => {
                let value = self.expression(e)?;
                self.emit(Op::Init(n.clone(), value, false));
            }
            Stmt::Var(n, e) => {
                if let Some(e) = e {
                    let value = self.expression(e)?;
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
                let catch_ip = if let Some((name, body)) = catch {
                    let at = self.emit(Op::Catch(name.clone(), body.clone()));
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
            Stmt::ForOf { .. } if stmt_await(s) => {
                return Err(ScriptError::Unsupported(
                    "await inside for-of is not implemented",
                ))
            }
            s => {
                self.emit(Op::Sync(s.clone()));
            }
        }
        Ok(())
    }
}
pub(super) fn compile(body: &[Stmt]) -> Result<Arc<Program>> {
    let mut c = Compiler {
        ops: vec![],
        next: 0,
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
    Break,
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
    catch: Option<usize>,
    finally: Option<usize>,
    end: usize,
    depth: usize,
    phase: Phase,
    pending: Completion,
}
#[derive(Clone)]
pub(super) struct Activation {
    program: Arc<Program>,
    ip: usize,
    envs: Vec<Id>,
    root: Id,
    cap: super::promises::Capability,
    handlers: Vec<Handler>,
    incoming: Option<Value>,
    awaiting: Option<usize>,
}
impl Activation {
    pub fn edges(&self) -> Vec<Id> {
        let mut ids = self.envs.clone();
        ids.push(self.root);
        ids.extend(self.cap.values().iter().filter_map(Value::id));
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
        env: Id,
        params: Vec<String>,
        body: Vec<Stmt>,
        kind: FunctionKind,
        receiver: Value,
        args: Vec<Value>,
        program: Arc<Program>,
    ) -> Result<Value> {
        let cap = self.capability(Value::Object(
            self.intrinsics.as_ref().unwrap().functions["Promise"],
        ))?;
        let local = self.new_environment(Some(env), &body, true)?;
        self.protect(&Value::Object(local))?;
        if kind != FunctionKind::AsyncArrow {
            self.initialize(local, "this", receiver)?;
            let arguments = self.arguments_object(&args)?;
            self.initialize(local, "arguments", arguments)?;
        }
        for (i, n) in params.iter().enumerate() {
            self.mutable_binding(local, n, args.get(i).cloned().unwrap_or(Value::Undefined))?;
        }
        self.start_program(local, program, cap)
    }
    pub(super) fn start_program(
        &mut self,
        local: Id,
        program: Arc<Program>,
        cap: super::promises::Capability,
    ) -> Result<Value> {
        let id = self.new_object(None)?;
        self.protect(&Value::Object(id))?;
        let mut data = self.data(id)?.clone();
        data.activation = Some(Activation {
            program,
            ip: 0,
            envs: vec![local],
            root: local,
            cap: cap.clone(),
            handlers: vec![],
            incoming: None,
            awaiting: None,
        });
        self.replace_data(id, data)?;
        self.resume_async(id, Value::Undefined, false)?;
        Ok(cap.promise)
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
        let resolve = if reject {
            frame.cap.reject.clone()
        } else {
            frame.cap.resolve.clone()
        };
        self.protect(&resolve)?;
        for r in 0..frame.program.registers {
            self.initialize(frame.root, &name(r), Value::Undefined)?;
        }
        let mut data = self.data(id)?.clone();
        data.activation = None;
        self.replace_data(id, data)?;
        self.invoke(resolve, vec![value])?;
        Ok(())
    }
    fn route_async(
        &mut self,
        id: Id,
        frame: &mut Activation,
        completion: Completion,
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
    pub(super) fn resume_async(&mut self, id: Id, value: Value, reject: bool) -> Result<Value> {
        self.frame(|e| {
            e.protect(&Value::Object(id))?;
            e.protect(&value)?;
            let Some(mut frame) = e.data(id)?.activation.clone() else {
                return Ok(Value::Undefined);
            };
            if let Some(register) = frame.awaiting.take() {
                if reject {
                    if e.route_async(id, &mut frame, Completion::Throw(value))? {
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
                        Op::Eval(expr, out) => {
                            let v = e.eval(&expr, env)?;
                            e.store(&frame, out, v)?;
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
                        Op::Await(r, out) => {
                            let value = e.register(&frame, r)?;
                            let promise = e.promise_resolve(
                                Value::Object(e.intrinsics.as_ref().unwrap().functions["Promise"]),
                                value,
                            )?;
                            e.protect(&promise)?;
                            frame.awaiting = Some(out);
                            e.save_activation(id, frame.clone())?;
                            let yes = e.internal_function(
                                super::promises::Internal::AsyncResume {
                                    activation: id,
                                    reject: false,
                                },
                                1,
                            )?;
                            let no = e.internal_function(
                                super::promises::Internal::AsyncResume {
                                    activation: id,
                                    reject: true,
                                },
                                1,
                            )?;
                            let Value::Object(p) = promise else {
                                unreachable!()
                            };
                            e.attach_reaction(p, Some(yes), Some(no), None)?;
                            return Ok(Some(Completion::Normal));
                        }
                        Op::Return(r) => {
                            return Ok(Some(Completion::Return(e.register(&frame, r)?)))
                        }
                        Op::Throw(r) => return Ok(Some(Completion::Throw(e.register(&frame, r)?))),
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
                        Op::Item(r, key, v) => {
                            let Value::Object(object) = e.register(&frame, r)? else {
                                unreachable!()
                            };
                            let value = e.register(&frame, v)?;
                            e.put_data(object, &key, value)?;
                        }
                        Op::Try {
                            catch,
                            finally,
                            end,
                        } => frame.handlers.push(Handler {
                            catch,
                            finally,
                            end,
                            depth: frame.envs.len(),
                            phase: Phase::Try,
                            pending: Completion::Normal,
                        }),
                        Op::Catch(n, body) => {
                            let value = frame.incoming.take().unwrap();
                            e.protect(&value)?;
                            let scope = e.new_environment(Some(env), &body, false)?;
                            e.protect(&Value::Object(scope))?;
                            e.mutable_binding(scope, &n, value)?;
                            e.hoist_functions(&body, scope)?;
                            frame.envs.push(scope);
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
                        Op::Sync(s) => match e.statement(&s, env, true) {
                            Ok(Some(v)) => return Ok(Some(Completion::Return(v))),
                            Err(ScriptError::LoopBreak) => return Ok(Some(Completion::Break)),
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
