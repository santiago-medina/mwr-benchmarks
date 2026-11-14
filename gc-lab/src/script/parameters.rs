//! Parameter setup is shared by ordinary and async functions, with a separate initializer scope.
use super::{
    parser::{self, Expr, Param, Stmt},
    vm::{Engine, FunctionKind, ScriptError, Value},
};
use crate::dual::Id;
type Result<T> = std::result::Result<T, ScriptError>;
impl Engine<'_> {
    pub(super) fn function_environment(
        &mut self,
        function: Id,
        parent: Id,
        params: &[Param],
        body: &[Stmt],
        kind: FunctionKind,
        receiver: Value,
        args: &[Value],
    ) -> Result<Id> {
        let names: Vec<_> = params.iter().flat_map(|p| p.binding.names()).collect();
        let mut declarations: Vec<_> = names
            .iter()
            .map(|name| Stmt::Let(name.clone(), Expr::Undefined))
            .collect();
        let pending = if matches!(
            kind,
            FunctionKind::Ordinary
                | FunctionKind::ClassConstructor
                | FunctionKind::DerivedConstructor
        ) {
            self.pending_construct.take()
        } else {
            None
        };
        if kind == FunctionKind::DerivedConstructor {
            declarations.push(Stmt::Let("this".into(), Expr::Undefined));
        }
        // Arrow default initializers execute in a parameter-local VarEnv.
        // Preserve the established non-arrow path: marking every parameter
        // record as function_scope changes direct eval and arguments binding
        // semantics of ordinary and async functions (Test262 regression).
        let parameter_var_scope = matches!(kind, FunctionKind::Arrow);
        let parameters = self.new_environment(
            Some(parent),
            &declarations,
            parameter_var_scope,
        )?;
        self.protect(&Value::Object(parameters))?;
        let sloppy = matches!(body.first(), Some(Stmt::Sloppy));
        let simple = params.iter().all(|p| {
            !p.rest && p.default.is_none() && matches!(p.binding, parser::Pattern::Name(_))
        });
        let mut mapped = None;
        self.set_env_strict(parameters, !sloppy);
        // Sloppy functions coerce `this`: nullish → global object, primitives → wrappers.
        let receiver = match receiver {
            Value::Undefined | Value::Null if sloppy => {
                Value::Object(self.intrinsics.as_ref().unwrap().global)
            }
            Value::String(_)
            | Value::Number(_)
            | Value::Bool(_)
            | Value::Symbol(_)
            | Value::BigInt(_)
                if sloppy =>
            {
                let boxed = self.box_value(receiver)?;
                Value::Object(boxed)
            }
            receiver => receiver,
        };
        if !matches!(kind, FunctionKind::Arrow | FunctionKind::AsyncArrow) {
            match pending {
                Some((new_target, func)) => {
                    self.initialize(parameters, "%newtarget", new_target)?;
                    self.initialize(parameters, "%func", func)?;
                }
                None => self.initialize(parameters, "%newtarget", Value::Undefined)?,
            }
            if kind != FunctionKind::DerivedConstructor {
                self.initialize(parameters, "this", receiver)?;
            }
            // §10.2.11 steps 15-18: skip the object when it is unobservable.
            let needed = match self.objects.get(&function) {
                Some(super::vm::Object::Closure {
                    needs_arguments, ..
                }) => *needs_arguments,
                _ => true,
            };
            if needed && !names.iter().any(|name| name == "arguments") {
                // Sloppy functions with simple parameter lists get a mapped arguments
                // object (CreateMappedArgumentsObject); the map is filled in below once
                // the body environment exists.
                let arguments = if sloppy && simple {
                    self.mapped_arguments_object(function, args)?
                } else {
                    self.arguments_object(args)?
                };
                if sloppy && simple {
                    mapped = arguments.id();
                }
                self.mutable_binding(parameters, "arguments", arguments)?;
            }
        }
        for (index, param) in params.iter().enumerate() {
            let mut value = if param.rest {
                let count = args.len().saturating_sub(index);
                let array = self.new_array(count as u32)?;
                self.protect(&Value::Object(array))?;
                for (i, value) in args.iter().skip(index).enumerate() {
                    self.put_data(array, i.to_string(), value.clone())?;
                }
                Value::Object(array)
            } else {
                args.get(index).cloned().unwrap_or(Value::Undefined)
            };
            self.protect(&value)?;
            if value == Value::Undefined {
                if let Some(default) = &param.default {
                    value = self.eval(default, parameters)?;
                    self.protect(&value)?;
                }
            }
            self.bind_pattern(&param.binding, value, parameters, parameters)?;
        }
        let local = self.new_environment(Some(parameters), body, true)?;
        self.protect(&Value::Object(local))?;
        let mut vars = vec![];
        parser::var_names(body, &mut vars);
        for name in vars {
            if names.contains(&name)
                || name == "arguments"
                    && !matches!(kind, FunctionKind::Arrow | FunctionKind::AsyncArrow)
            {
                let value = self.lookup(parameters, &name)?;
                self.initialize(local, &name, value)?;
            }
        }
        if let Some(arguments) = mapped {
            // Map index i to the last parameter named like it, in the environment
            // where the body resolves that name (the body copy when `var` redeclares it).
            let mut map: Vec<Option<(Id, String)>> = vec![None; args.len().min(params.len())];
            let mut seen = std::collections::HashSet::new();
            for index in (0..map.len()).rev() {
                let parser::Pattern::Name(name) = &params[index].binding else {
                    continue;
                };
                if !seen.insert(name.clone()) {
                    continue;
                }
                let env = if self.env_has_own(local, name) {
                    local
                } else {
                    parameters
                };
                map[index] = Some((env, name.clone()));
            }
            let mut d = self.data(arguments)?.clone();
            d.parameter_map = Some(map);
            self.replace_data(arguments, d)?;
        }
        Ok(local)
    }
}
