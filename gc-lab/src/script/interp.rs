//! The register interpreter for `bytecode::Chunk` (BYTECODE_VM.md, stage 3). Each
//! instruction reuses the tree-walker's helpers for its semantics (`get_member`,
//! `binary_values`, `invoke_this`, ...), so the two engines differ only in how they reach
//! those helpers: the VM keeps the body's bindings in registers instead of environment
//! records and dispatches a flat instruction array instead of recursing over the AST.
//!
//! Registers live in one `Vec<Value>` shared by all active VM frames (a frame owns the
//! window `base..base + registers`). Their object ids are mirrored in
//! `Runtime::vm_seeds`, which the collector reads as roots.
use super::bytecode::{self, Chunk, Insn, Op, NO_REG};
use super::objects::{Descriptor, Key};
use super::parser::{Expr, Param, Stmt};
use super::references::Reference;
use super::vm::{Engine, ScriptError, Value};
use crate::dual::Id;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

type Result<T> = std::result::Result<T, ScriptError>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EngineMode {
    #[default]
    Tree,
    Vm,
}

/// Compilation counters, for `js_check --vm-stats` and the design note.
#[derive(Clone, Debug, Default)]
pub struct VmStats {
    pub compiled: usize,
    pub calls: u64,
    /// Calls that ran without their own environment records (OBJECT_MODEL.md slice 2).
    pub elided: u64,
    pub fallbacks: HashMap<&'static str, usize>,
    pub max_registers: u32,
    /// Script bodies compiled (BYTECODE_VM.md, coverage step 13), and the reasons the
    /// rest stayed on the tree-walker.
    pub scripts: usize,
    pub script_fallbacks: HashMap<&'static str, usize>,
}

/// Register-file limit across nested VM frames.
const MAX_STACK: usize = 1 << 20;
/// Cache entries before dead bodies are pruned.
const CACHE_PRUNE: usize = 4096;

#[derive(Default)]
pub(super) struct VmState {
    pub(super) mode: EngineMode,
    /// Keyed by the body's `Rc` address; the `Weak` detects a reused address.
    cache: HashMap<usize, (Weak<Vec<Stmt>>, Option<Rc<Chunk>>)>,
    regs: Vec<Value>,
    pub(super) stats: VmStats,
}

