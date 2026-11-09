//! `eval` (ECMA-262 PerformEval and EvalDeclarationInstantiation).
//!
//! Direct eval (`eval(...)` where the name resolves to %eval%) runs in the caller's
//! environment with the caller's strictness; indirect eval runs against the global
//! environment as sloppy code unless its own prologue says "use strict". Sloppy eval
//! declares `var`s and functions in the caller's variable environment (configurable
//! global properties at the top level); strict eval gets its own variable scope.
use super::{
    objects::{Descriptor, Kind},
    parser::{self, Stmt},
    vm::{Engine, FunctionKind, Object, ScriptError, Value},
};
use crate::dual::Id;
type Result<T> = std::result::Result<T, ScriptError>;

impl Engine<'_> {
    pub(super) fn is_intrinsic_eval(&self, value: &Value) -> bool {
        match (value, &self.intrinsics) {
            (Value::Object(id), Some(i)) => i.functions.get("eval") == Some(id),
            _ => false,
        }
    }
    /// Records the value of an expression statement while eval code runs
    /// (statement completion values; a no-op outside eval).
    pub(super) fn set_completion(&mut self, value: Value) -> Result<()> {
        if self.completion.is_none() {
            return Ok(());
        }
        let root = match value.id() {
            Some(id) => Some(self.runtime.root(id)?),
            None => None,
        };
        if let Some((_, Some(old))) = self.completion.replace((value, root)) {
            self.runtime.unroot(old)?;
        }
        Ok(())
    }
    pub(super) fn completion_value(&self) -> Option<Value> {
        self.completion.as_ref().map(|(v, _)| v.clone())
    }
    fn env_binds(&self, mut env: Id, name: &str) -> bool {
        while let Some(Object::Environment {
            bindings, parent, ..
        }) = self.objects.get(&env)
        {
            if bindings.contains_key(name) {
                return true;
            }
            match parent {
                Some(p) => env = *p,
                None => return false,
            }
        }
        false
    }
    /// PerformEval; `caller` is the calling environment for a direct eval.
    pub(super) fn perform_eval(&mut self, source: Value, caller: Option<Id>) -> Result<Value> {
        let Value::String(text) = source else {
            return Ok(source);
        };
        let global = self
            .global
            .ok_or(ScriptError::Runtime("eval without global environment"))?;
        let global = self.runtime.resolve(global)?;
        let env = caller.unwrap_or(global);
        let caller_strict = caller.is_some() && self.env_strict(env);
        let (super_property, super_call) = match caller {
            Some(env) => (self.env_binds(env, "%home"), self.env_binds(env, "%func")),
            None => (false, false),
        };
        let program = match parser::parse_eval(&text, caller_strict, super_property, super_call) {
            Ok(program) => program,
            Err(error) => return self.throw_error("SyntaxError", error.message),
        };
        let strict = !matches!(program.first(), Some(Stmt::Sloppy));
        self.frame(|e| {
            let scope = e.new_environment(Some(env), &program, strict)?;
            e.protect(&Value::Object(scope))?;
            e.set_env_strict(scope, strict);
            let saved = e.completion.replace((Value::Undefined, None));
            let result = (|| {
                if strict {
                    e.hoist_functions(&program, scope)?;
                } else {
                    e.declare_eval(&program, env, scope)?;
                }
                for s in &program {
                    e.tick()?;
                    if e.frame(|e| e.statement(s, scope, false))?.is_some() {
                        break;
                    }
                }
                Ok(())
            })();
            let completion = std::mem::replace(&mut e.completion, saved);
            let value = match completion {
                Some((value, root)) => {
                    if let Some(root) = root {
                        e.runtime.unroot(root)?;
                    }
                    value
                }
                None => Value::Undefined,
            };
            result.map(|()| value)
        })
    }
    /// EvalDeclarationInstantiation for sloppy eval code: functions and `var`s go to
    /// the caller's variable environment; `scope` keeps only the lexical names.
    fn declare_eval(&mut self, program: &[Stmt], env: Id, scope: Id) -> Result<()> {
        let var_env = self.var_environment(env)?;
        let mut vars = vec![];
        parser::var_names(program, &mut vars);
        let mut functions: Vec<&Stmt> = vec![];
        for s in parser::direct_statements(program) {
            if let Some(name) = function_name(s) {
                functions.retain(|f| function_name(f) != Some(name));
                functions.push(s);
            }
        }
        let function_names: Vec<String> = functions
            .iter()
            .filter_map(|f| function_name(f).map(String::from))
            .collect();
        if let Some(Object::Environment {
            bindings, mutable, ..
        }) = self.objects.get_mut(&scope)
        {
            for name in &function_names {
                bindings.remove(name);
                mutable.remove(name);
            }
        }
        // A var may not hoist across a lexical declaration of the same name.
        for name in vars.iter().chain(&function_names) {
            let mut e = env;
            while let Some(Object::Environment {
                parent,
                bindings,
                lexical,
                with_object,
                ..
            }) = self.objects.get(&e)
            {
                let conflict = if e == var_env {
                    lexical.contains(name)
                } else {
                    with_object.is_none() && bindings.contains_key(name)
                };
                if conflict {
                    return self.throw_error(
                        "SyntaxError",
                        &format!("eval var {name} conflicts with a lexical declaration"),
                    );
                }
                match parent {
                    Some(p) if e != var_env => e = *p,
                    _ => break,
                }
            }
        }
        let global_object = match self.objects.get(&var_env) {
            Some(Object::Environment { parent: None, .. }) => {
                Some(self.intrinsics.as_ref().unwrap().global)
            }
            _ => None,
        };
        if let Some(g) = global_object {
            let extensible = self.data(g)?.extensible;
            for name in &function_names {
                let allowed = match self.own(g, name.as_str())? {
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
            for name in &vars {
                if !extensible && self.own(g, name.as_str())?.is_none() {
                    return self
                        .throw_error("TypeError", &format!("cannot declare global var {name}"));
                }
            }
        }
        for f in functions {
            let (Stmt::Function(name, params, body)
            | Stmt::AsyncFunction(name, params, body)
            | Stmt::GeneratorFunction(name, params, body, _)) = f
            else {
                continue;
            };
            let kind = match f {
                Stmt::GeneratorFunction(.., true) => FunctionKind::AsyncGenerator,
                Stmt::GeneratorFunction(..) => FunctionKind::Generator,
                Stmt::AsyncFunction(..) => FunctionKind::Async,
                _ => FunctionKind::Ordinary,
            };
            let function = self.make_function(scope, Some(name), params, body, kind, false)?;
            self.protect(&function)?;
            if let Some(g) = global_object {
                let fresh = self.own(g, name.as_str())?.is_none_or(|p| p.configurable);
                let descriptor = if fresh {
                    Descriptor {
                        value: Some(function),
                        writable: Some(true),
                        enumerable: Some(true),
                        configurable: Some(true),
                        ..Descriptor::default()
                    }
                } else {
                    Descriptor {
                        value: Some(function),
                        ..Descriptor::default()
                    }
                };
                if !self.define(g, name.as_str(), descriptor)? {
                    return self.throw_error("TypeError", "global function definition refused");
                }
                self.add_object_binding(var_env, name);
            } else {
                self.add_var_binding(var_env, name);
                self.initialize(var_env, name, function)?;
            }
        }
        for name in &vars {
            if function_names.contains(name) {
                continue;
            }
            if let Some(g) = global_object {
                if self.own(g, name.as_str())?.is_none() {
                    self.define(
                        g,
                        name.as_str(),
                        Descriptor {
                            value: Some(Value::Undefined),
                            writable: Some(true),
                            enumerable: Some(true),
                            configurable: Some(true),
                            ..Descriptor::default()
                        },
                    )?;
                }
                self.add_object_binding(var_env, name);
            } else {
                self.add_var_binding(var_env, name);
            }
        }
        Ok(())
    }
    fn add_object_binding(&mut self, env: Id, name: &str) {
        if let Some(Object::Environment {
            object_bindings, ..
        }) = self.objects.get_mut(&env)
        {
            object_bindings.insert(name.to_string());
        }
    }
    fn add_var_binding(&mut self, env: Id, name: &str) {
        if let Some(Object::Environment {
            bindings, mutable, ..
        }) = self.objects.get_mut(&env)
        {
            if !bindings.contains_key(name) {
                bindings.insert(name.to_string(), Some(Value::Undefined));
                mutable.insert(name.to_string());
            }
        }
    }
}

fn function_name(s: &Stmt) -> Option<&str> {
    match s {
        Stmt::Function(n, ..) | Stmt::AsyncFunction(n, ..) | Stmt::GeneratorFunction(n, ..) => {
            Some(n)
        }
        _ => None,
    }
}
