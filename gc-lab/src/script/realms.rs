//! Realm Records (ECMA-262 §9.3): several realms in one agent. See REALMS.md.
//!
//! The running realm's intrinsics and global environment live in `Engine::intrinsics` /
//! `Engine::global`; the other realms wait in `Engine::realms`. Objects allocated while a
//! realm other than 0 runs are recorded in `object_realms`, which gives every function its
//! [[Realm]] (OrdinaryFunctionCreate §10.2.3, CreateBuiltinFunction §10.3.4).
use super::{
    builtins::Intrinsics,
    objects::{Descriptor, Kind},
    parser::{self, Stmt},
    vm::{Engine, Object, ScriptError, Value},
};
use crate::dual::{ExternalRoot, Id};
type Result<T> = std::result::Result<T, ScriptError>;

/// A realm that is not running: its global environment root and its intrinsics.
/// The running realm's slot holds `intrinsics: None` (they are in `Engine::intrinsics`).
pub(super) struct RealmSlot {
    pub env: ExternalRoot,
    pub intrinsics: Option<Intrinsics>,
}

impl Engine<'_> {
    /// [[Realm]] of an object: the realm that was running when it was allocated.
    pub(super) fn realm_of(&self, id: Id) -> usize {
        if self.realms.is_empty() {
            return 0;
        }
        self.object_realms.get(&id).map_or(0, |r| *r as usize)
    }
    /// [[Realm]] of a function with its own execution context: built-ins and ECMAScript
    /// functions. Proxies and bound functions push no context (§10.5.12, §10.4.1.1), so a
    /// call through them switches only when it reaches the target.
    pub(super) fn callee_realm(&self, id: Id) -> Option<usize> {
        match self.objects.get(&id) {
            Some(Object::Builtin { .. } | Object::Closure { .. }) => Some(self.realm_of(id)),
            _ => None,
        }
    }
    /// Make realm `k` the running realm (the Realm of a new execution context, §10.2.1.1
    /// PrepareForOrdinaryCall step 4 / §10.3.1 step 6). Returns the previous realm.
    pub(super) fn enter_realm(&mut self, k: usize) -> usize {
        let previous = self.current_realm;
        if k == previous {
            return previous;
        }
        self.realms[previous].intrinsics = self.intrinsics.take();
        self.intrinsics = self.realms[k].intrinsics.take();
        self.global = Some(self.realms[k].env);
        self.current_realm = k;
        previous
    }
    /// Run `f` with `callee`'s realm as the running realm.
    #[inline]
    pub(super) fn in_realm_of<T>(
        &mut self,
        callee: &Value,
        f: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        let target = match (self.realms.is_empty(), callee) {
            (false, Value::Object(id)) => match self.callee_realm(*id) {
                Some(realm) => realm,
                None => return f(self),
            },
            _ => return f(self),
        };
        // `caller_realm` is the realm of the execution context below the callee's: the
        // [[Construct]] checks after the body (§10.2.2 steps 10-12) run there.
        let caller = std::mem::replace(&mut self.caller_realm, self.current_realm);
        let result = if target == self.current_realm {
            f(self)
        } else {
            let previous = self.enter_realm(target);
            let result = f(self);
            self.enter_realm(previous);
            result
        };
        self.caller_realm = caller;
        result
    }
    /// Run `f` with realm `k` as the running realm.
    pub(super) fn in_realm<T>(
        &mut self,
        k: usize,
        f: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        if k == self.current_realm {
            return f(self);
        }
        let previous = self.enter_realm(k);
        let result = f(self);
        self.enter_realm(previous);
        result
    }
    /// Intrinsics of realm `k` (running or not).
    pub(super) fn realm_intrinsics(&self, k: usize) -> &Intrinsics {
        if k == self.current_realm {
            self.intrinsics.as_ref().unwrap()
        } else {
            self.realms[k].intrinsics.as_ref().unwrap()
        }
    }
    /// GetFunctionRealm (§7.3.24): follows bound functions and proxies; a revoked proxy
    /// throws a TypeError. Objects with no [[Realm]] answer the running realm (step 4).
    pub(super) fn function_realm(&mut self, value: &Value) -> Result<usize> {
        let Value::Object(mut id) = value else {
            return Ok(self.current_realm);
        };
        for _ in 0..100_000 {
            match self.objects.get(&id) {
                Some(Object::Proxy { target: None, .. }) => {
                    return self
                        .throw_error("TypeError", "Cannot perform operation on a revoked proxy")
                }
                Some(Object::Proxy {
                    target: Some(t), ..
                }) => id = *t,
                Some(Object::Bound {
                    target: Value::Object(t),
                    ..
                }) => id = *t,
                Some(Object::Builtin { .. } | Object::Closure { .. } | Object::Bound { .. }) => {
                    return Ok(self.realm_of(id))
                }
                _ => return Ok(self.current_realm),
            }
        }
        Err(ScriptError::Runtime("proxy chain limit"))
    }
    /// GetPrototypeFromConstructor (§10.1.14): `constructor.prototype` if it is an object,
    /// otherwise the intrinsic `pick` of GetFunctionRealm(constructor).
    pub(super) fn prototype_from_constructor(
        &mut self,
        constructor: Value,
        pick: impl FnOnce(&Intrinsics) -> Id,
    ) -> Result<Id> {
        let p = self.get_member(constructor.clone(), "prototype")?;
        if let Value::Object(p) = p {
            return Ok(p);
        }
        let realm = self.function_realm(&constructor)?;
        Ok(pick(self.realm_intrinsics(realm)))
    }
    /// test262 `$262.createRealm()` (INTERPRETING.md; InitializeHostDefinedRealm §9.6):
    /// a new realm with fresh intrinsics and global object. Returns its `$262`.
    pub(super) fn create_realm(&mut self) -> Result<Value> {
        let k = self.new_realm()?;
        self.in_realm(k, |e| {
            let global = e.intrinsics.as_ref().unwrap().global;
            e.get_member(Value::Object(global), "$262")
        })
    }
    /// InitializeHostDefinedRealm (§9.6): a new realm with fresh intrinsics and global
    /// object; the running realm is unchanged. Returns the realm's index.
    pub(super) fn new_realm(&mut self) -> Result<usize> {
        if self.realms.is_empty() {
            let env = self
                .global
                .ok_or(ScriptError::Runtime("no global environment"))?;
            self.realms.push(RealmSlot {
                env,
                intrinsics: None,
            });
        }
        if self.realms.len() >= 64 {
            return Err(ScriptError::Runtime("realm limit"));
        }
        let env = self.new_environment(None, &[], true)?;
        let root = self.runtime.root(env)?;
        let k = self.realms.len();
        self.realms.push(RealmSlot {
            env: root,
            intrinsics: None,
        });
        // Enter the new realm with no intrinsics yet; bootstrap installs them. Objects
        // allocated from here on belong to realm k.
        let previous = self.current_realm;
        self.realms[previous].intrinsics = self.intrinsics.take();
        self.current_realm = k;
        self.global = Some(root);
        let result = self.bootstrap(env);
        if self.intrinsics.is_none() {
            // Bootstrap failed before installing intrinsics: drop the half-made realm.
            self.realms.pop();
            let _ = self.runtime.unroot(root);
            self.current_realm = previous;
            self.intrinsics = self.realms[previous].intrinsics.take();
            self.global = Some(self.realms[previous].env);
            return result.map(|_| k);
        }
        self.enter_realm(previous);
        result.map(|_| k)
    }
    /// test262 `$262.evalScript(source)`: ParseScript + ScriptEvaluation (§16.1.6) in the
    /// running realm (the call switched to the `$262`'s realm).
    pub(super) fn eval_script(&mut self, source: Value) -> Result<Value> {
        let Value::String(text) = source else {
            return self.throw_error("TypeError", "evalScript source is not a string");
        };
        let text = text.text()?.to_owned();
        let program = match super::parser::parse(&text) {
            Ok(program) => program,
            // Unimplemented syntax stays Unsupported, never a SyntaxError pass.
            Err(error)
                if error.message.contains("unsupported") || error.message.ends_with("limit") =>
            {
                return Err(ScriptError::Syntax(error))
            }
            Err(error) => return self.throw_error("SyntaxError", error.message),
        };
        let global = self
            .global
            .ok_or(ScriptError::Runtime("no global environment"))?;
        let env = self.runtime.resolve(global)?;
        self.global_declaration_instantiation(&program, env)?;
        // Each Script has its own strictness (§11.2.2), the global environment is shared.
        let strict = !matches!(program.first(), Some(Stmt::Sloppy));
        let saved = self.env_strict(env);
        self.set_env_strict(env, strict);
        // ScriptEvaluation (§16.1.6) returns the Script's completion value, which is what
        // `$262.evalScript` hands back (test262 INTERPRETING.md).
        let completion = self.completion.replace((Value::Undefined, None));
        let result = self.frame(|e| e.block(&program, env, false));
        let value = match std::mem::replace(&mut self.completion, completion) {
            Some((value, root)) => {
                if let Some(root) = root {
                    self.runtime.unroot(root)?;
                }
                value
            }
            None => Value::Undefined,
        };
        self.set_env_strict(env, saved);
        result.map(|_| value)
    }
    /// GlobalDeclarationInstantiation (§16.1.7) of a further Script against an existing
    /// global environment `env`.
    fn global_declaration_instantiation(&mut self, program: &[Stmt], env: Id) -> Result<()> {
        let global = self.intrinsics.as_ref().unwrap().global;
        let mut lexical_names = vec![];
        let mut constant_names = vec![];
        for stmt in parser::direct_statements(program) {
            match stmt {
                Stmt::Bind { kind, pattern, .. } if kind != "var" => {
                    for name in pattern.names() {
                        if kind != "let" {
                            constant_names.push(name.clone());
                        }
                        lexical_names.push(name);
                    }
                }
                Stmt::Const(n, _) => {
                    constant_names.push(n.clone());
                    lexical_names.push(n.clone());
                }
                Stmt::Let(n, _) => lexical_names.push(n.clone()),
                _ => {}
            }
        }
        let mut functions: Vec<String> = vec![];
        for s in program.iter().map(parser::strip_labels) {
            if let Stmt::Function(n, ..)
            | Stmt::AsyncFunction(n, ..)
            | Stmt::GeneratorFunction(n, ..) = s
            {
                functions.retain(|f| f != n);
                functions.push(n.clone());
            }
        }
        let mut vars = vec![];
        parser::var_names(program, &mut vars);
        let (declared_vars, declared_lexical) = match self.objects.get(&env) {
            Some(Object::Environment {
                object_bindings,
                lexical,
                ..
            }) => (object_bindings.clone(), lexical.clone()),
            _ => return Err(ScriptError::Runtime("invalid environment")),
        };
        // Steps 3-4: lexical names may not clash with var, lexical or restricted globals.
        for name in &lexical_names {
            if declared_vars.contains(name) || declared_lexical.contains(name) {
                return self.throw_error(
                    "SyntaxError",
                    &format!("Identifier '{name}' has already been declared"),
                );
            }
            // HasRestrictedGlobalProperty (§9.1.1.4.14).
            if self
                .own(global, name.as_str())?
                .is_some_and(|p| !p.configurable)
            {
                return self.throw_error(
                    "SyntaxError",
                    &format!("Identifier '{name}' has already been declared"),
                );
            }
        }
        // Step 5: var names may not clash with lexical declarations.
        for name in vars.iter().chain(&functions) {
            if declared_lexical.contains(name) {
                return self.throw_error(
                    "SyntaxError",
                    &format!("Identifier '{name}' has already been declared"),
                );
            }
        }
        let extensible = self.data(global)?.extensible;
        // Step 10: CanDeclareGlobalFunction (§9.1.1.4.16).
        for name in &functions {
            let allowed = match self.own(global, name.as_str())? {
                None => extensible,
                Some(p) => {
                    p.configurable
                        || matches!(p.kind, Kind::Data { writable: true, .. }) && p.enumerable
                }
            };
            if !allowed {
                return self.throw_error(
                    "TypeError",
                    &format!("cannot declare global function {name}"),
                );
            }
        }
        // Step 12: CanDeclareGlobalVar (§9.1.1.4.15).
        for name in &vars {
            if !functions.contains(name)
                && !extensible
                && self.own(global, name.as_str())?.is_none()
            {
                return self.throw_error("TypeError", &format!("cannot declare global var {name}"));
            }
        }
        // Step 16: lexical bindings, uninitialized.
        if let Some(Object::Environment {
            bindings,
            mutable,
            constants,
            lexical,
            ..
        }) = self.objects.get_mut(&env)
        {
            for name in &lexical_names {
                bindings.insert(name.clone(), None);
                lexical.insert(name.clone());
                if constant_names.contains(name) {
                    constants.insert(name.clone());
                    mutable.remove(name);
                } else {
                    mutable.insert(name.clone());
                }
            }
        }
        // Step 17: CreateGlobalFunctionBinding (§9.1.1.4.18) with D = false; the value is
        // stored when `block` hoists the function.
        for name in &functions {
            let descriptor = if self
                .own(global, name.as_str())?
                .is_none_or(|p| p.configurable)
            {
                Descriptor {
                    value: Some(Value::Undefined),
                    writable: Some(true),
                    enumerable: Some(true),
                    configurable: Some(false),
                    ..Descriptor::default()
                }
            } else {
                Descriptor {
                    value: Some(Value::Undefined),
                    ..Descriptor::default()
                }
            };
            if !self.define(global, name.as_str(), descriptor)? {
                return self.throw_error("TypeError", "global function definition refused");
            }
        }
        // Step 18: CreateGlobalVarBinding (§9.1.1.4.17) with D = false.
        for name in &vars {
            if !functions.contains(name) && self.own(global, name.as_str())?.is_none() {
                self.define(
                    global,
                    name.as_str(),
                    Descriptor {
                        value: Some(Value::Undefined),
                        writable: Some(true),
                        enumerable: Some(true),
                        configurable: Some(false),
                        ..Descriptor::default()
                    },
                )?;
            }
        }
        if let Some(Object::Environment {
            object_bindings,
            bindings,
            ..
        }) = self.objects.get_mut(&env)
        {
            for name in vars.iter().chain(&functions) {
                object_bindings.insert(name.clone());
                // A var shadows nothing: a stale declarative binding (a built-in) would
                // otherwise answer before the global object.
                if !declared_lexical.contains(name) {
                    bindings.remove(name);
                }
            }
        }
        Ok(())
    }
    /// Drop every realm but the running one's global root (that is `Engine::global`,
    /// which `release_globals` unroots).
    pub(super) fn release_realms(&mut self) {
        if self.realms.is_empty() {
            return;
        }
        self.enter_realm(0);
        for slot in std::mem::take(&mut self.realms).into_iter().skip(1) {
            let _ = self.runtime.unroot(slot.env);
        }
        self.current_realm = 0;
        self.object_realms.clear();
    }
}