impl Engine<'_> {
    /// The chunk for a function body, compiling it on first use. `None` keeps the call
    /// on the tree-walker.
    pub(super) fn vm_chunk(&mut self, params: &[Param], body: &Rc<Vec<Stmt>>) -> Option<Rc<Chunk>> {
        if self.bytecode.mode != EngineMode::Vm {
            return None;
        }
        let key = Rc::as_ptr(body) as usize;
        if let Some((weak, chunk)) = self.bytecode.cache.get(&key) {
            if weak.upgrade().is_some_and(|live| Rc::ptr_eq(&live, body)) {
                return chunk.clone();
            }
        }
        if self.bytecode.cache.len() >= CACHE_PRUNE {
            self.bytecode
                .cache
                .retain(|_, (weak, _)| weak.strong_count() > 0);
        }
        let chunk = match bytecode::compile(params, body) {
            Ok(chunk) => {
                self.bytecode.stats.compiled += 1;
                if std::env::var_os("MWR_VM_DUMP").is_some() {
                    for (pc, insn) in chunk.code.iter().enumerate() {
                        eprintln!("{pc:4} {insn:?}");
                    }
                }
                let max = &mut self.bytecode.stats.max_registers;
                *max = (*max).max(chunk.registers);
                Some(Rc::new(chunk))
            }
            Err(reason) => {
                *self.bytecode.stats.fallbacks.entry(reason).or_default() += 1;
                None
            }
        };
        self.bytecode
            .cache
            .insert(key, (Rc::downgrade(body), chunk.clone()));
        chunk
    }

    /// The chunk for a Script body (§16.1.6), or `None` to run it on the tree-walker.
    /// Scripts run once, so the chunk is not cached.
    pub(super) fn vm_script_chunk(&mut self, body: &[Stmt]) -> Option<Rc<Chunk>> {
        if self.bytecode.mode != EngineMode::Vm {
            return None;
        }
        match bytecode::compile_script(body) {
            Ok(chunk) => {
                self.bytecode.stats.compiled += 1;
                self.bytecode.stats.scripts += 1;
                let max = &mut self.bytecode.stats.max_registers;
                *max = (*max).max(chunk.registers);
                Some(Rc::new(chunk))
            }
            Err(reason) => {
                *self.bytecode.stats.fallbacks.entry(reason).or_default() += 1;
                *self
                    .bytecode
                    .stats
                    .script_fallbacks
                    .entry(reason)
                    .or_default() += 1;
                None
            }
        }
    }

    /// Run a compiled body in its function environment `local` (already instantiated by
    /// FunctionDeclarationInstantiation). Same contract as `block(body, local, true)`.
    pub(super) fn run_chunk(&mut self, chunk: &Chunk, local: Id) -> Result<Option<Value>> {
        let base = self.bytecode.regs.len();
        let top = base + chunk.registers as usize;
        if top > MAX_STACK {
            return Err(ScriptError::Runtime("vm register stack limit"));
        }
        self.bytecode.stats.calls += 1;
        self.bytecode.regs.resize(top, Value::Undefined);
        self.runtime.vm_seeds().resize(top, None);
        let mark = self.roots_mark();
        let result = self.execute_chunk(chunk, local, base);
        self.release_roots(mark);
        self.bytecode.regs.truncate(base);
        self.runtime.vm_seeds().truncate(base);
        result
    }

    /// Run an elidable chunk (`Chunk::elide`, OBJECT_MODEL.md slice 2) in the closure's
    /// [[Environment]] `parent`: no function environment records, and the arguments go
    /// straight into the parameter registers (missing ones stay undefined).
    pub(super) fn run_chunk_elided(
        &mut self,
        chunk: &Chunk,
        parent: Id,
        args: &[Value],
        strict: bool,
    ) -> Result<Option<Value>> {
        let base = self.bytecode.regs.len();
        let top = base + chunk.registers as usize;
        if top > MAX_STACK {
            return Err(ScriptError::Runtime("vm register stack limit"));
        }
        self.bytecode.stats.calls += 1;
        self.bytecode.stats.elided += 1;
        self.bytecode.regs.resize(top, Value::Undefined);
        self.runtime.vm_seeds().resize(top, None);
        let mark = self.roots_mark();
        for ((_, reg), value) in chunk.params.iter().zip(args) {
            self.set_reg(base, *reg, value.clone());
        }
        let result = self
            .protect(&Value::Object(parent))
            .and_then(|_| self.execute_body(chunk, parent, base, strict));
        self.release_roots(mark);
        self.bytecode.regs.truncate(base);
        self.runtime.vm_seeds().truncate(base);
        result
    }

    fn set_reg(&mut self, base: usize, reg: u32, value: Value) {
        let slot = base + reg as usize;
        self.runtime.vm_seeds()[slot] = value.id();
        self.bytecode.regs[slot] = value;
    }
    fn reg(&self, base: usize, reg: u32) -> Value {
        self.bytecode.regs[base + reg as usize].clone()
    }
    fn reference_of<'a>(
        &self,
        encoded: &Value,
        name: &'a str,
        local: Id,
        strict: bool,
    ) -> Reference<'a> {
        match encoded {
            // Environment Records are internal objects distinct from JS objects.
            // This encodes both kinds of rooted Reference in a VM Value::Object
            // without changing the JavaScript value representation.
            Value::Object(object)
                if matches!(
                    self.objects.get(object),
                    Some(super::vm::Object::Environment { .. })
                ) =>
            {
                Reference::Binding(*object, name, strict)
            }
            Value::Object(object) => Reference::WithBinding(*object, name, strict),
            Value::Bool(true) => Reference::Binding(local, name, strict),
            _ => Reference::Unresolvable(name),
        }
    }

    fn execute_chunk(&mut self, chunk: &Chunk, local: Id, base: usize) -> Result<Option<Value>> {
        // §10.2.11 step 36 / `block`'s hoisting: top-level function declarations exist
        // before the body (and before parameters are read: a declaration may shadow one).
        if !chunk.hoist.is_empty() {
            self.hoist_functions(&chunk.hoist, local)?;
        }
        for (name, reg) in &chunk.params {
            let value = self.lookup(local, &chunk.names[*name as usize])?;
            self.set_reg(base, *reg, value);
        }
        let strict = self.env_strict(local);
        self.execute_body(chunk, local, base, strict)
    }
    fn execute_body(
        &mut self,
        chunk: &Chunk,
        local: Id,
        base: usize,
        strict: bool,
    ) -> Result<Option<Value>> {
        let loop_mark = self.roots_mark();
        let mut pc = 0usize;
        // The current environment: `local`, or a block environment held in a register
        // (`PushEnv`; stage 3 coverage step 11).
        let mut env = local;
        loop {
            match self.execute_from(chunk, &mut env, local, base, strict, loop_mark, &mut pc) {
                // §14.15.3: a throw completion inside a try block (or a catch with a finally)
                // goes to its handler; other abrupt completions (limits, Unsupported, host
                // errors) are not catchable, as in the tree-walker.
                Err(ScriptError::Thrown(ex)) => {
                    let at = (pc - 1) as u32;
                    let Some(handler) = chunk.handlers.iter().find(|h| h.start <= at && at < h.end)
                    else {
                        return Err(ScriptError::Thrown(ex));
                    };
                    env = self.env_in(base, handler.env, local);
                    self.set_reg(base, handler.exc, ex.value);
                    if let Some(kind) = handler.kind {
                        self.set_reg(base, kind, Value::number(1.0));
                    }
                    pc = handler.target as usize;
                }
                other => return other,
            }
        }
    }

    /// The dispatch loop from `*pc`; a throw leaves `*pc` one past the faulting instruction.
    /// The argument values of a `Call`/`New`: `argc` registers from `args`, or the
    /// elements of the spread Array in `args` (`SPREAD_ARGC`).
    fn call_arguments(&mut self, base: usize, args: u32, argc: u32) -> Result<Vec<Value>> {
        if argc == bytecode::SPREAD_ARGC {
            let Value::Object(array) = self.reg(base, args) else {
                unreachable!("spread array register")
            };
            return self.argument_values(array);
        }
        let start = base + args as usize;
        Ok(self.bytecode.regs[start..start + argc as usize].to_vec())
    }
    /// The environment in register `reg` (`NO_REG`: the function environment `local`).
    fn env_in(&self, base: usize, reg: u32, local: Id) -> Id {
        if reg == NO_REG {
            return local;
        }
        match self.reg(base, reg) {
            Value::Object(id) => id,
            _ => unreachable!("environment register"),
        }
    }
    fn block_env(&mut self, parent: Id, layout: &[(u32, bool)], chunk: &Chunk) -> Result<Id> {
        let names: Vec<(String, bool)> = layout
            .iter()
            .map(|(name, constant)| (chunk.names[*name as usize].clone(), *constant))
            .collect();
        self.lexical_environment(parent, &names)
    }
    #[allow(clippy::too_many_arguments)]
    fn execute_from(
        &mut self,
        chunk: &Chunk,
        env: &mut Id,
        function_env: Id,
        base: usize,
        strict: bool,
        loop_mark: usize,
        pc: &mut usize,
    ) -> Result<Option<Value>> {
        loop {
            let insn = chunk.code[*pc];
            *pc += 1;
            match insn {
                Insn::Const { dst, k } => {
                    let value = chunk.consts[k as usize].clone();
                    self.set_reg(base, dst, value);
                }
                Insn::Move { dst, src } => {
                    let value = self.reg(base, src);
                    self.set_reg(base, dst, value);
                }
                Insn::Clear { from, to } => {
                    for slot in base + from as usize..base + to as usize {
                        self.bytecode.regs[slot] = Value::Undefined;
                        self.runtime.vm_seeds()[slot] = None;
                    }
                }
                Insn::Tdz => {
                    return self.throw_error("ReferenceError", "binding used before initialization")
                }
                Insn::ConstAssign => {
                    return self.throw_error("TypeError", "assignment to constant")
                }
                Insn::GetName { dst, name } => {
                    let n = name as usize;
                    let (value, _) =
                        self.lookup_hinted(*env, &chunk.names[n], Some(&chunk.hints[n]))?;
                    self.set_reg(base, dst, value);
                }
                Insn::GetCallee { dst, this, name } => {
                    let n = name as usize;
                    let (value, object) =
                        self.lookup_hinted(*env, &chunk.names[n], Some(&chunk.hints[n]))?;
                    self.set_reg(base, dst, value);
                    let receiver = object.map(Value::Object).unwrap_or(Value::Undefined);
                    self.set_reg(base, this, receiver);
                }
                Insn::TypeofName { dst, name } => {
                    let n = name as usize;
                    let found = self.lookup_hinted(*env, &chunk.names[n], Some(&chunk.hints[n]));
                    let value = match found.map(|(v, _)| v) {
                        Err(ScriptError::Thrown(ex))
                            if ex.name.as_deref() == Some("ReferenceError")
                                && ex.message.ends_with(" is not defined") =>
                        {
                            Value::Undefined
                        }
                        r => r?,
                    };
                    self.set_reg(base, dst, value.clone());
                    let value = self.unary_values("typeof", value)?;
                    self.set_reg(base, dst, value);
                }
                Insn::ResolveName { dst, name } => {
                    let encoded = match self.resolve_name_hinted(
                        *env,
                        &chunk.names[name as usize],
                        strict,
                        Some(&chunk.hints[name as usize]),
                    )? {
                        super::vm::Resolved::With(object) => Value::Object(object),
                        super::vm::Resolved::Found(found) => Value::Object(found),
                        super::vm::Resolved::Unresolvable => Value::Bool(false),
                    };
                    self.set_reg(base, dst, encoded);
                }
                Insn::GetRef {
                    dst,
                    reference,
                    name,
                } => {
                    let encoded = self.reg(base, reference);
                    let n = name as usize;
                    let r = self.reference_of(&encoded, &chunk.names[n], *env, strict);
                    let value = match r {
                        Reference::Binding(env, name, _) => {
                            self.lookup_hinted(env, name, Some(&chunk.hints[n]))?.0
                        }
                        r => self.reference_get(&r)?,
                    };
                    self.set_reg(base, dst, value);
                }
                Insn::SetRef {
                    reference,
                    name,
                    src,
                } => {
                    let encoded = self.reg(base, reference);
                    let n = name as usize;
                    let r = self.reference_of(&encoded, &chunk.names[n], *env, strict);
                    let value = self.reg(base, src);
                    match r {
                        Reference::Binding(env, name, _) => {
                            self.protect(&value)?;
                            let hint = Some(&chunk.hints[n]);
                            self.assign_binding_hinted(env, name, value, hint, strict)?;
                        }
                        r => {
                            self.reference_set(r, value)?;
                        }
                    }
                }
                Insn::Hatch {
                    dst,
                    expr,
                    name,
                    tail,
                } => {
                    let e = &chunk.exprs[expr as usize];
                    let value = if name != u32::MAX {
                        self.eval_named(e, *env, &chunk.names[name as usize])?
                    } else if tail && self.tail_ok && self.non_tail == 0 {
                        self.eval_tail(e, *env)?
                    } else {
                        self.eval(e, *env)?
                    };
                    self.set_reg(base, dst, value);
                }
                Insn::CheckCoercible { obj } => {
                    if matches!(
                        self.bytecode.regs[base + obj as usize],
                        Value::Null | Value::Undefined
                    ) {
                        return self.throw_error("TypeError", "nullish property receiver");
                    }
                }
                Insn::GetProp { dst, obj, name } => {
                    let receiver = self.reg(base, obj);
                    let value = self.get_member(receiver, chunk.names[name as usize].as_str())?;
                    self.set_reg(base, dst, value);
                }
                Insn::SetProp { obj, name, src } => {
                    let receiver = self.reg(base, obj);
                    let value = self.reg(base, src);
                    let key = Key::from(chunk.names[name as usize].as_str());
                    self.assign_member(receiver, key, value, strict)?;
                }
                Insn::PropKey { dst, obj, key } => {
                    let receiver = self.reg(base, obj);
                    let raw = self.reg(base, key);
                    // ToPropertyKey of a primitive has no side effects, so the raw value
                    // stays and GetElem/SetElem convert it; objects convert once here.
                    if !matches!(
                        raw,
                        Value::Object(_) | Value::Document | Value::Native { .. }
                    ) {
                        if matches!(receiver, Value::Null | Value::Undefined) {
                            return self.throw_error("TypeError", "nullish property receiver");
                        }
                        self.set_reg(base, dst, raw);
                        continue;
                    }
                    let value = match self.member_key(&receiver, raw)? {
                        Key::String(s) => Value::String(s.into()),
                        Key::Symbol(s) => Value::Symbol(s),
                    };
                    self.set_reg(base, dst, value);
                }
                Insn::GetElem { dst, obj, key } => {
                    let receiver = self.reg(base, obj);
                    let key = self.reg(base, key);
                    if let Some(index) = Self::element_index(&key) {
                        if let Some(value) = self.get_elem_fast(&receiver, index)? {
                            self.set_reg(base, dst, value);
                            continue;
                        }
                    }
                    let key = self.to_key(key)?;
                    let value = self.get_member(receiver, key)?;
                    self.set_reg(base, dst, value);
                }
                Insn::SetElem { obj, key, src } => {
                    let receiver = self.reg(base, obj);
                    let key = self.reg(base, key);
                    let value = self.reg(base, src);
                    if let (Some(index), Value::Object(id)) = (Self::element_index(&key), &receiver)
                    {
                        if self.set_element_fast(*id, index, &value)? {
                            continue;
                        }
                    }
                    let key = self.to_key(key)?;
                    self.assign_member(receiver, key, value, strict)?;
                }
                Insn::DeleteProp { dst, obj, name } => {
                    let key = Key::from(chunk.names[name as usize].as_str());
                    let value = self.delete_value(base, obj, key, strict)?;
                    self.set_reg(base, dst, value);
                }
                Insn::DeleteElem { dst, obj, key } => {
                    let key = self.reg(base, key);
                    let key = self.to_key(key)?;
                    let value = self.delete_value(base, obj, key, strict)?;
                    self.set_reg(base, dst, value);
                }
                Insn::TemplateObject { dst, expr } => {
                    let Expr::Template(t) = &chunk.exprs[expr as usize] else {
                        return Err(ScriptError::Runtime("vm: template site"));
                    };
                    let value = self.template_object(t, *env)?;
                    self.set_reg(base, dst, value);
                }
                Insn::TemplateConcat { dst, src, part } => {
                    let Value::String(text) = self.reg(base, dst) else {
                        return Err(ScriptError::Runtime("vm: template text"));
                    };
                    let value = self.reg(base, src);
                    let text = self.template_text(text, value, &chunk.names[part as usize])?;
                    self.set_reg(base, dst, Value::String(text));
                }
                Insn::Call {
                    dst,
                    callee,
                    this,
                    args,
                    argc,
                    tail,
                } => {
                    let f = self.reg(base, callee);
                    let receiver = if this == NO_REG {
                        Value::Undefined
                    } else {
                        self.reg(base, this)
                    };
                    let values = self.call_arguments(base, args, argc)?;
                    let value = if tail && self.tail_ok && self.non_tail == 0 {
                        self.defer_tail_call(f, receiver, values)?
                    } else {
                        self.invoke_this(f, receiver, values)?
                    };
                    self.set_reg(base, dst, value);
                }
                Insn::New {
                    dst,
                    callee,
                    args,
                    argc,
                } => {
                    let f = self.reg(base, callee);
                    let values = self.call_arguments(base, args, argc)?;
                    let value = self.construct(f, values)?;
                    self.set_reg(base, dst, value);
                }
                Insn::Binary { dst, op, a, b } => {
                    let value = match (
                        &self.bytecode.regs[base + a as usize],
                        &self.bytecode.regs[base + b as usize],
                    ) {
                        (Value::Number(x), Value::Number(y)) if number_op(op) => {
                            numbers(op, f64::from_bits(*x), f64::from_bits(*y))
                        }
                        _ => {
                            let x = self.reg(base, a);
                            let y = self.reg(base, b);
                            self.binary_values(op.as_str(), x, y)?
                        }
                    };
                    self.set_reg(base, dst, value);
                }
                Insn::Unary { dst, op, src } => {
                    let v = self.reg(base, src);
                    let value = match (op, &v) {
                        ("!", _) => Value::Bool(!v.truthy()),
                        ("-", Value::Number(x)) => Value::number(-f64::from_bits(*x)),
                        _ => self.unary_values(op, v)?,
                    };
                    self.set_reg(base, dst, value);
                }
                Insn::ToNumeric { dst, src } => {
                    let v = self.reg(base, src);
                    let value = match v {
                        Value::Number(_) => v,
                        v => match self.to_primitive(v, false)? {
                            big @ Value::BigInt(_) => big,
                            v => Value::number(self.to_number(v)?),
                        },
                    };
                    self.set_reg(base, dst, value);
                }
                Insn::Step {
                    dst,
                    src,
                    increment,
                } => {
                    let value = match &self.bytecode.regs[base + src as usize] {
                        Value::Number(x) => {
                            Value::number(f64::from_bits(*x) + if increment { 1.0 } else { -1.0 })
                        }
                        Value::BigInt(value) => {
                            let one = super::bigint::Big::from_u64(1);
                            let next = if increment {
                                value.add(&one)
                            } else {
                                value.sub(&one)
                            };
                            self.big_value(next)?
                        }
                        other => {
                            // A prefix update steps ToNumeric(old) (§13.4.4.1 step 3).
                            let other = other.clone();
                            match self.to_primitive(other, false)? {
                                Value::BigInt(value) => {
                                    let one = super::bigint::Big::from_u64(1);
                                    let next = if increment {
                                        value.add(&one)
                                    } else {
                                        value.sub(&one)
                                    };
                                    self.big_value(next)?
                                }
                                v => Value::number(
                                    self.to_number(v)? + if increment { 1.0 } else { -1.0 },
                                ),
                            }
                        }
                    };
                    self.set_reg(base, dst, value);
                }
                Insn::Jump { to } => *pc = to as usize,
                Insn::JumpIfFalse { cond, to } => {
                    if !self.bytecode.regs[base + cond as usize].truthy() {
                        *pc = to as usize;
                    }
                }
                Insn::JumpIfTrue { cond, to } => {
                    if self.bytecode.regs[base + cond as usize].truthy() {
                        *pc = to as usize;
                    }
                }
                Insn::JumpIfNotNullish { cond, to } => {
                    if !matches!(
                        self.bytecode.regs[base + cond as usize],
                        Value::Null | Value::Undefined
                    ) {
                        *pc = to as usize;
                    }
                }
                Insn::Loop { to } => {
                    self.tick()?;
                    self.release_roots(loop_mark);
                    *pc = to as usize;
                }
                Insn::Return { src } => return Ok(Some(self.reg(base, src))),
                Insn::ReturnUndefined => return Ok(None),
                // The end of a finally block (§14.15.3 step 4 of TryStatement : try Block
                // Finally): 0 = normal, 1 = rethrow `val`, 2 = the next instruction returns.
                Insn::EndFinally { kind, val, to } => {
                    let kind = self.reg(base, kind);
                    if kind == Value::number(0.0) {
                        *pc = to as usize;
                    } else if kind == Value::number(1.0) {
                        let value = self.reg(base, val);
                        return Err(self.thrown(value));
                    }
                }
                Insn::Throw { src } => {
                    let value = self.reg(base, src);
                    return Err(self.thrown(value));
                }
                Insn::NewArray { dst } => {
                    let id = self.new_array(0)?;
                    self.set_reg(base, dst, Value::Object(id));
                }
                Insn::ArrayPush { array, src } => {
                    let Value::Object(id) = self.reg(base, array) else {
                        unreachable!("array register")
                    };
                    let value = self.reg(base, src);
                    self.append_array(id, Some(value), false)?;
                }
                Insn::ArraySpread { array, src } => {
                    let Value::Object(id) = self.reg(base, array) else {
                        unreachable!("array register")
                    };
                    let value = self.reg(base, src);
                    self.append_array(id, Some(value), true)?;
                }
                Insn::ArrayHole { array } => {
                    let Value::Object(id) = self.reg(base, array) else {
                        unreachable!("array register")
                    };
                    self.append_array(id, None, false)?;
                }
                Insn::NewObject { dst } => {
                    let prototype = self.intrinsics.as_ref().unwrap().object;
                    let id = self.new_object(Some(prototype))?;
                    self.set_reg(base, dst, Value::Object(id));
                }
                Insn::PushEnv { dst, layout } => {
                    let new = self.block_env(*env, &chunk.layouts[layout as usize], chunk)?;
                    self.set_reg(base, dst, Value::Object(new));
                    *env = new;
                }
                Insn::SetEnv { src } => *env = self.env_in(base, src, function_env),
                Insn::CopyEnv { dst, layout } => {
                    let names: Vec<String> = chunk.layouts[layout as usize]
                        .iter()
                        .filter(|(_, constant)| !constant)
                        .map(|(name, _)| chunk.names[*name as usize].clone())
                        .collect();
                    let new = self.clone_iteration(*env, &names)?;
                    self.set_reg(base, dst, Value::Object(new));
                    *env = new;
                }
                Insn::InitName { name, src } => {
                    let value = self.reg(base, src);
                    self.initialize(*env, chunk.names[name as usize].as_str(), value)?;
                }
                Insn::GetIterator {
                    iter,
                    next,
                    src,
                    enumeration,
                } => {
                    let value = self.reg(base, src);
                    let (iterator, method) = if enumeration {
                        let iterator = self.enumeration(value)?;
                        let next = self.intrinsics.as_ref().unwrap().functions
                            ["%ForInIteratorPrototype%.next"];
                        (iterator, Value::Object(next))
                    } else {
                        let method = self.get_member(value.clone(), Key::Symbol(0))?;
                        self.protect(&method)?;
                        self.open_iterator(value, method)?
                    };
                    self.set_reg(base, iter, iterator);
                    self.set_reg(base, next, method);
                }
                Insn::IterNext {
                    dst,
                    iter,
                    next,
                    to,
                } => {
                    let (iterator, method) = (self.reg(base, iter), self.reg(base, next));
                    match self.iterator_step_value(&iterator, &method)? {
                        None => *pc = to as usize,
                        Some(value) => self.set_reg(base, dst, value),
                    }
                }
                Insn::IterStep { dst, iter, next } => {
                    let iterator = self.reg(base, iter);
                    let mut done = iterator == Value::Undefined;
                    let method = self.reg(base, next);
                    let outcome = self.binding_step(iterator, method, &mut done);
                    if done {
                        self.set_reg(base, iter, Value::Undefined);
                    }
                    let value = outcome?;
                    self.set_reg(base, dst, value);
                }
                Insn::IterRest { dst, iter, next } => {
                    let array = self.new_array(0)?;
                    self.set_reg(base, dst, Value::Object(array));
                    let iterator = self.reg(base, iter);
                    let method = self.reg(base, next);
                    let mut done = iterator == Value::Undefined;
                    while !done {
                        self.tick()?;
                        let outcome =
                            self.binding_step(iterator.clone(), method.clone(), &mut done);
                        if done {
                            self.set_reg(base, iter, Value::Undefined);
                        }
                        let value = outcome?;
                        if !done {
                            self.append_array(array, Some(value), false)?;
                        }
                    }
                }
                Insn::ObjectRest {
                    dst,
                    src,
                    keys,
                    count,
                } => {
                    let id = self.new_object(Some(self.intrinsics.as_ref().unwrap().object))?;
                    self.set_reg(base, dst, Value::Object(id));
                    let mut excluded = Vec::with_capacity(count as usize);
                    for i in 0..count {
                        let key = self.reg(base, keys + i);
                        excluded.push(self.to_key(key)?);
                    }
                    let source = self.reg(base, src);
                    self.copy_properties(id, source, &excluded)?;
                }
                Insn::IterClose { iter } => {
                    let iterator = self.reg(base, iter);
                    if iterator == Value::Undefined {
                        continue;
                    }
                    self.protect(&iterator)?;
                    self.set_reg(base, iter, Value::Undefined);
                    self.close_iterator(iterator, Ok(None))?;
                }
                Insn::IterCloseThrow { iter, exc } => {
                    let iterator = self.reg(base, iter);
                    let value = self.reg(base, exc);
                    if iterator == Value::Undefined {
                        return Err(self.thrown(value));
                    }
                    self.protect(&iterator)?;
                    self.protect(&value)?;
                    self.set_reg(base, iter, Value::Undefined);
                    let pending = Err(self.thrown(value.clone()));
                    // A throw completion stays abrupt: IteratorClose returns it (§7.4.11 step 5).
                    return match self.close_iterator(iterator, pending) {
                        Err(error) => Err(error),
                        Ok(_) => Err(self.thrown(value)),
                    };
                }
                Insn::DefineField { obj, name, src } => {
                    let Value::Object(id) = self.reg(base, obj) else {
                        unreachable!("object register")
                    };
                    let value = self.reg(base, src);
                    self.define(
                        id,
                        chunk.names[name as usize].as_str(),
                        Descriptor {
                            value: Some(value),
                            writable: Some(true),
                            enumerable: Some(true),
                            configurable: Some(true),
                            ..Descriptor::default()
                        },
                    )?;
                }
            }
        }
    }

    /// `delete` of a property reference, as `eval_unary` does it.
    fn delete_value(&mut self, base: usize, obj: u32, key: Key, strict: bool) -> Result<Value> {
        if let Value::Object(id) = self.reg(base, obj) {
            if !self.delete(id, &key)? {
                if !strict {
                    return Ok(Value::Bool(false));
                }
                return self.throw_error("TypeError", "cannot delete property");
            }
        }
        Ok(Value::Bool(true))
    }

    pub(super) fn engine_mode(&self) -> EngineMode {
        self.bytecode.mode
    }
    pub fn set_engine_mode(&mut self, mode: EngineMode) {
        self.bytecode.mode = mode;
    }
    pub fn vm_stats(&self) -> VmStats {
        self.bytecode.stats.clone()
    }
}

/// The operators whose Number-Number case `numbers` computes (§6.1.6.1).
fn number_op(op: Op) -> bool {
    matches!(
        op,
        Op::Add
            | Op::Sub
            | Op::Mul
            | Op::Div
            | Op::Rem
            | Op::Lt
            | Op::Gt
            | Op::Le
            | Op::Ge
            | Op::StrictEq
            | Op::StrictNe
            | Op::Eq
            | Op::Ne
    )
}
fn numbers(op: Op, a: f64, b: f64) -> Value {
    match op {
        Op::Add => Value::number(a + b),
        Op::Sub => Value::number(a - b),
        Op::Mul => Value::number(a * b),
        Op::Div => Value::number(a / b),
        Op::Rem => Value::number(a % b),
        // Number::lessThan: NaN compares false both ways.
        Op::Lt => Value::Bool(a < b),
        Op::Gt => Value::Bool(a > b),
        Op::Le => Value::Bool(a <= b),
        Op::Ge => Value::Bool(a >= b),
        Op::StrictEq | Op::Eq => Value::Bool(a == b),
        Op::StrictNe | Op::Ne => Value::Bool(a != b),
        _ => unreachable!("number_op"),
    }
}
