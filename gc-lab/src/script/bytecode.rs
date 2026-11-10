//! Stage 3 of BYTECODE_VM.md, coverage step 1: a register bytecode for leaf function
//! bodies (no nested closures that capture the body's own bindings, no `eval`, no
//! `arguments`). Parameters, `var`, `let` and `const` live in registers; names the body
//! does not declare go through the environment chain as before.
//!
//! A subexpression that mentions none of the body's own names is not compiled: it runs
//! on the tree-walker against the function environment (`Insn::Hatch`). That is exact,
//! because without closures over the body's bindings nothing else can observe the
//! registers. Everything else the compiler does not cover makes `compile` fail with a
//! reason, and the call keeps running on the tree-walker (per-function fallback).
//!
//! TDZ is resolved statically (§14.3.1, §9.1.1.1.1 GetBindingValue step 3): without
//! closures, `switch` or `eval`, a reference to a lexical binding executes after its
//! declaration exactly when it is textually after the end of that declaration.
use super::ast_scan::{captured_names, mentions};
use super::js_string::JsString;
use super::parser::{self, Expr, Param, Property, Stmt};
use super::patterns::{Pattern, PatternKey};
use super::vm::Value;
use std::collections::{HashMap, HashSet};

pub(super) type Reg = u32;
/// `this` operand of a call with an undefined receiver.
pub(super) const NO_REG: Reg = u32::MAX;
/// `argc` of a call or `new` whose arguments were spread into the Array in `args`.
pub(super) const SPREAD_ARGC: u32 = u32::MAX;
type CResult<T> = std::result::Result<T, &'static str>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Exp,
    Eq,
    Ne,
    StrictEq,
    StrictNe,
    Lt,
    Gt,
    Le,
    Ge,
    Shl,
    Sar,
    Shr,
    BitAnd,
    BitOr,
    BitXor,
    In,
    InstanceOf,
}
const OPS: &[(Op, &str)] = &[
    (Op::Add, "+"),
    (Op::Sub, "-"),
    (Op::Mul, "*"),
    (Op::Div, "/"),
    (Op::Rem, "%"),
    (Op::Exp, "**"),
    (Op::Eq, "=="),
    (Op::Ne, "!="),
    (Op::StrictEq, "==="),
    (Op::StrictNe, "!=="),
    (Op::Lt, "<"),
    (Op::Gt, ">"),
    (Op::Le, "<="),
    (Op::Ge, ">="),
    (Op::Shl, "<<"),
    (Op::Sar, ">>"),
    (Op::Shr, ">>>"),
    (Op::BitAnd, "&"),
    (Op::BitOr, "|"),
    (Op::BitXor, "^"),
    (Op::In, "in"),
    (Op::InstanceOf, "instanceof"),
];
impl Op {
    fn parse(text: &str) -> Option<Self> {
        OPS.iter().find(|(_, s)| *s == text).map(|(op, _)| *op)
    }
    pub(super) fn as_str(self) -> &'static str {
        OPS.iter().find(|(op, _)| *op == self).unwrap().1
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Logical {
    And,
    Or,
    Coalesce,
}
fn logical(op: &str) -> Option<Logical> {
    match op {
        "&&" => Some(Logical::And),
        "||" => Some(Logical::Or),
        "??" => Some(Logical::Coalesce),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Insn {
    Const {
        dst: Reg,
        k: u32,
    },
    Move {
        dst: Reg,
        src: Reg,
    },
    /// Registers `from..to` become undefined (scope exit, statement temporaries): a dead
    /// register must not keep an object (or a WeakRef target) alive.
    Clear {
        from: Reg,
        to: Reg,
    },
    /// A lexical binding read or written before its declaration ran.
    Tdz,
    /// PutValue on a `const` binding (§9.1.1.1.5 SetMutableBinding step 5).
    ConstAssign,
    GetName {
        dst: Reg,
        name: u32,
    },
    /// ResolveBinding + GetValue for a callee, with the `with` base object as `this`.
    GetCallee {
        dst: Reg,
        this: Reg,
        name: u32,
    },
    TypeofName {
        dst: Reg,
        name: u32,
    },
    /// ResolveBinding (§9.4.2) for a name the body does not declare, encoded in `dst`:
    /// an object is a `with` binding object, true a binding, false unresolvable.
    ResolveName {
        dst: Reg,
        name: u32,
    },
    GetRef {
        dst: Reg,
        reference: Reg,
        name: u32,
    },
    SetRef {
        reference: Reg,
        name: u32,
        src: Reg,
    },
    /// Tree-walker evaluation of `exprs[expr]` (it mentions no register binding).
    Hatch {
        dst: Reg,
        expr: u32,
        name: u32,
        tail: bool,
    },
    /// GetTemplateObject (§13.2.8.4) of the tagged template `exprs[expr]` (cached per
    /// realm and site).
    TemplateObject {
        dst: Reg,
        expr: u32,
    },
    /// One TemplateLiteral step (§13.2.8.6): `dst` (a String) + ToString(`src`) + the
    /// cooked text `names[part]`.
    TemplateConcat {
        dst: Reg,
        src: Reg,
        part: u32,
    },
    /// The nullish check of a property reference (RequireObjectCoercible).
    CheckCoercible {
        obj: Reg,
    },
    GetProp {
        dst: Reg,
        obj: Reg,
        name: u32,
    },
    SetProp {
        obj: Reg,
        name: u32,
        src: Reg,
    },
    /// ToPropertyKey of `key` for `obj`, as a String or Symbol value.
    PropKey {
        dst: Reg,
        obj: Reg,
        key: Reg,
    },
    GetElem {
        dst: Reg,
        obj: Reg,
        key: Reg,
    },
    SetElem {
        obj: Reg,
        key: Reg,
        src: Reg,
    },
    DeleteProp {
        dst: Reg,
        obj: Reg,
        name: u32,
    },
    DeleteElem {
        dst: Reg,
        obj: Reg,
        key: Reg,
    },
    Call {
        dst: Reg,
        callee: Reg,
        this: Reg,
        args: Reg,
        argc: u32,
        tail: bool,
    },
    New {
        dst: Reg,
        callee: Reg,
        args: Reg,
        argc: u32,
    },
    Binary {
        dst: Reg,
        op: Op,
        a: Reg,
        b: Reg,
    },
    Unary {
        dst: Reg,
        op: &'static str,
        src: Reg,
    },
    /// ToNumeric (§7.1.3) of an update expression's old value.
    ToNumeric {
        dst: Reg,
        src: Reg,
    },
    Step {
        dst: Reg,
        src: Reg,
        increment: bool,
    },
    Jump {
        to: u32,
    },
    JumpIfFalse {
        cond: Reg,
        to: u32,
    },
    JumpIfTrue {
        cond: Reg,
        to: u32,
    },
    JumpIfNotNullish {
        cond: Reg,
        to: u32,
    },
    /// A backward jump: one step of the execution budget, and the scratch roots of the
    /// iteration are released.
    Loop {
        to: u32,
    },
    Return {
        src: Reg,
    },
    ReturnUndefined,
    /// The end of a finally block: `kind` 0 jumps to `to`, 1 rethrows `val`, 2 falls
    /// through to the code that returns `val` (or forwards it to an outer finally).
    EndFinally {
        kind: Reg,
        val: Reg,
        to: u32,
    },
    Throw {
        src: Reg,
    },
    NewArray {
        dst: Reg,
    },
    ArrayPush {
        array: Reg,
        src: Reg,
    },
    /// A SpreadElement (§13.2.4.1 ArrayAccumulation, §13.3.8.1 ArgumentListEvaluation):
    /// every value of the iterable `src`, appended to the Array in `array`.
    ArraySpread {
        array: Reg,
        src: Reg,
    },
    ArrayHole {
        array: Reg,
    },
    NewObject {
        dst: Reg,
    },
    DefineField {
        obj: Reg,
        name: u32,
        src: Reg,
    },
    /// InitializeBinding (§9.1.1.1.4) of a captured top-level lexical in the function
    /// environment.
    InitName {
        name: u32,
        src: Reg,
    },
    /// ForIn/OfHeadEvaluation (§14.7.5.6) steps 6-7 on the value in `src`: an
    /// EnumerateObjectProperties iterator (§14.7.5.9) or GetIterator (§7.4.3).
    GetIterator {
        iter: Reg,
        next: Reg,
        src: Reg,
        enumeration: bool,
    },
    /// ForIn/OfBodyEvaluation (§14.7.5.7) steps 6.a-6.d: IteratorNext, jump to `to` when
    /// done, else IteratorValue into `dst`.
    IterNext {
        dst: Reg,
        iter: Reg,
        next: Reg,
        to: u32,
    },
    /// IteratorStep + IteratorValue of a destructuring iterator (§8.6.3, §13.15.5.5):
    /// `dst` is undefined once the record is done. A done result or an abrupt step sets
    /// [[Done]] by clearing `iter`, so neither `IterClose` nor `IterCloseThrow` closes it.
    IterStep {
        dst: Reg,
        iter: Reg,
        next: Reg,
    },
    /// The rest element of an array pattern (§8.6.3 BindingRestElement, §13.15.5.5
    /// AssignmentRestElement): a new Array of the remaining values; `iter` ends done.
    IterRest {
        dst: Reg,
        iter: Reg,
        next: Reg,
    },
    /// A rest property (§8.6.2 RestBindingInitialization, §13.15.5.4): a new ordinary
    /// object and CopyDataProperties (§7.3.25) of `src` into it, excluding the property
    /// keys in registers `keys..keys + count`.
    ObjectRest {
        dst: Reg,
        src: Reg,
        keys: Reg,
        count: u32,
    },
    /// IteratorClose (§7.4.11) with a normal completion (break, return): the register
    /// is cleared first, so a throw from `return` does not close it again.
    IterClose {
        iter: Reg,
    },
    /// IteratorClose with the throw completion in `exc` (unless `iter` was already
    /// closed), then rethrow it.
    IterCloseThrow {
        iter: Reg,
        exc: Reg,
    },
    /// BlockDeclarationInstantiation (§14.2.3) of the environment-resident lexicals of a
    /// block, catch clause or loop head: a new declarative environment, child of the
    /// current one, with the uninitialized bindings of `layouts[layout]`. It becomes the
    /// current environment; `dst` keeps it alive and restorable.
    PushEnv {
        dst: Reg,
        layout: u32,
    },
    /// The current environment becomes the one in `src` (`NO_REG`: the function's).
    SetEnv {
        src: Reg,
    },
    /// CreatePerIterationEnvironment (§14.7.4.4) of the current environment (the `let`
    /// names of `layouts[layout]`), which becomes current and is stored in `dst`.
    CopyEnv {
        dst: Reg,
        layout: u32,
    },
}

pub(super) struct Chunk {
    pub(super) code: Vec<Insn>,
    pub(super) consts: Vec<Value>,
    pub(super) names: Vec<String>,
    pub(super) exprs: Vec<Expr>,
    pub(super) registers: u32,
    /// Parameter bindings copied from the function environment at entry.
    pub(super) params: Vec<(u32, Reg)>,
    /// Exception handlers, innermost first: a throw at `start <= pc < end` stores the
    /// value in `exc`, sets `kind` (a finally's completion kind) to 1, and jumps to `target`.
    pub(super) handlers: Vec<Handler>,
    /// Top-level function declarations, instantiated in the function environment at entry
    /// (§10.2.11 step 36).
    pub(super) hoist: Vec<Stmt>,
    /// Block environment layouts of `PushEnv`/`CopyEnv`: (name index, constant).
    pub(super) layouts: Vec<Vec<(u32, bool)>>,
    /// OBJECT_MODEL.md slice 2: the call needs no environment record of its own, so the
    /// caller skips FunctionDeclarationInstantiation's records (§10.2.11) and the body runs
    /// in the closure's [[Environment]], with the arguments written straight into
    /// `params` (in parameter order). See `elidable`.
    pub(super) elide: bool,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Handler {
    pub(super) start: u32,
    pub(super) end: u32,
    pub(super) target: u32,
    pub(super) exc: Reg,
    pub(super) kind: Option<Reg>,
    /// The register holding the environment current at `target` (`NO_REG`: the
    /// function's): a throw out of a block environment restores it.
    pub(super) env: Reg,
}

/// A try statement with a finally whose try block or catch is being compiled: a `return`
/// stores its value in `val`, sets `kind` to 2 and jumps (via `jumps`) to the finally.
struct Finally {
    kind: Reg,
    val: Reg,
    jumps: Vec<usize>,
    /// `envs.len()` at the try statement: a `return` restores that environment.
    env_depth: usize,
}

#[derive(Clone, Copy)]
struct Binding {
    reg: Reg,
    constant: bool,
    ready: bool,
}
struct Control {
    label: Option<String>,
    is_loop: bool,
    breaks: Vec<usize>,
    continues: Vec<usize>,
    /// Registers at or above this are dead once control leaves the statement.
    mark: Reg,
    /// `finally.len()` when the statement began: a break or continue to it from deeper
    /// would have to run a finally block (not compiled yet).
    finally_depth: usize,
    /// A for-of loop's iterator register: leaving the loop by break or return closes it.
    iterator: Option<Reg>,
    /// A switch statement: the target of an unlabeled `break`, not of `continue`.
    switch: bool,
    /// `envs.len()` at the statement: a break or continue restores that environment.
    env_depth: usize,
}

struct Compiler {
    code: Vec<Insn>,
    consts: Vec<Value>,
    names: Vec<String>,
    name_index: HashMap<String, u32>,
    exprs: Vec<Expr>,
    next: Reg,
    max: Reg,
    function: HashMap<String, Reg>,
    scopes: Vec<HashMap<String, Binding>>,
    controls: Vec<Control>,
    needles: Vec<String>,
    /// Moves whose destination is a discarded statement result; `finish` drops them.
    dead: Vec<usize>,
    handlers: Vec<Handler>,
    finally: Vec<Finally>,
    /// Try blocks (and catches with a finally) around the current code: no tail calls
    /// there (§15.10.2).
    non_tail: u32,
    /// Function-level names a closure may capture: they live in the function environment
    /// record, not in registers (BYTECODE_VM.md, stage 3 coverage step 3).
    captured: HashSet<String>,
    /// Registers holding the block environments around the current code, innermost
    /// last (stage 3 coverage step 11); empty: the function environment is current.
    envs: Vec<Reg>,
    /// Per `enter_scope`: whether it pushed a block environment.
    entered: Vec<bool>,
    layouts: Vec<Vec<(u32, bool)>>,
}

/// The register limit of one function (u16 operands in the design; checked here).
const MAX_REGISTERS: u32 = 4096;

pub(super) fn compile(params: &[Param], body: &[Stmt]) -> CResult<Chunk> {
    compile_body(params, body, false)
}

/// A Script body (§16.1.6 ScriptEvaluation) compiled for the global environment.
/// GlobalDeclarationInstantiation (§16.1.7) runs before the chunk, in `new_environment`
/// and the chunk's `hoist` list (the same `hoist_functions` the tree-walker uses), so
/// every global `var`, function and top-level lexical name lives in the global
/// environment (object record or declarative record) and is reached by name, never
/// through a register. Only lexical names of nested blocks get registers
/// (BYTECODE_VM.md, coverage step 13).
pub(super) fn compile_script(body: &[Stmt]) -> CResult<Chunk> {
    compile_body(&[], body, true)
}

fn compile_body(params: &[Param], body: &[Stmt], script: bool) -> CResult<Chunk> {
    // `%forward` is the derived default constructor's argument forwarding
    // (§15.7.14 ClassDefinitionEvaluation step 14.a: `super(...args)` over the argument
    // list). `arguments_needed` keeps the arguments object for such a body, so it is
    // environment-resident, and the `SuperCall` is hatched: the tree-walker's
    // `super_call` reads the forwarded values from it (BYTECODE_VM.md, coverage step 10).
    // A direct eval (§19.2.1.1 PerformEval) reads and writes any binding by name, and a
    // sloppy one adds `var`s to the function's variable environment
    // (§19.2.1.3 EvalDeclarationInstantiation). Every binding of such a body is then
    // environment-resident and the eval call itself runs through a hatch against the
    // function environment (BYTECODE_VM.md, stage 3 coverage step 9).
    let direct_eval = mentions(&[&params, &body], &["Name(\"eval\")"]);
    let uses_arguments = super::execute::arguments_needed(params, body);
    let mut c = Compiler {
        code: vec![],
        consts: vec![],
        names: vec![],
        name_index: HashMap::new(),
        exprs: vec![],
        next: 0,
        max: 0,
        function: HashMap::new(),
        scopes: vec![],
        controls: vec![],
        needles: vec![],
        dead: vec![],
        handlers: vec![],
        finally: vec![],
        non_tail: 0,
        captured: HashSet::new(),
        envs: vec![],
        entered: vec![],
        layouts: vec![],
    };
    let mut all = vec![];
    // FunctionDeclarationInstantiation (§10.2.11 steps 24-28) runs on the tree-walker
    // before the chunk: defaults, patterns and rest elements are already bound in the
    // parameter environment, so the chunk only copies the bound names.
    let param_names: Vec<String> = params.iter().flat_map(|p| p.binding.names()).collect();
    all.extend(param_names.iter().cloned());
    let mut vars = vec![];
    declared_names(body, &mut vars, &mut all)?;
    all.sort();
    all.dedup();
    // Names a closure mentions, and the top-level function declarations (instantiated in
    // the environment before the body runs), are environment-resident.
    let mut hoist = vec![];
    for statement in body {
        if let Stmt::Function(name, ..)
        | Stmt::AsyncFunction(name, ..)
        | Stmt::GeneratorFunction(name, ..) = parser::strip_labels(statement)
        {
            c.captured.insert(name.clone());
            hoist.push(statement.clone());
        }
    }
    c.captured.extend(captured_names(&body, &all));
    // A closure in a parameter initializer closes over the parameter environment: the
    // body must read and write those parameters there.
    c.captured.extend(captured_names(&params, &all));
    if direct_eval {
        c.captured.extend(all.iter().cloned());
    }
    if script {
        // Global bindings: every VarDeclaredName (at any depth) and every top-level
        // LexicallyDeclaredName (§16.1.7 steps 1-2).
        c.captured.extend(vars.iter().cloned());
        for statement in parser::direct_statements(body) {
            match statement {
                Stmt::Let(name, _) | Stmt::Const(name, _) => {
                    c.captured.insert(name.clone());
                }
                Stmt::Bind { pattern, .. } => c.captured.extend(pattern.names()),
                _ => {}
            }
        }
    }
    if uses_arguments {
        // FunctionDeclarationInstantiation (§10.2.11 steps 15-22) already bound
        // `arguments` in the environment. A sloppy function with simple parameters gets a
        // mapped arguments object (§10.4.4.7) whose elements alias the parameter bindings,
        // so its parameters must live in the environment too.
        c.captured.insert("arguments".to_string());
        if matches!(body.first(), Some(Stmt::Sloppy)) {
            for param in params {
                c.captured.extend(param.binding.names());
            }
        }
    }
    let mut param_regs = vec![];
    for name in &param_names {
        if !c.function.contains_key(name) && !c.captured.contains(name) {
            let reg = c.alloc()?;
            c.function.insert(name.clone(), reg);
            let index = c.name(name);
            param_regs.push((index, reg));
        }
    }
    for name in vars {
        if !c.function.contains_key(&name) && !c.captured.contains(&name) {
            let reg = c.alloc()?;
            c.function.insert(name, reg);
        }
    }
    c.needles = all
        .iter()
        .filter(|n| !c.captured.contains(*n))
        .map(|n| format!("{n:?}"))
        .collect();
    c.block_body(body, true)?;
    c.emit(Insn::ReturnUndefined);
    let elide = !script && elidable(params, body, &param_names, &c, direct_eval, uses_arguments);
    let (code, handlers) = finish(c.code, &c.consts, &param_regs, &c.dead, c.handlers);
    Ok(Chunk {
        code,
        consts: c.consts,
        names: c.names,
        exprs: c.exprs,
        registers: c.max,
        params: param_regs,
        handlers,
        hoist,
        layouts: c.layouts,
        elide,
    })
}

/// Whether a compiled function body can run without its own environment records
/// (OBJECT_MODEL.md slice 2). FunctionDeclarationInstantiation (§10.2.11) creates them,
/// but a record is unobservable when no code resolves a name in it: every parameter and
/// declared name is register-resident, and nothing reads the function's `this`,
/// `new.target`, `super`, `arguments` or HomeObject through the environment chain.
/// Narrow rule (literature check #376): no hatch (it evaluates against the frame's
/// environment), no direct eval, no `arguments`, no `with`, no nested function or class
/// (their [[Environment]] would be the parent and an arrow reads `this` through it), no
/// block environments, no hoisted declarations, and a simple parameter list without
/// duplicates (no separate parameter scope, §10.2.11 step 28; arguments map 1:1 onto
/// `params`).
fn elidable(
    params: &[Param],
    body: &[Stmt],
    param_names: &[String],
    c: &Compiler,
    direct_eval: bool,
    uses_arguments: bool,
) -> bool {
    let simple = params
        .iter()
        .all(|p| !p.rest && p.default.is_none() && matches!(p.binding, parser::Pattern::Name(_)));
    let unique = {
        let mut seen = HashSet::new();
        param_names.iter().all(|n| seen.insert(n))
    };
    simple
        && unique
        && !direct_eval
        && !uses_arguments
        && c.captured.is_empty()
        && c.layouts.is_empty()
        && !c.code.iter().any(|i| matches!(i, Insn::Hatch { .. }))
        && !mentions(
            &[&params, &body],
            &[
                "This",
                "NewTarget",
                "Super",
                "Arrow",
                "Function",
                "Class",
                "With(",
                "ImportMeta",
                "ImportCall",
                "Name(\"arguments\")",
                "Name(\"eval\")",
            ],
        )
}

/// `var` names (hoisted to the function) and every declared name, through nested
/// statements. Unsupported statements are rejected later by `statement`.
fn declared_names(body: &[Stmt], vars: &mut Vec<String>, all: &mut Vec<String>) -> CResult<()> {
    for statement in body {
        declared_in(statement, vars, all)?;
    }
    Ok(())
}
fn declared_in(statement: &Stmt, vars: &mut Vec<String>, all: &mut Vec<String>) -> CResult<()> {
    match statement {
        Stmt::Var(name, _) => {
            vars.push(name.clone());
            all.push(name.clone());
        }
        Stmt::Let(name, _) | Stmt::Const(name, _) => all.push(name.clone()),
        Stmt::Bind { kind, pattern, .. } => {
            for name in pattern.names() {
                if kind == "var" {
                    vars.push(name.clone());
                }
                all.push(name);
            }
        }
        Stmt::Declarations(items) | Stmt::Block(items) => declared_names(items, vars, all)?,
        Stmt::If(_, a, b) => {
            declared_in(a, vars, all)?;
            if let Some(b) = b {
                declared_in(b, vars, all)?;
            }
        }
        Stmt::While { body, .. } | Stmt::Labeled(_, body) => declared_in(body, vars, all)?,
        Stmt::Switch(_, cases) => {
            for (_, body) in cases {
                declared_names(body, vars, all)?;
            }
        }
        Stmt::ForIn {
            kind,
            binding,
            body,
            ..
        }
        | Stmt::ForOf {
            kind,
            binding,
            body,
            ..
        } => {
            if kind != "assignment" {
                for name in binding.names() {
                    if kind == "var" {
                        vars.push(name.clone());
                    }
                    all.push(name);
                }
            }
            declared_in(body, vars, all)?;
        }
        Stmt::For { init, body, .. } => {
            if let Some(init) = init {
                declared_in(init, vars, all)?;
            }
            declared_in(body, vars, all)?;
        }
        Stmt::Try {
            body,
            catch,
            finally,
        } => {
            declared_names(body, vars, all)?;
            if let Some((param, body)) = catch {
                if let Some(param) = param {
                    all.extend(param.names());
                }
                declared_names(body, vars, all)?;
            }
            if let Some(body) = finally {
                declared_names(body, vars, all)?;
            }
        }
        _ => {}
    }
    Ok(())
}

impl Compiler {
    fn alloc(&mut self) -> CResult<Reg> {
        let reg = self.next;
        self.next += 1;
        if self.next > MAX_REGISTERS {
            return Err("vm: register limit");
        }
        self.max = self.max.max(self.next);
        Ok(reg)
    }
    fn emit(&mut self, insn: Insn) -> usize {
        self.code.push(insn);
        self.code.len() - 1
    }
    fn here(&self) -> u32 {
        self.code.len() as u32
    }
    fn patch(&mut self, at: usize, target: u32) {
        match &mut self.code[at] {
            Insn::Jump { to }
            | Insn::JumpIfFalse { to, .. }
            | Insn::JumpIfTrue { to, .. }
            | Insn::JumpIfNotNullish { to, .. }
            | Insn::Loop { to }
            | Insn::IterNext { to, .. }
            | Insn::EndFinally { to, .. } => *to = target,
            _ => unreachable!("patching a non-jump"),
        }
    }
    fn name(&mut self, name: &str) -> u32 {
        if let Some(index) = self.name_index.get(name) {
            return *index;
        }
        let index = self.names.len() as u32;
        self.names.push(name.to_string());
        self.name_index.insert(name.to_string(), index);
        index
    }
    fn constant(&mut self, value: Value, dst: Reg) {
        let k = self.consts.len() as u32;
        self.consts.push(value);
        self.emit(Insn::Const { dst, k });
    }
    fn resolve(&self, name: &str) -> Option<Binding> {
        for scope in self.scopes.iter().rev() {
            if let Some(binding) = scope.get(name) {
                return Some(*binding);
            }
        }
        self.function.get(name).map(|reg| Binding {
            reg: *reg,
            constant: false,
            ready: true,
        })
    }
    fn mark_ready(&mut self, name: &str) {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(binding) = scope.get_mut(name) {
                binding.ready = true;
                return;
            }
        }
    }
    /// Whether `e` mentions a name this body declares (a conservative textual check).
    fn touches_locals(&self, e: &Expr) -> bool {
        let needles: Vec<&str> = self.needles.iter().map(String::as_str).collect();
        !needles.is_empty() && mentions(&[e], &needles)
    }
    /// Whether evaluating any of `later` can assign the local `name`.
    fn writes(later: &[&Expr], name: &str) -> bool {
        if later.is_empty() {
            return false;
        }
        let quoted = format!("Name({name:?})");
        let needles = [
            format!("Assign({quoted}"),
            format!("Update({quoted}"),
            format!("\", {quoted}, "),
        ];
        let needles: Vec<&str> = needles.iter().map(String::as_str).collect();
        later.iter().any(|e| mentions(&[e], &needles))
    }

    // ----- scopes and statements -----

    fn current_env(&self) -> Reg {
        self.envs.last().copied().unwrap_or(NO_REG)
    }
    /// A new block environment for `names` (name, constant), made current.
    fn push_env(&mut self, names: &[(String, bool)]) -> CResult<Reg> {
        let dst = self.alloc()?;
        let layout = self.layouts.len() as u32;
        let entries = names.iter().map(|(n, c)| (self.name(n), *c)).collect();
        self.layouts.push(entries);
        self.emit(Insn::PushEnv { dst, layout });
        self.envs.push(dst);
        Ok(dst)
    }
    /// Leave the innermost block environment.
    fn pop_env(&mut self) {
        self.envs.pop();
        let src = self.current_env();
        self.emit(Insn::SetEnv { src });
    }
    /// Make the environment of depth `depth` current (a jump out of blocks).
    fn restore_env(&mut self, depth: usize) {
        if self.envs.len() > depth {
            let src = if depth == 0 {
                NO_REG
            } else {
                self.envs[depth - 1]
            };
            self.emit(Insn::SetEnv { src });
        }
    }
    fn enter_scope(&mut self, body: &[Stmt], function: bool) -> CResult<Reg> {
        let mark = self.next;
        let mut scope = HashMap::new();
        let mut env_names = vec![];
        for statement in parser::direct_statements(body) {
            let declared: Vec<(String, bool)> = match statement {
                Stmt::Let(name, _) => vec![(name.clone(), false)],
                Stmt::Const(name, _) => vec![(name.clone(), true)],
                Stmt::Bind { kind, pattern, .. } if kind != "var" => {
                    if parser::is_using(kind) {
                        return Err("vm: using declaration");
                    }
                    let constant = kind == "const";
                    pattern.names().into_iter().map(|n| (n, constant)).collect()
                }
                Stmt::Function(..) | Stmt::AsyncFunction(..) | Stmt::GeneratorFunction(..)
                    if function =>
                {
                    continue
                }
                Stmt::Function(..) | Stmt::AsyncFunction(..) | Stmt::GeneratorFunction(..) => {
                    return Err("vm: function declaration")
                }
                _ => continue,
            };
            for (name, constant) in declared {
                if self.captured.contains(&name) {
                    if function {
                        continue;
                    }
                    // Environment-resident: a block environment per evaluation.
                    env_names.push((name, constant));
                    continue;
                }
                let reg = self.alloc()?;
                scope.insert(
                    name,
                    Binding {
                        reg,
                        constant,
                        ready: false,
                    },
                );
            }
        }
        self.scopes.push(scope);
        self.entered.push(!env_names.is_empty());
        if !env_names.is_empty() {
            self.push_env(&env_names)?;
        }
        Ok(mark)
    }
    fn leave_scope(&mut self, mark: Reg) {
        self.scopes.pop();
        if self.entered.pop() == Some(true) {
            self.pop_env();
        }
        if self.next > mark {
            self.emit(Insn::Clear {
                from: mark,
                to: self.next,
            });
        }
        self.next = mark;
    }
    fn block_body(&mut self, body: &[Stmt], function: bool) -> CResult<()> {
        let mark = self.enter_scope(body, function)?;
        for statement in body {
            if function
                && matches!(
                    parser::strip_labels(statement),
                    Stmt::Function(..) | Stmt::AsyncFunction(..) | Stmt::GeneratorFunction(..)
                )
            {
                // Instantiated at entry (`Chunk::hoist`); the statement itself is empty.
                continue;
            }
            self.statement(statement)?;
        }
        if function {
            self.scopes.pop();
            self.next = mark;
        } else {
            self.leave_scope(mark);
        }
        Ok(())
    }
    fn statement(&mut self, statement: &Stmt) -> CResult<()> {
        let mark = self.next;
        let peak = self.max;
        self.max = self.next;
        let result = self.statement_inner(statement);
        if self.max > mark && !matches!(statement, Stmt::Block(_)) {
            // Statement temporaries are dead now (the tree-walker drops its frame roots).
            let top = self.max;
            self.emit(Insn::Clear {
                from: mark,
                to: top,
            });
        }
        self.max = self.max.max(peak);
        self.next = mark;
        result
    }
    fn statement_inner(&mut self, statement: &Stmt) -> CResult<()> {
        match statement {
            Stmt::Empty | Stmt::Sloppy => {}
            Stmt::Declarations(items) => {
                for item in items {
                    self.statement_inner(item)?;
                }
            }
            Stmt::Expr(e) => {
                let dst = self.alloc()?;
                self.discarded(e, dst)?;
            }
            Stmt::Var(name, init) => {
                if let Some(init) = init {
                    self.var_init(name, init)?;
                }
            }
            Stmt::Let(name, init) | Stmt::Const(name, init) => self.declare(name, Some(init))?,
            Stmt::Bind {
                kind,
                pattern: Pattern::Name(name),
                init,
            } => {
                if kind == "var" {
                    if let Some(init) = init {
                        self.var_init(name, init)?;
                    }
                } else if parser::is_using(kind) {
                    return Err("vm: using declaration");
                } else {
                    self.declare(name, init.as_ref())?;
                }
            }
            Stmt::Bind {
                kind,
                pattern,
                init,
            } => {
                if parser::is_using(kind) {
                    return Err("vm: using declaration");
                }
                // §14.3.1.2 LexicalBinding / §14.3.2.1 VariableDeclaration: a pattern
                // always has an initializer (an early error otherwise).
                let init = init.as_ref().ok_or("vm: destructuring declaration")?;
                let value = self.alloc()?;
                self.expr(init, value)?;
                let mode = if kind == "var" {
                    BindMode::Var
                } else {
                    BindMode::Lexical
                };
                self.bind_pattern(pattern, value, mode)?;
            }
            Stmt::Set(object, key, value) => {
                let obj = self.operand(object, &[value])?;
                let src = self.operand(value, &[])?;
                let name = self.name(key);
                self.emit(Insn::SetProp { obj, name, src });
            }
            Stmt::Return(e) => {
                let dst = self.alloc()?;
                self.tail(e, dst)?;
                self.return_value(dst);
            }
            Stmt::Throw(e) => {
                let src = self.operand(e, &[])?;
                self.emit(Insn::Throw { src });
            }
            Stmt::Block(body) => self.block_body(body, false)?,
            Stmt::If(condition, yes, no) => {
                let cond = self.alloc()?;
                self.expr(condition, cond)?;
                let skip = self.emit(Insn::JumpIfFalse { cond, to: 0 });
                self.statement(yes)?;
                if let Some(no) = no {
                    let end = self.emit(Insn::Jump { to: 0 });
                    let here = self.here();
                    self.patch(skip, here);
                    self.statement(no)?;
                    let here = self.here();
                    self.patch(end, here);
                } else {
                    let here = self.here();
                    self.patch(skip, here);
                }
            }
            Stmt::While { test, body, post } => {
                let top = self.here();
                let mut exit = None;
                if !post {
                    exit = Some(self.test(test)?);
                }
                self.loop_body(None, body)?;
                let continue_at = self.here();
                if *post {
                    exit = Some(self.test(test)?);
                }
                self.emit(Insn::Loop { to: top });
                self.close_loop(continue_at, exit);
            }
            Stmt::For {
                init,
                test,
                update,
                body,
            } => {
                let scope = match init.as_deref() {
                    Some(init) => {
                        let mark = self.enter_scope(std::slice::from_ref(init), false)?;
                        self.statement(init)?;
                        Some(mark)
                    }
                    None => None,
                };
                // §14.7.4.2 ForLoopEvaluation: with environment-resident `let` names in
                // the head, CreatePerIterationEnvironment runs after the initializer and
                // after each body, before the update (ForBodyEvaluation steps 2, 3.e).
                let per_iteration = match init.as_deref() {
                    Some(init) if self.entered.last() == Some(&true) && is_let(init) => {
                        let dst = self.current_env();
                        let layout = self.layouts.len() as u32 - 1;
                        self.emit(Insn::CopyEnv { dst, layout });
                        Some((dst, layout))
                    }
                    _ => None,
                };
                let top = self.here();
                let exit = match test {
                    Some(test) => Some(self.test(test)?),
                    None => None,
                };
                self.loop_body(None, body)?;
                let continue_at = self.here();
                if let Some((dst, layout)) = per_iteration {
                    self.emit(Insn::CopyEnv { dst, layout });
                }
                if let Some(update) = update {
                    let mark = self.next;
                    let dst = self.alloc()?;
                    self.discarded(update, dst)?;
                    self.emit(Insn::Clear {
                        from: mark,
                        to: self.next,
                    });
                    self.next = mark;
                }
                self.emit(Insn::Loop { to: top });
                self.close_loop(continue_at, exit);
                if let Some(mark) = scope {
                    self.leave_scope(mark);
                }
            }
            Stmt::Labeled(label, body) => {
                self.controls.push(Control {
                    label: Some(label.clone()),
                    is_loop: false,
                    breaks: vec![],
                    continues: vec![],
                    mark: self.next,
                    finally_depth: self.finally.len(),
                    iterator: None,
                    switch: false,
                    env_depth: self.envs.len(),
                });
                self.statement(body)?;
                let control = self.controls.pop().unwrap();
                let here = self.here();
                for at in control.breaks {
                    self.patch(at, here);
                }
            }
            Stmt::Break(label) => {
                let index = self
                    .controls
                    .iter()
                    .rposition(|c| match label {
                        Some(label) => c.label.as_deref() == Some(label),
                        None => c.is_loop || c.switch,
                    })
                    .ok_or("vm: break target")?;
                if self.controls[index].finally_depth < self.finally.len() {
                    return Err("vm: break through finally");
                }
                self.exit_to(index, false);
            }
            Stmt::Continue => {
                let index = self
                    .controls
                    .iter()
                    .rposition(|c| c.is_loop)
                    .ok_or("vm: continue target")?;
                if self.controls[index].finally_depth < self.finally.len() {
                    return Err("vm: continue through finally");
                }
                self.exit_to(index, true);
            }
            Stmt::Function(..) | Stmt::AsyncFunction(..) | Stmt::GeneratorFunction(..) => {
                return Err("vm: function declaration")
            }
            Stmt::With(..) => return Err("vm: with"),
            Stmt::Switch(value, cases) => self.switch_statement(value, cases)?,
            Stmt::ForIn {
                kind,
                binding,
                iterable,
                body,
            } => self.for_in_of(kind, binding, iterable, body, true)?,
            Stmt::ForOf {
                asynchronous: true, ..
            } => return Err("vm: for-await"),
            Stmt::ForOf {
                kind,
                binding,
                iterable,
                body,
                ..
            } => self.for_in_of(kind, binding, iterable, body, false)?,
            Stmt::Try {
                body,
                catch,
                finally,
            } => self.try_statement(body, catch.as_ref(), finally.as_deref())?,
        }
        Ok(())
    }
    /// ForInOfStatement (§14.7.5.5-§14.7.5.7). Layout: `iterable; GetIterator; top:
    /// IterNext -> exit; bind; body; Loop top; [for-of: handler: IterCloseThrow]; exit:`.
    /// The handler covers the binding and the body, not IteratorNext (§14.7.5.7 step
    /// 6.i: only an abrupt binding or body closes the iterator, and only for-of does).
    fn for_in_of(
        &mut self,
        kind: &str,
        binding: &Pattern,
        iterable: &Expr,
        body: &Stmt,
        enumeration: bool,
    ) -> CResult<()> {
        if !matches!(kind, "var" | "let" | "const" | "assignment") {
            return Err("vm: using declaration");
        }
        let lexical = kind != "var" && kind != "assignment";
        let mark = self.next;
        // A single name or member target; any other head is a destructuring pattern
        // (ForIn/OfBodyEvaluation step 6.h: BindingInitialization or
        // DestructuringAssignmentEvaluation of the value).
        let target = match binding {
            Pattern::Name(name) => Some(Expr::Name(name.clone())),
            Pattern::Target(target)
                if kind == "assignment"
                    && matches!(**target, Expr::Member(..) | Expr::Computed(..)) =>
            {
                Some((**target).clone())
            }
            _ => None,
        };
        let mut env_names = vec![];
        if lexical {
            // §14.7.5.6 step 2: the bound names are in their TDZ while the iterable runs.
            let mut scope = HashMap::new();
            for name in binding.names() {
                if self.captured.contains(&name) {
                    // Environment-resident: the TDZ environment around the iterable
                    // (§14.7.5.6 steps 2-5), then a fresh one per iteration (§14.7.5.7
                    // step 6.g).
                    env_names.push((name, kind == "const"));
                    continue;
                }
                let reg = self.alloc()?;
                scope.insert(
                    name,
                    Binding {
                        reg,
                        constant: kind == "const",
                        ready: false,
                    },
                );
            }
            self.scopes.push(scope);
        }
        let outer_env = self.current_env();
        let env_reg = if env_names.is_empty() {
            None
        } else {
            let reg = self.push_env(&env_names)?;
            Some((reg, self.layouts.len() as u32 - 1))
        };
        let iter = self.alloc()?;
        let next = self.alloc()?;
        let src = self.alloc()?;
        self.expr(iterable, src)?;
        self.emit(Insn::GetIterator {
            iter,
            next,
            src,
            enumeration,
        });
        self.constant(Value::Undefined, src);
        if env_reg.is_some() {
            self.envs.pop();
            self.emit(Insn::SetEnv { src: outer_env });
        }
        let top = self.here();
        let value = self.alloc()?;
        let exit = self.emit(Insn::IterNext {
            dst: value,
            iter,
            next,
            to: 0,
        });
        let start = self.here();
        if let Some((dst, layout)) = env_reg {
            self.emit(Insn::PushEnv { dst, layout });
            self.envs.push(dst);
        }
        // The tree-walker keeps both loops out of tail position (the iterator closes
        // after a `return`), and so does this.
        self.non_tail += 1;
        let result = self.for_in_of_body(
            kind,
            binding,
            target.as_ref(),
            value,
            iter,
            body,
            enumeration,
        );
        self.non_tail -= 1;
        result?;
        let end = self.here();
        let continue_at = end;
        if env_reg.is_some() {
            self.envs.pop();
            self.emit(Insn::SetEnv { src: outer_env });
        }
        self.emit(Insn::Loop { to: top });
        if !enumeration {
            let exc = self.alloc()?;
            let target = self.here();
            self.emit(Insn::IterCloseThrow { iter, exc });
            self.handlers.push(Handler {
                start,
                end,
                target,
                exc,
                kind: None,
                env: outer_env,
            });
        }
        self.close_loop(continue_at, Some(exit));
        if env_reg.is_some() {
            // A `break` leaves from the iteration environment.
            self.emit(Insn::SetEnv { src: outer_env });
        }
        if lexical {
            self.scopes.pop();
        }
        self.emit(Insn::Clear {
            from: mark,
            to: self.next,
        });
        self.next = mark;
        Ok(())
    }
    /// One iteration: bind the value (§14.7.5.7 steps 6.e-6.h), then the body.
    fn for_in_of_body(
        &mut self,
        kind: &str,
        binding: &Pattern,
        target: Option<&Expr>,
        value: Reg,
        iter: Reg,
        body: &Stmt,
        enumeration: bool,
    ) -> CResult<()> {
        let Some(target) = target else {
            let mode = match kind {
                "var" => BindMode::Var,
                "assignment" => BindMode::Assign,
                _ => BindMode::Lexical,
            };
            self.bind_pattern(binding, value, mode)?;
            return self.for_in_of_tail(iter, body, enumeration);
        };
        match target {
            Expr::Name(name)
                if (kind == "let" || kind == "const") && self.resolve(name).is_none() =>
            {
                let name = self.name(name);
                self.emit(Insn::InitName { name, src: value });
            }
            Expr::Name(name) if kind == "let" || kind == "const" => {
                self.mark_ready(name);
                let reg = self.resolve(name).unwrap().reg;
                self.emit(Insn::Move {
                    dst: reg,
                    src: value,
                });
            }
            Expr::Name(name) => self.store_name(name, value)?,
            Expr::Member(base, key) => {
                let obj = self.operand(base, &[])?;
                self.emit(Insn::CheckCoercible { obj });
                let name = self.name(key);
                self.emit(Insn::SetProp {
                    obj,
                    name,
                    src: value,
                });
            }
            Expr::Computed(base, key) => {
                let obj = self.operand(base, &[key])?;
                let raw = self.operand(key, &[])?;
                let key = self.alloc()?;
                self.emit(Insn::PropKey {
                    dst: key,
                    obj,
                    key: raw,
                });
                self.emit(Insn::SetElem {
                    obj,
                    key,
                    src: value,
                });
            }
            _ => unreachable!("checked targets"),
        }
        self.for_in_of_tail(iter, body, enumeration)
    }
    fn for_in_of_tail(&mut self, iter: Reg, body: &Stmt, enumeration: bool) -> CResult<()> {
        self.controls.push(Control {
            label: None,
            is_loop: true,
            breaks: vec![],
            continues: vec![],
            mark: self.next,
            finally_depth: self.finally.len(),
            iterator: (!enumeration).then_some(iter),
            switch: false,
            env_depth: self.envs.len(),
        });
        self.statement(body)
    }
    /// BindingInitialization (§8.6.2) of a declaration pattern (`Lexical`: a let/const
    /// InitializeBinding; `Var`: PutValue on the var binding) or
    /// DestructuringAssignmentEvaluation (§13.15.5.2, `Assign`) of `pattern` with the
    /// value in `src`. A target's reference is evaluated before its value is fetched
    /// (§8.6.3 SingleNameBinding step 1, §13.15.5.5-6 step 1).
    fn bind_pattern(&mut self, pattern: &Pattern, src: Reg, mode: BindMode) -> CResult<()> {
        let leaf = self.pattern_leaf(pattern, mode)?;
        self.bind_value(pattern, leaf, src, mode)
    }
    /// The reference of a single-name or target pattern (through an initializer);
    /// `None` for a nested object or array pattern.
    fn pattern_leaf(&mut self, pattern: &Pattern, mode: BindMode) -> CResult<Option<Leaf>> {
        let target = match pattern {
            Pattern::Default(inner, _) => return self.pattern_leaf(inner, mode),
            Pattern::Object(..) | Pattern::Array(..) => return Ok(None),
            Pattern::Name(name) => Expr::Name(name.clone()),
            Pattern::Target(target) => (**target).clone(),
        };
        Ok(Some(match target {
            Expr::Name(name) if mode == BindMode::Lexical => Leaf::Init(name),
            Expr::Name(name) if self.resolve(&name).is_some() => Leaf::Local(name),
            Expr::Name(name) => {
                let reference = self.alloc()?;
                let index = self.name(&name);
                self.emit(Insn::ResolveName {
                    dst: reference,
                    name: index,
                });
                Leaf::Free(reference, index)
            }
            // The base and key go to fresh registers: a later default or element may
            // reassign a local they mention before the PutValue.
            Expr::Member(base, key) if mode == BindMode::Assign => {
                let obj = self.alloc()?;
                self.expr(&base, obj)?;
                self.emit(Insn::CheckCoercible { obj });
                Leaf::Prop(obj, self.name(&key))
            }
            Expr::Computed(base, key) if mode == BindMode::Assign => {
                let obj = self.alloc()?;
                self.expr(&base, obj)?;
                let raw = self.alloc()?;
                self.expr(&key, raw)?;
                let key = self.alloc()?;
                self.emit(Insn::PropKey {
                    dst: key,
                    obj,
                    key: raw,
                });
                Leaf::Elem(obj, key)
            }
            _ => return Err("vm: assignment target"),
        }))
    }
    fn bind_value(
        &mut self,
        pattern: &Pattern,
        leaf: Option<Leaf>,
        src: Reg,
        mode: BindMode,
    ) -> CResult<()> {
        match pattern {
            Pattern::Name(_) | Pattern::Target(_) => self.put_leaf(leaf.unwrap(), src),
            Pattern::Default(inner, default) => {
                // §8.6.3 SingleNameBinding step 3 / §13.15.5.5 step 3: the initializer
                // runs only for undefined, with NamedEvaluation for an identifier target.
                let value = self.alloc()?;
                self.emit(Insn::Move { dst: value, src });
                let undefined = self.alloc()?;
                self.constant(Value::Undefined, undefined);
                let cond = self.alloc()?;
                self.emit(Insn::Binary {
                    dst: cond,
                    op: Op::StrictEq,
                    a: value,
                    b: undefined,
                });
                let skip = self.emit(Insn::JumpIfFalse { cond, to: 0 });
                match &**inner {
                    Pattern::Name(name) => self.named(default, value, name)?,
                    Pattern::Target(target) if matches!(**target, Expr::Name(_)) => {
                        let Expr::Name(name) = &**target else {
                            unreachable!("name target")
                        };
                        self.named(default, value, name)?
                    }
                    _ => self.expr(default, value)?,
                }
                let here = self.here();
                self.patch(skip, here);
                self.bind_value(inner, leaf, value, mode)
            }
            Pattern::Object(properties, rest) => {
                // §8.6.2 ObjectBindingPattern step 1 / §13.15.5.2 step 1.
                self.emit(Insn::CheckCoercible { obj: src });
                let count = properties.len() as u32;
                let keys = self.next;
                if rest.is_some() {
                    for _ in 0..count {
                        self.alloc()?;
                    }
                }
                for (i, (key, property)) in properties.iter().enumerate() {
                    let value = self.alloc()?;
                    match key {
                        PatternKey::Static(key) => {
                            if rest.is_some() {
                                let s = Value::String(key.clone().into());
                                self.constant(s, keys + i as u32);
                            }
                            let leaf = self.pattern_leaf(property, mode)?;
                            let name = self.name(key);
                            self.emit(Insn::GetProp {
                                dst: value,
                                obj: src,
                                name,
                            });
                            self.bind_value(property, leaf, value, mode)?;
                        }
                        PatternKey::Computed(e) => {
                            let raw = self.alloc()?;
                            self.expr(e, raw)?;
                            let key = if rest.is_some() {
                                keys + i as u32
                            } else {
                                self.alloc()?
                            };
                            self.emit(Insn::PropKey {
                                dst: key,
                                obj: src,
                                key: raw,
                            });
                            let leaf = self.pattern_leaf(property, mode)?;
                            self.emit(Insn::GetElem {
                                dst: value,
                                obj: src,
                                key,
                            });
                            self.bind_value(property, leaf, value, mode)?;
                        }
                    }
                }
                if let Some(rest) = rest {
                    let leaf = self.pattern_leaf(rest, mode)?;
                    let dst = self.alloc()?;
                    self.emit(Insn::ObjectRest {
                        dst,
                        src,
                        keys,
                        count,
                    });
                    self.bind_value(rest, leaf, dst, mode)?;
                }
                Ok(())
            }
            Pattern::Array(items, rest) => {
                // §8.6.2 ArrayBindingPattern / §13.15.5.2 ArrayAssignmentPattern: GetIterator,
                // IteratorBindingInitialization, then IteratorClose unless done.
                let iter = self.alloc()?;
                let next = self.alloc()?;
                self.emit(Insn::GetIterator {
                    iter,
                    next,
                    src,
                    enumeration: false,
                });
                let start = self.here();
                for item in items {
                    let leaf = match item {
                        Some(item) => self.pattern_leaf(item, mode)?,
                        None => None,
                    };
                    let value = self.alloc()?;
                    self.emit(Insn::IterStep {
                        dst: value,
                        iter,
                        next,
                    });
                    if let Some(item) = item {
                        self.bind_value(item, leaf, value, mode)?;
                    }
                }
                if let Some(rest) = rest {
                    let leaf = self.pattern_leaf(rest, mode)?;
                    let dst = self.alloc()?;
                    self.emit(Insn::IterRest { dst, iter, next });
                    self.bind_value(rest, leaf, dst, mode)?;
                }
                let end = self.here();
                let over = self.emit(Insn::Jump { to: 0 });
                // An abrupt completion closes the iterator unless it is done (step 4 / 5).
                let exc = self.alloc()?;
                let target = self.here();
                self.emit(Insn::IterCloseThrow { iter, exc });
                self.handlers.push(Handler {
                    start,
                    end,
                    target,
                    exc,
                    kind: None,
                    env: self.current_env(),
                });
                let here = self.here();
                self.patch(over, here);
                self.emit(Insn::IterClose { iter });
                Ok(())
            }
        }
    }
    fn put_leaf(&mut self, leaf: Leaf, src: Reg) -> CResult<()> {
        match leaf {
            Leaf::Init(name) => match self.resolve(&name) {
                Some(binding) => {
                    self.emit(Insn::Move {
                        dst: binding.reg,
                        src,
                    });
                    self.mark_ready(&name);
                }
                None if self.captured.contains(&name) => {
                    let name = self.name(&name);
                    self.emit(Insn::InitName { name, src });
                }
                None => return Err("vm: undeclared lexical"),
            },
            Leaf::Local(name) => self.store_name(&name, src)?,
            Leaf::Free(reference, name) => {
                self.emit(Insn::SetRef {
                    reference,
                    name,
                    src,
                });
            }
            Leaf::Prop(obj, name) => {
                self.emit(Insn::SetProp { obj, name, src });
            }
            Leaf::Elem(obj, key) => {
                self.emit(Insn::SetElem { obj, key, src });
            }
        }
        Ok(())
    }
    /// PutValue of the register `src` on the binding `name` (a `var` or assignment
    /// target): a register local, or a reference resolved now (§9.4.2).
    fn store_name(&mut self, name: &str, src: Reg) -> CResult<()> {
        match self.resolve(name) {
            Some(binding) if !binding.ready => {
                self.emit(Insn::Tdz);
            }
            Some(binding) if binding.constant => {
                self.emit(Insn::ConstAssign);
            }
            Some(binding) => {
                self.emit(Insn::Move {
                    dst: binding.reg,
                    src,
                });
            }
            None => {
                let reference = self.alloc()?;
                let index = self.name(name);
                self.emit(Insn::ResolveName {
                    dst: reference,
                    name: index,
                });
                self.emit(Insn::SetRef {
                    reference,
                    name: index,
                    src,
                });
            }
        }
        Ok(())
    }
    /// `return` of the value in `src`: through the innermost pending finally, if any.
    fn return_value(&mut self, src: Reg) {
        // §14.7.5.7 step 6.l: the for-of loops left before the next finally block close
        // their iterators, innermost first (the finally's own return path closes the rest).
        let depth = self.finally.len();
        let open: Vec<Reg> = self
            .controls
            .iter()
            .rev()
            .filter(|c| c.finally_depth == depth)
            .filter_map(|c| c.iterator)
            .collect();
        for iter in open {
            self.emit(Insn::IterClose { iter });
        }
        let Some(ctx) = self.finally.last() else {
            self.emit(Insn::Return { src });
            return;
        };
        let (kind, val, depth) = (ctx.kind, ctx.val, ctx.env_depth);
        self.restore_env(depth);
        if val != src {
            self.emit(Insn::Move { dst: val, src });
        }
        self.constant(Value::number(2.0), kind);
        let at = self.emit(Insn::Jump { to: 0 });
        self.finally.last_mut().unwrap().jumps.push(at);
    }
    /// TryStatement (§14.15.3). Layout:
    /// `try-block; Jump normal; catch-block; normal: kind = 0; fin: finally-block;
    /// EndFinally; return-path`. Handlers: try-block -> catch (or fin), catch -> fin.
    fn try_statement(
        &mut self,
        body: &[Stmt],
        catch: Option<&(Option<Pattern>, Vec<Stmt>)>,
        finally: Option<&[Stmt]>,
    ) -> CResult<()> {
        if let Some((param, catch_body)) = catch {
            match param {
                None => {}
                Some(Pattern::Name(name)) => {
                    // Annex B.3.4 (a `var` redeclaring the catch parameter) stays on the
                    // tree-walker.
                    let (mut vars, mut all) = (vec![], vec![]);
                    declared_names(catch_body, &mut vars, &mut all)?;
                    if vars.contains(name) {
                        return Err("vm: var redeclares catch parameter");
                    }
                }
                // A `var` redeclaring a pattern's name is an early error (§14.15.1).
                Some(_) => {}
            }
        }
        let fin = match finally {
            Some(_) => {
                let kind = self.alloc()?;
                let val = self.alloc()?;
                self.finally.push(Finally {
                    kind,
                    val,
                    jumps: vec![],
                    env_depth: self.envs.len(),
                });
                Some((kind, val))
            }
            None => None,
        };
        let first_handler = self.handlers.len();
        let outer_env = self.current_env();
        let start = self.here();
        self.non_tail += 1;
        let result = self.block_body(body, false);
        self.non_tail -= 1;
        result?;
        let end = self.here();
        // Handlers of this statement that go to the finally block (target patched below).
        let mut to_finally = vec![];
        let mut skip = None;
        if let Some((param, catch_body)) = catch {
            let exc = self.alloc()?;
            skip = Some(self.emit(Insn::Jump { to: 0 }));
            let catch_start = self.here();
            self.handlers.push(Handler {
                start,
                end,
                target: catch_start,
                exc,
                kind: None,
                env: outer_env,
            });
            let mut scope = HashMap::new();
            let mut catch_env = false;
            if let Some(Pattern::Name(name)) = param
                .as_ref()
                .filter(|p| matches!(p, Pattern::Name(n) if self.captured.contains(n)))
            {
                // §14.15.2 CatchClauseEvaluation steps 2-4: an environment for the
                // environment-resident parameter.
                self.push_env(&[(name.clone(), false)])?;
                let index = self.name(name);
                self.emit(Insn::InitName {
                    name: index,
                    src: exc,
                });
                catch_env = true;
            } else if let Some(Pattern::Name(name)) = param {
                scope.insert(
                    name.clone(),
                    Binding {
                        reg: exc,
                        constant: false,
                        ready: true,
                    },
                );
            } else if let Some(pattern) = param {
                // §14.15.2 steps 2-5: BindingInitialization of the CatchParameter pattern
                // in the catch environment; its names are in their TDZ until bound.
                let mut env_names = vec![];
                for name in pattern.names() {
                    if self.captured.contains(&name) {
                        env_names.push((name, false));
                        continue;
                    }
                    let reg = self.alloc()?;
                    let binding = Binding {
                        reg,
                        constant: false,
                        ready: false,
                    };
                    scope.insert(name, binding);
                }
                if !env_names.is_empty() {
                    self.push_env(&env_names)?;
                    catch_env = true;
                }
                self.scopes.push(std::mem::take(&mut scope));
                let bound = self.bind_pattern(pattern, exc, BindMode::Lexical);
                scope = self.scopes.pop().unwrap();
                bound?;
            }
            // §15.10.2: a catch is in tail position only without a finally.
            let guard = fin.is_some() as u32;
            self.non_tail += guard;
            self.scopes.push(scope);
            let result = self.block_body(catch_body, false);
            self.scopes.pop();
            self.non_tail -= guard;
            result?;
            if catch_env {
                self.pop_env();
            }
            if let Some((kind, val)) = fin {
                to_finally.push(self.handlers.len());
                self.handlers.push(Handler {
                    start: catch_start,
                    end: self.here(),
                    target: 0,
                    exc: val,
                    kind: Some(kind),
                    env: outer_env,
                });
            }
        } else if let Some((kind, val)) = fin {
            to_finally.push(self.handlers.len());
            self.handlers.push(Handler {
                start,
                end,
                target: 0,
                exc: val,
                kind: Some(kind),
                env: outer_env,
            });
        }
        debug_assert!(self.handlers[first_handler..]
            .iter()
            .all(|h| h.start >= start));
        let Some(finally_body) = finally else {
            let here = self.here();
            if let Some(skip) = skip {
                self.patch(skip, here);
            }
            return Ok(());
        };
        let (kind, val) = fin.unwrap();
        let ctx = self.finally.pop().unwrap();
        let normal = self.here();
        if let Some(skip) = skip {
            self.patch(skip, normal);
        }
        self.constant(Value::number(0.0), kind);
        let fin_start = self.here();
        for index in to_finally {
            self.handlers[index].target = fin_start;
        }
        for at in ctx.jumps {
            self.patch(at, fin_start);
        }
        self.block_body(finally_body, false)?;
        let end_finally = self.emit(Insn::EndFinally { kind, val, to: 0 });
        // kind 2: the pending return.
        self.return_value(val);
        let here = self.here();
        self.patch(end_finally, here);
        Ok(())
    }
    /// Jump out of `controls[index]` (break, or continue of a loop), clearing the
    /// registers of the scopes left on the way.
    /// SwitchStatement evaluation (§14.12.4): the discriminant is evaluated in the outer
    /// scope, the CaseBlock is one lexical scope, and CaseBlockEvaluation compares each
    /// clause selector in source order with IsStrictlyEqual, the default clause last.
    fn switch_statement(
        &mut self,
        value: &Expr,
        cases: &[(Option<Expr>, Vec<Stmt>)],
    ) -> CResult<()> {
        let discriminant = self.alloc()?;
        self.expr(value, discriminant)?;
        let all: Vec<Stmt> = cases
            .iter()
            .flat_map(|(_, body)| body.iter().cloned())
            .collect();
        let mark = self.enter_scope(&all, false)?;
        // Lexical bindings are initialized when their declaration runs; the VM tracks
        // TDZ at compile time, which a jump into a later clause would bypass.
        let lexicals: Vec<String> = self.scopes.last().unwrap().keys().cloned().collect();
        for name in &lexicals {
            let needle = format!("{name:?}");
            let declared = cases
                .iter()
                .position(|(_, body)| mentions(&[&body], &[needle.as_str()]))
                .unwrap_or(cases.len());
            if declared + 1 < cases.len()
                && mentions(&[&&cases[declared + 1..]], &[needle.as_str()])
            {
                return Err("vm: switch lexical across clauses");
            }
        }
        self.controls.push(Control {
            label: None,
            is_loop: false,
            breaks: vec![],
            continues: vec![],
            mark: self.next,
            finally_depth: self.finally.len(),
            iterator: None,
            switch: true,
            env_depth: self.envs.len(),
        });
        let mut entries = vec![];
        for (test, _) in cases {
            let Some(test) = test else {
                entries.push(None);
                continue;
            };
            let scratch = self.next;
            let selector = self.alloc()?;
            self.expr(test, selector)?;
            let cond = self.alloc()?;
            self.emit(Insn::Binary {
                dst: cond,
                op: Op::StrictEq,
                a: discriminant,
                b: selector,
            });
            entries.push(Some(self.emit(Insn::JumpIfTrue { cond, to: 0 })));
            self.emit(Insn::Clear {
                from: scratch,
                to: self.next,
            });
            self.next = scratch;
        }
        let no_match = self.emit(Insn::Jump { to: 0 });
        let mut default = None;
        for (index, (_, body)) in cases.iter().enumerate() {
            let here = self.here();
            match entries[index] {
                Some(at) => self.patch(at, here),
                None => default = Some(here),
            }
            for statement in body {
                self.statement(statement)?;
            }
        }
        let control = self.controls.pop().unwrap();
        let end = self.here();
        self.patch(no_match, default.unwrap_or(end));
        for at in control.breaks {
            self.patch(at, end);
        }
        self.leave_scope(mark);
        Ok(())
    }
    fn exit_to(&mut self, index: usize, is_continue: bool) {
        // The for-of loops left on the way close their iterators (§14.7.5.7 step 6.l),
        // innermost first; a continue keeps its own loop's iterator open.
        let skip = index + is_continue as usize;
        let open: Vec<Reg> = self.controls[skip.min(self.controls.len())..]
            .iter()
            .rev()
            .filter_map(|c| c.iterator)
            .collect();
        for iter in open {
            self.emit(Insn::IterClose { iter });
        }
        self.restore_env(self.controls[index].env_depth);
        let mark = self.controls[index].mark;
        if self.next > mark {
            self.emit(Insn::Clear {
                from: mark,
                to: self.next,
            });
        }
        let at = self.emit(Insn::Jump { to: 0 });
        let control = &mut self.controls[index];
        if is_continue {
            control.continues.push(at);
        } else {
            control.breaks.push(at);
        }
    }
    fn test(&mut self, test: &Expr) -> CResult<usize> {
        let mark = self.next;
        let cond = self.alloc()?;
        self.expr(test, cond)?;
        let at = self.emit(Insn::JumpIfFalse { cond, to: 0 });
        // The condition register is dead after the jump.
        self.emit(Insn::Clear {
            from: mark,
            to: self.next,
        });
        self.next = mark;
        Ok(at)
    }
    fn loop_body(&mut self, label: Option<String>, body: &Stmt) -> CResult<()> {
        self.controls.push(Control {
            label,
            is_loop: true,
            breaks: vec![],
            continues: vec![],
            mark: self.next,
            finally_depth: self.finally.len(),
            iterator: None,
            switch: false,
            env_depth: self.envs.len(),
        });
        self.statement(body)
    }
    fn close_loop(&mut self, continue_at: u32, exit: Option<usize>) {
        let control = self.controls.pop().unwrap();
        let end = self.here();
        for at in control.continues {
            self.patch(at, continue_at);
        }
        for at in control.breaks {
            self.patch(at, end);
        }
        if let Some(exit) = exit {
            self.patch(exit, end);
        }
    }
    /// `var name = init` (§14.3.2.1): PutValue on the resolved binding.
    fn var_init(&mut self, name: &str, init: &Expr) -> CResult<()> {
        if self.resolve(name).is_none() && self.captured.contains(name) {
            let dst = self.alloc()?;
            return self.assign(&Expr::Name(name.to_string()), init, dst);
        }
        self.assign_local(name, init, None)
    }
    fn declare(&mut self, name: &str, init: Option<&Expr>) -> CResult<()> {
        if self.resolve(name).is_none() && self.captured.contains(name) {
            let src = self.alloc()?;
            match init {
                Some(init) => self.named(init, src, name)?,
                None => self.constant(Value::Undefined, src),
            }
            let name = self.name(name);
            self.emit(Insn::InitName { name, src });
            return Ok(());
        }
        let binding = self.resolve(name).ok_or("vm: undeclared lexical")?;
        match init {
            // Reads of `name` inside its own initializer are still in its TDZ.
            Some(init) => self.named(init, binding.reg, name)?,
            None => self.constant(Value::Undefined, binding.reg),
        }
        self.mark_ready(name);
        Ok(())
    }

    // ----- expressions -----

    /// The register holding `e`'s value: a local's own register when nothing in `later`
    /// can reassign it before the value is used, otherwise a fresh temporary.
    fn operand(&mut self, e: &Expr, later: &[&Expr]) -> CResult<Reg> {
        if let Expr::Name(name) = e {
            if let Some(binding) = self.resolve(name) {
                if binding.ready && !Self::writes(later, name) {
                    return Ok(binding.reg);
                }
            }
        }
        let dst = self.alloc()?;
        self.expr(e, dst)?;
        Ok(dst)
    }
    /// Evaluate `e` for its effects only (an expression statement or a `for` update).
    /// A postfix update then has the same effects as a prefix one, and a final copy
    /// into the unused result register is dropped by `finish`.
    fn discarded(&mut self, e: &Expr, dst: Reg) -> CResult<()> {
        match e {
            Expr::Update(target, increment, false) => self.update(target, *increment, true, dst)?,
            _ => self.expr(e, dst)?,
        }
        if let Some(Insn::Move { dst: d, .. }) = self.code.last() {
            if *d == dst {
                self.dead.push(self.code.len() - 1);
            }
        }
        Ok(())
    }
    fn hatch(&mut self, e: &Expr, dst: Reg, name: Option<&str>, tail: bool) {
        let expr = self.exprs.len() as u32;
        self.exprs.push(e.clone());
        let name = match name {
            Some(name) => self.name(name),
            None => u32::MAX,
        };
        self.emit(Insn::Hatch {
            dst,
            expr,
            name,
            tail,
        });
    }
    /// NamedEvaluation (§8.4.5) of an initializer for `name`.
    fn named(&mut self, e: &Expr, dst: Reg, name: &str) -> CResult<()> {
        if !self.touches_locals(e) && e.anonymous_function() {
            self.hatch(e, dst, Some(name), false);
            return Ok(());
        }
        self.expr(e, dst)
    }
    /// Compile `e` natively when the compiler covers it; otherwise roll the partial code
    /// back and report `false`, so the caller hatches it. Only for expressions that touch
    /// no register binding (a hatch evaluates them against the environment). Before
    /// coverage step 13 such expressions were always hatched, which sent nearly all of
    /// top-level script code (where every name is global) back to the tree-walker.
    fn native(&mut self, f: impl FnOnce(&mut Self) -> CResult<()>) -> bool {
        let saved = (
            self.code.len(),
            self.consts.len(),
            self.exprs.len(),
            self.dead.len(),
            self.handlers.len(),
            self.layouts.len(),
            self.next,
            self.max,
            self.non_tail,
        );
        if f(self).is_ok() {
            return true;
        }
        self.code.truncate(saved.0);
        self.consts.truncate(saved.1);
        self.exprs.truncate(saved.2);
        self.dead.truncate(saved.3);
        self.handlers.truncate(saved.4);
        self.layouts.truncate(saved.5);
        self.next = saved.6;
        self.max = saved.7;
        self.non_tail = saved.8;
        false
    }
    /// The `return` expression; calls in tail position (§15.10.2) are marked.
    fn tail(&mut self, e: &Expr, dst: Reg) -> CResult<()> {
        if self.non_tail > 0 {
            if !self.touches_locals(e) && !literal(e) {
                self.hatch(e, dst, None, false);
                return Ok(());
            }
            return self.expr(e, dst);
        }
        if !self.touches_locals(e) && !literal(e) {
            if !self.native(|c| c.tail_native(e, dst)) {
                self.hatch(e, dst, None, true);
            }
            return Ok(());
        }
        self.tail_native(e, dst)
    }
    fn tail_native(&mut self, e: &Expr, dst: Reg) -> CResult<()> {
        match e {
            Expr::Call(f, args) => self.call(f, args, dst, true),
            Expr::Template(t) if t.tag.is_some() => {
                let tag = t.tag.as_deref().unwrap();
                self.call_with(tag, CallArgs::Template(e, t), dst, true)
            }
            Expr::Conditional(condition, yes, no) => {
                let cond = self.alloc()?;
                self.expr(condition, cond)?;
                let skip = self.emit(Insn::JumpIfFalse { cond, to: 0 });
                self.tail(yes, dst)?;
                let end = self.emit(Insn::Jump { to: 0 });
                let here = self.here();
                self.patch(skip, here);
                self.tail(no, dst)?;
                let here = self.here();
                self.patch(end, here);
                Ok(())
            }
            Expr::Binary(op, a, b) if logical(op).is_some() => {
                self.expr(a, dst)?;
                let at = self.short_circuit(logical(op).unwrap(), dst);
                self.tail(b, dst)?;
                let here = self.here();
                self.patch(at, here);
                Ok(())
            }
            Expr::Sequence(items) if !items.is_empty() => {
                let (last, rest) = items.split_last().unwrap();
                for item in rest {
                    let scratch = self.alloc()?;
                    self.expr(item, scratch)?;
                }
                self.tail(last, dst)
            }
            _ => self.expr(e, dst),
        }
    }
    fn short_circuit(&mut self, op: Logical, cond: Reg) -> usize {
        self.emit(match op {
            Logical::And => Insn::JumpIfFalse { cond, to: 0 },
            Logical::Or => Insn::JumpIfTrue { cond, to: 0 },
            Logical::Coalesce => Insn::JumpIfNotNullish { cond, to: 0 },
        })
    }
    fn expr(&mut self, e: &Expr, dst: Reg) -> CResult<()> {
        match e {
            Expr::Number(n) => {
                let value = n.parse::<f64>().map_err(|_| "vm: number literal")?;
                self.constant(Value::number(value), dst);
                return Ok(());
            }
            Expr::String(s) => {
                self.constant(Value::String(s.clone().into()), dst);
                return Ok(());
            }
            Expr::Units(u) => {
                self.constant(Value::String(JsString::from_units(u)), dst);
                return Ok(());
            }
            Expr::Bool(b) => {
                self.constant(Value::Bool(*b), dst);
                return Ok(());
            }
            Expr::Null => {
                self.constant(Value::Null, dst);
                return Ok(());
            }
            Expr::Undefined => {
                self.constant(Value::Undefined, dst);
                return Ok(());
            }
            Expr::Name(name) => {
                match self.resolve(name) {
                    Some(Binding { ready: false, .. }) => {
                        self.emit(Insn::Tdz);
                    }
                    Some(binding) => {
                        if binding.reg != dst {
                            self.emit(Insn::Move {
                                dst,
                                src: binding.reg,
                            });
                        }
                    }
                    None => {
                        let name = self.name(name);
                        self.emit(Insn::GetName { dst, name });
                    }
                }
                return Ok(());
            }
            Expr::This => {
                let name = self.name("this");
                self.emit(Insn::GetName { dst, name });
                return Ok(());
            }
            _ => {}
        }
        if !self.touches_locals(e) {
            if !self.native(|c| c.expr_native(e, dst)) {
                self.hatch(e, dst, None, false);
            }
            return Ok(());
        }
        self.expr_native(e, dst)
    }
    fn expr_native(&mut self, e: &Expr, dst: Reg) -> CResult<()> {
        match e {
            Expr::Member(base, key) => {
                let obj = self.operand(base, &[])?;
                let name = self.name(key);
                self.emit(Insn::GetProp { dst, obj, name });
            }
            Expr::Computed(base, key) => {
                let obj = self.operand(base, &[key])?;
                let raw = self.operand(key, &[])?;
                let key = self.alloc()?;
                self.emit(Insn::PropKey {
                    dst: key,
                    obj,
                    key: raw,
                });
                self.emit(Insn::GetElem { dst, obj, key });
            }
            Expr::Call(f, args) => self.call(f, args, dst, false)?,
            Expr::New(f, args) => {
                let later: Vec<&Expr> = args.iter().collect();
                let callee = self.operand(f, &later)?;
                let (args, argc) = self.arguments(args)?;
                self.emit(Insn::New {
                    dst,
                    callee,
                    args,
                    argc,
                });
            }
            Expr::Unary(op, x) => self.unary(op, x, dst)?,
            Expr::Binary(op, a, b) => {
                if let Some(op) = logical(op) {
                    self.expr(a, dst)?;
                    let at = self.short_circuit(op, dst);
                    self.expr(b, dst)?;
                    let here = self.here();
                    self.patch(at, here);
                } else {
                    let op = Op::parse(op).ok_or("vm: binary operator")?;
                    if matches!(**a, Expr::PrivateName(_)) {
                        return Err("vm: private name");
                    }
                    let a = self.operand(a, &[b])?;
                    let b = self.operand(b, &[])?;
                    self.emit(Insn::Binary { dst, op, a, b });
                }
            }
            Expr::Conditional(condition, yes, no) => {
                let cond = self.alloc()?;
                self.expr(condition, cond)?;
                let skip = self.emit(Insn::JumpIfFalse { cond, to: 0 });
                self.expr(yes, dst)?;
                let end = self.emit(Insn::Jump { to: 0 });
                let here = self.here();
                self.patch(skip, here);
                self.expr(no, dst)?;
                let here = self.here();
                self.patch(end, here);
            }
            Expr::Sequence(items) => {
                let Some((last, rest)) = items.split_last() else {
                    return Err("vm: empty sequence");
                };
                for item in rest {
                    let scratch = self.alloc()?;
                    self.expr(item, scratch)?;
                }
                self.expr(last, dst)?;
            }
            Expr::Assign(target, value) => self.assign(target, value, dst)?,
            Expr::Compound(op, target, value) => self.compound(op, target, value, dst)?,
            Expr::Update(target, increment, prefix) => {
                self.update(target, *increment, *prefix, dst)?
            }
            Expr::Array(elements) => {
                self.emit(Insn::NewArray { dst });
                for element in elements {
                    match element {
                        None => {
                            self.emit(Insn::ArrayHole { array: dst });
                        }
                        Some(Expr::Spread(inner)) => {
                            let src = self.operand(inner, &[])?;
                            self.emit(Insn::ArraySpread { array: dst, src });
                        }
                        Some(element) => {
                            let src = self.operand(element, &[])?;
                            self.emit(Insn::ArrayPush { array: dst, src });
                        }
                    }
                }
            }
            Expr::Object(properties) => {
                if properties.iter().any(|(k, _)| k == "__proto__") {
                    return Err("vm: __proto__ literal");
                }
                // The object is built in a temporary: `dst` may be a binding a property
                // value still reads.
                let obj = self.alloc()?;
                self.emit(Insn::NewObject { dst: obj });
                for (key, property) in properties {
                    let Property::Value(value) = property else {
                        return Err("vm: object literal property kind");
                    };
                    if matches!(
                        value,
                        Expr::Function { method: true, .. }
                            | Expr::GeneratorFunction { method: true, .. }
                            | Expr::AsyncFunction { method: true, .. }
                    ) {
                        return Err("vm: object literal method");
                    }
                    let src = self.alloc()?;
                    self.named(value, src, key)?;
                    let name = self.name(key);
                    self.emit(Insn::DefineField { obj, name, src });
                }
                self.emit(Insn::Move { dst, src: obj });
            }
            Expr::Spread(_) => return Err("vm: spread"),
            Expr::Template(t) => match &t.tag {
                Some(tag) => self.call_with(tag, CallArgs::Template(e, t), dst, false)?,
                None => {
                    // The text is built in a temporary: `dst` may be a binding a later
                    // substitution reads. Each substitution is converted before the next
                    // one is evaluated (§13.2.8.6, TemplateSpans).
                    let cooked = |i: usize| t.parts[i].0.as_deref().ok_or("vm: template escape");
                    let acc = self.alloc()?;
                    self.constant(Value::String(cooked(0)?.to_string().into()), acc);
                    for (i, value) in t.values.iter().enumerate() {
                        let src = self.operand(value, &[])?;
                        let part = self.name(cooked(i + 1)?);
                        self.emit(Insn::TemplateConcat {
                            dst: acc,
                            src,
                            part,
                        });
                    }
                    self.emit(Insn::Move { dst, src: acc });
                }
            },
            Expr::Arrow { .. }
            | Expr::Function { .. }
            | Expr::AsyncFunction { .. }
            | Expr::AsyncArrow { .. }
            | Expr::GeneratorFunction { .. }
            | Expr::Class(_) => {
                // Every local a closure mentions is environment-resident (`captured`), so
                // the tree-walker creates it against the function environment.
                if self.touches_locals(e) {
                    return Err("vm: closure over locals");
                }
                self.hatch(e, dst, None, false);
            }
            Expr::Destructure(pattern, rhs) => {
                // §13.15.2 AssignmentExpression step 2: the value is the right-hand side.
                let value = self.alloc()?;
                self.expr(rhs, value)?;
                self.bind_pattern(pattern, value, BindMode::Assign)?;
                self.emit(Insn::Move { dst, src: value });
            }
            Expr::OptionalChain(_) | Expr::Optional(_) => return Err("vm: optional chain"),
            _ => return Err("vm: expression kind"),
        }
        Ok(())
    }
    fn arguments(&mut self, args: &[Expr]) -> CResult<(Reg, u32)> {
        if args.iter().any(|a| matches!(a, Expr::Spread(_))) {
            // ArgumentListEvaluation (§13.3.8.1) with spreads: the values accumulate in an
            // Array, as on the tree-walker (`spread_arguments`).
            let array = self.alloc()?;
            self.emit(Insn::NewArray { dst: array });
            for arg in args {
                let (e, spread) = match arg {
                    Expr::Spread(inner) => (&**inner, true),
                    e => (e, false),
                };
                let src = self.operand(e, &[])?;
                self.emit(if spread {
                    Insn::ArraySpread { array, src }
                } else {
                    Insn::ArrayPush { array, src }
                });
            }
            return Ok((array, SPREAD_ARGC));
        }
        let base = self.next;
        for _ in args {
            self.alloc()?;
        }
        for (i, arg) in args.iter().enumerate() {
            self.expr(arg, base + i as u32)?;
        }
        Ok((base, args.len() as u32))
    }
    fn call(&mut self, f: &Expr, args: &[Expr], dst: Reg, tail: bool) -> CResult<()> {
        self.call_with(f, CallArgs::Plain(args), dst, tail)
    }
    /// EvaluateCall (§13.3.6.2); a tagged template's arguments are its template object
    /// and its substitutions (§13.3.11.1, §13.2.8.3 ArgumentListEvaluation).
    fn call_with(&mut self, f: &Expr, args: CallArgs, dst: Reg, tail: bool) -> CResult<()> {
        let later: Vec<&Expr> = match args {
            CallArgs::Plain(args) => args.iter().collect(),
            CallArgs::Template(_, t) => t.values.iter().collect(),
        };
        if let (Expr::Name(name), CallArgs::Plain(args)) = (f, &args) {
            if name == "eval" && self.resolve(name).is_none() {
                // §13.3.6.1 steps 6.a-6.b: a call through the name `eval` is a direct eval
                // when the callee is %eval%; the tree-walker decides and runs PerformEval
                // with the function environment as the caller's. When the callee is not
                // %eval%, step 9 is an ordinary EvaluateCall with tailPosition, so the
                // hatch keeps `tail` (eval_tail defers only that case).
                let call = Expr::Call(Box::new(f.clone()), args.to_vec());
                self.hatch(&call, dst, None, tail);
                return Ok(());
            }
        }
        let (callee, this) = match f {
            Expr::Name(name) if self.resolve(name).is_none() => {
                let callee = self.alloc()?;
                let this = self.alloc()?;
                let name = self.name(name);
                self.emit(Insn::GetCallee {
                    dst: callee,
                    this,
                    name,
                });
                (callee, this)
            }
            Expr::Member(base, key) => {
                let this = self.operand(base, &later)?;
                let callee = self.alloc()?;
                let name = self.name(key);
                self.emit(Insn::GetProp {
                    dst: callee,
                    obj: this,
                    name,
                });
                (callee, this)
            }
            Expr::Computed(base, key) => {
                let mut rest = vec![&**key];
                rest.extend(later.iter().copied());
                let this = self.operand(base, &rest)?;
                let raw = self.operand(key, &later)?;
                let key = self.alloc()?;
                self.emit(Insn::PropKey {
                    dst: key,
                    obj: this,
                    key: raw,
                });
                let callee = self.alloc()?;
                self.emit(Insn::GetElem {
                    dst: callee,
                    obj: this,
                    key,
                });
                (callee, this)
            }
            Expr::Optional(_)
            | Expr::OptionalChain(_)
            | Expr::SuperMember(_)
            | Expr::SuperComputed(_) => return Err("vm: callee kind"),
            _ => (self.operand(f, &later)?, NO_REG),
        };
        let (args, argc) = match args {
            CallArgs::Plain(args) => self.arguments(args)?,
            CallArgs::Template(site, t) => {
                let base = self.next;
                for _ in 0..=t.values.len() {
                    self.alloc()?;
                }
                let expr = self.exprs.len() as u32;
                self.exprs.push(site.clone());
                self.emit(Insn::TemplateObject { dst: base, expr });
                for (i, value) in t.values.iter().enumerate() {
                    self.expr(value, base + 1 + i as u32)?;
                }
                (base, t.values.len() as u32 + 1)
            }
        };
        self.emit(Insn::Call {
            dst,
            callee,
            this,
            args,
            argc,
            tail,
        });
        Ok(())
    }
    fn unary(&mut self, op: &str, x: &Expr, dst: Reg) -> CResult<()> {
        match (op, x) {
            ("delete", Expr::Member(base, key)) => {
                let obj = self.operand(base, &[])?;
                let name = self.name(key);
                self.emit(Insn::DeleteProp { dst, obj, name });
            }
            ("delete", Expr::Computed(base, key)) => {
                let obj = self.operand(base, &[key])?;
                let raw = self.operand(key, &[])?;
                let key = self.alloc()?;
                self.emit(Insn::PropKey {
                    dst: key,
                    obj,
                    key: raw,
                });
                self.emit(Insn::DeleteElem { dst, obj, key });
            }
            ("delete", Expr::Name(name)) if self.resolve(name).is_none() => {
                // §13.5.1.2: deleting an environment binding (a sloppy eval's `var`, a
                // global property); the tree-walker's DeleteBinding against `local`.
                let e = Expr::Unary(op.to_string(), Box::new(x.clone()));
                self.hatch(&e, dst, None, false);
            }
            // A register-resident binding is a declarative binding created non-deletable
            // (only eval-created ones are deletable, and such bodies keep none in
            // registers): DeleteBinding returns false (§9.1.1.1.7). Strict code cannot
            // reach this (early error, §13.5.1.1).
            ("delete", Expr::Name(_)) => self.constant(Value::Bool(false), dst),
            ("delete", _) => return Err("vm: delete operand"),
            ("typeof", Expr::Name(name)) if self.resolve(name).is_none() => {
                let name = self.name(name);
                self.emit(Insn::TypeofName { dst, name });
            }
            _ => {
                let op: &'static str = match op {
                    "-" => "-",
                    "+" => "+",
                    "!" => "!",
                    "~" => "~",
                    "void" => "void",
                    "typeof" => "typeof",
                    _ => return Err("vm: unary operator"),
                };
                if matches!(x, Expr::OptionalChain(_)) {
                    return Err("vm: optional chain");
                }
                let src = self.operand(x, &[])?;
                self.emit(Insn::Unary { dst, op, src });
            }
        }
        Ok(())
    }
    /// `name = value` (or `var name = value`) for a register binding; `dst` also
    /// receives the value.
    fn assign_local(&mut self, name: &str, value: &Expr, dst: Option<Reg>) -> CResult<()> {
        let Some(binding) = self.resolve(name) else {
            return Err("vm: unresolved var");
        };
        if !binding.ready || binding.constant {
            let src = self.alloc()?;
            self.named(value, src, name)?;
            self.emit(if binding.ready {
                Insn::ConstAssign
            } else {
                Insn::Tdz
            });
            return Ok(());
        }
        let quoted = format!("{name:?}");
        // Compile straight into the binding unless the value still reads it.
        if mentions(&[value], &[quoted.as_str()]) {
            let src = self.alloc()?;
            self.named(value, src, name)?;
            self.emit(Insn::Move {
                dst: binding.reg,
                src,
            });
        } else {
            self.named(value, binding.reg, name)?;
        }
        if let Some(dst) = dst {
            if dst != binding.reg {
                self.emit(Insn::Move {
                    dst,
                    src: binding.reg,
                });
            }
        }
        Ok(())
    }
    fn assign(&mut self, target: &Expr, value: &Expr, dst: Reg) -> CResult<()> {
        match target {
            Expr::Name(name) if self.resolve(name).is_some() => {
                self.assign_local(name, value, Some(dst))
            }
            Expr::Name(name) => {
                let reference = self.alloc()?;
                let index = self.name(name);
                self.emit(Insn::ResolveName {
                    dst: reference,
                    name: index,
                });
                self.named(value, dst, name)?;
                self.emit(Insn::SetRef {
                    reference,
                    name: index,
                    src: dst,
                });
                Ok(())
            }
            Expr::Member(base, key) => {
                let obj = self.operand(base, &[value])?;
                self.emit(Insn::CheckCoercible { obj });
                let src = self.operand(value, &[])?;
                let name = self.name(key);
                self.emit(Insn::SetProp { obj, name, src });
                if src != dst {
                    self.emit(Insn::Move { dst, src });
                }
                Ok(())
            }
            Expr::Computed(base, key) => {
                let obj = self.operand(base, &[key, value])?;
                let raw = self.operand(key, &[value])?;
                let key = self.alloc()?;
                self.emit(Insn::PropKey {
                    dst: key,
                    obj,
                    key: raw,
                });
                let src = self.operand(value, &[])?;
                self.emit(Insn::SetElem { obj, key, src });
                if src != dst {
                    self.emit(Insn::Move { dst, src });
                }
                Ok(())
            }
            _ => Err("vm: assignment target"),
        }
    }
    /// The reference of an update or compound assignment target, read into `old`.
    fn read_target(&mut self, target: &Expr, old: Reg, later: &[&Expr]) -> CResult<Target> {
        Ok(match target {
            Expr::Name(name) => match self.resolve(name) {
                Some(binding) => {
                    if !binding.ready {
                        self.emit(Insn::Tdz);
                    } else {
                        self.emit(Insn::Move {
                            dst: old,
                            src: binding.reg,
                        });
                    }
                    Target::Local(binding)
                }
                None => {
                    let reference = self.alloc()?;
                    let name = self.name(name);
                    self.emit(Insn::ResolveName {
                        dst: reference,
                        name,
                    });
                    self.emit(Insn::GetRef {
                        dst: old,
                        reference,
                        name,
                    });
                    Target::Free(reference, name)
                }
            },
            Expr::Member(base, key) => {
                let obj = self.operand(base, later)?;
                self.emit(Insn::CheckCoercible { obj });
                let name = self.name(key);
                self.emit(Insn::GetProp {
                    dst: old,
                    obj,
                    name,
                });
                Target::Prop(obj, name)
            }
            Expr::Computed(base, key) => {
                let mut rest = vec![&**key];
                rest.extend(later.iter().copied());
                let obj = self.operand(base, &rest)?;
                let raw = self.operand(key, later)?;
                let key = self.alloc()?;
                self.emit(Insn::PropKey {
                    dst: key,
                    obj,
                    key: raw,
                });
                self.emit(Insn::GetElem { dst: old, obj, key });
                Target::Elem(obj, key)
            }
            _ => return Err("vm: update target"),
        })
    }
    fn write_target(&mut self, target: Target, src: Reg) {
        match target {
            Target::Local(binding) => {
                self.emit(if binding.constant {
                    Insn::ConstAssign
                } else {
                    Insn::Move {
                        dst: binding.reg,
                        src,
                    }
                });
            }
            Target::Free(reference, name) => {
                self.emit(Insn::SetRef {
                    reference,
                    name,
                    src,
                });
            }
            Target::Prop(obj, name) => {
                self.emit(Insn::SetProp { obj, name, src });
            }
            Target::Elem(obj, key) => {
                self.emit(Insn::SetElem { obj, key, src });
            }
        }
    }
    fn compound(&mut self, op: &str, target: &Expr, value: &Expr, dst: Reg) -> CResult<()> {
        let old = self.alloc()?;
        // §13.15.2 steps 4.a/5.a: the logical forms name an anonymous function for an
        // IdentifierReference target.
        let name = match target {
            Expr::Name(name) => Some(name.clone()),
            _ => None,
        };
        let target = self.read_target(target, old, &[value])?;
        if let Some(op) = logical(op) {
            // §13.15.2: the logical forms skip the PutValue when they short-circuit.
            self.emit(Insn::Move { dst, src: old });
            let at = self.short_circuit(op, old);
            let fresh = self.alloc()?;
            match name {
                Some(name) => self.named(value, fresh, &name)?,
                None => self.expr(value, fresh)?,
            }
            self.write_target(target, fresh);
            self.emit(Insn::Move { dst, src: fresh });
            let here = self.here();
            self.patch(at, here);
        } else {
            let op = Op::parse(op).ok_or("vm: compound operator")?;
            let b = self.operand(value, &[])?;
            let result = self.alloc()?;
            self.emit(Insn::Binary {
                dst: result,
                op,
                a: old,
                b,
            });
            self.write_target(target, result);
            self.emit(Insn::Move { dst, src: result });
        }
        Ok(())
    }
    fn update(&mut self, target: &Expr, increment: bool, prefix: bool, dst: Reg) -> CResult<()> {
        if let Expr::Name(name) = target {
            if let Some(binding) = self.resolve(name).filter(|b| b.ready && !b.constant) {
                // §13.4.4.1 / §13.4.2.1 on a register binding: ToNumeric(old), step, store.
                // Step applies ToNumeric itself, so a prefix update needs no separate one.
                let local = binding.reg;
                if prefix {
                    self.emit(Insn::Step {
                        dst: local,
                        src: local,
                        increment,
                    });
                    self.emit(Insn::Move { dst, src: local });
                } else {
                    let numeric = self.alloc()?;
                    self.emit(Insn::ToNumeric {
                        dst: numeric,
                        src: local,
                    });
                    self.emit(Insn::Step {
                        dst: local,
                        src: numeric,
                        increment,
                    });
                    self.emit(Insn::Move { dst, src: numeric });
                }
                return Ok(());
            }
        }
        let old = self.alloc()?;
        let target = self.read_target(target, old, &[])?;
        let numeric = self.alloc()?;
        self.emit(Insn::ToNumeric {
            dst: numeric,
            src: old,
        });
        let next = self.alloc()?;
        self.emit(Insn::Step {
            dst: next,
            src: numeric,
            increment,
        });
        self.write_target(target, next);
        self.emit(Insn::Move {
            dst,
            src: if prefix { next } else { numeric },
        });
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum CallArgs<'a> {
    Plain(&'a [Expr]),
    /// The `Expr::Template` and its record.
    Template(&'a Expr, &'a super::templates::Template),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BindMode {
    Lexical,
    Var,
    Assign,
}

/// The reference of a destructuring target, evaluated before its value.
enum Leaf {
    Init(String),
    Local(String),
    Free(Reg, u32),
    Prop(Reg, u32),
    Elem(Reg, Reg),
}

enum Target {
    Local(Binding),
    Free(Reg, u32),
    Prop(Reg, u32),
    Elem(Reg, Reg),
}

/// Registers that may hold a GC reference (an Object or Native value), over-approximated
/// flow-insensitively: parameters, plus every instruction that can write one.
fn reference_registers(
    code: &[Insn],
    consts: &[Value],
    params: &[(u32, Reg)],
    handlers: &[Handler],
) -> Vec<bool> {
    let size = code
        .iter()
        .filter_map(|insn| match *insn {
            Insn::Clear { to, .. } => Some(to as usize),
            _ => None,
        })
        .chain(params.iter().map(|(_, reg)| *reg as usize + 1))
        .max()
        .unwrap_or(0);
    let mut refs = vec![false; size.max(1)];
    let mut mark = |refs: &mut Vec<bool>, reg: Reg| -> bool {
        let slot = reg as usize;
        if slot >= refs.len() {
            refs.resize(slot + 1, false);
        }
        !std::mem::replace(&mut refs[slot], true)
    };
    for (_, reg) in params {
        mark(&mut refs, *reg);
    }
    // A handler stores the thrown value.
    for handler in handlers {
        mark(&mut refs, handler.exc);
    }
    loop {
        let mut changed = false;
        for insn in code {
            let written = match *insn {
                Insn::Const { dst, k } => consts[k as usize].id().is_some().then_some(dst),
                Insn::Move { dst, src } => refs
                    .get(src as usize)
                    .copied()
                    .unwrap_or(false)
                    .then_some(dst),
                // Operators, ToNumeric, keys, typeof and delete produce primitives.
                Insn::Binary { .. }
                | Insn::Unary { .. }
                | Insn::ToNumeric { .. }
                | Insn::Step { .. }
                | Insn::TemplateConcat { .. }
                | Insn::PropKey { .. }
                | Insn::TypeofName { .. }
                | Insn::DeleteProp { .. }
                | Insn::DeleteElem { .. } => None,
                Insn::GetCallee { dst, this, .. } => {
                    changed |= mark(&mut refs, this);
                    Some(dst)
                }
                Insn::GetName { dst, .. }
                | Insn::ResolveName { dst, .. }
                | Insn::GetRef { dst, .. }
                | Insn::Hatch { dst, .. }
                | Insn::TemplateObject { dst, .. }
                | Insn::GetProp { dst, .. }
                | Insn::GetElem { dst, .. }
                | Insn::Call { dst, .. }
                | Insn::New { dst, .. }
                | Insn::NewArray { dst }
                | Insn::NewObject { dst }
                | Insn::IterNext { dst, .. }
                | Insn::IterStep { dst, .. }
                | Insn::IterRest { dst, .. }
                | Insn::ObjectRest { dst, .. }
                | Insn::PushEnv { dst, .. }
                | Insn::CopyEnv { dst, .. } => Some(dst),
                Insn::GetIterator { iter, next, .. } => {
                    changed |= mark(&mut refs, next);
                    Some(iter)
                }
                Insn::Clear { .. }
                | Insn::Tdz
                | Insn::ConstAssign
                | Insn::SetRef { .. }
                | Insn::CheckCoercible { .. }
                | Insn::SetProp { .. }
                | Insn::SetElem { .. }
                | Insn::Jump { .. }
                | Insn::JumpIfFalse { .. }
                | Insn::JumpIfTrue { .. }
                | Insn::JumpIfNotNullish { .. }
                | Insn::Loop { .. }
                | Insn::Return { .. }
                | Insn::ReturnUndefined
                | Insn::EndFinally { .. }
                | Insn::Throw { .. }
                | Insn::ArrayPush { .. }
                | Insn::ArrayHole { .. }
                | Insn::ArraySpread { .. }
                | Insn::DefineField { .. }
                | Insn::InitName { .. }
                | Insn::IterClose { .. }
                | Insn::IterCloseThrow { .. }
                | Insn::SetEnv { .. } => None,
            };
            if let Some(reg) = written {
                changed |= mark(&mut refs, reg);
            }
        }
        if !changed {
            return refs;
        }
    }
}

/// Drop instructions with no effect: `Clear`s over registers that never hold a GC
/// reference (clearing exists only to end liveness for the collector) and the `dead`
/// result copies. Jump targets are remapped; a target on a dropped instruction moves to
/// the next kept one, which is where executing it would have continued.
fn finish(
    code: Vec<Insn>,
    consts: &[Value],
    params: &[(u32, Reg)],
    dead: &[usize],
    mut handlers: Vec<Handler>,
) -> (Vec<Insn>, Vec<Handler>) {
    let refs = reference_registers(&code, consts, params, &handlers);
    let mut keep = vec![true; code.len()];
    for at in dead {
        keep[*at] = false;
    }
    for (at, insn) in code.iter().enumerate() {
        if let Insn::Clear { from, to } = *insn {
            keep[at] = (from..to).any(|r| refs.get(r as usize).copied().unwrap_or(false));
        }
    }
    let mut index = Vec::with_capacity(code.len() + 1);
    let mut next = 0u32;
    for kept in &keep {
        index.push(next);
        next += *kept as u32;
    }
    index.push(next);
    for handler in &mut handlers {
        handler.start = index[handler.start as usize];
        handler.end = index[handler.end as usize];
        handler.target = index[handler.target as usize];
    }
    let code = code
        .into_iter()
        .zip(keep)
        .filter(|(_, kept)| *kept)
        .map(|(mut insn, _)| {
            match &mut insn {
                Insn::Jump { to }
                | Insn::JumpIfFalse { to, .. }
                | Insn::JumpIfTrue { to, .. }
                | Insn::JumpIfNotNullish { to, .. }
                | Insn::Loop { to }
                | Insn::IterNext { to, .. }
                | Insn::EndFinally { to, .. } => *to = index[*to as usize],
                _ => {}
            }
            insn
        })
        .collect();
    (code, handlers)
}

/// A `let` declaration (the for-head kind with per-iteration bindings, §14.7.4.2 step 2).
fn is_let(init: &Stmt) -> bool {
    match init {
        Stmt::Let(..) => true,
        Stmt::Bind { kind, .. } => kind == "let",
        Stmt::Declarations(items) => items.first().is_some_and(is_let),
        _ => false,
    }
}

/// Literals compile to constants even where the tree-walker could evaluate them.
fn literal(e: &Expr) -> bool {
    matches!(
        e,
        Expr::Number(_)
            | Expr::String(_)
            | Expr::Units(_)
            | Expr::Bool(_)
            | Expr::Null
            | Expr::Undefined
    )
}
