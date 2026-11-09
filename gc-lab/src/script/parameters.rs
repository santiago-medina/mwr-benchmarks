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
        let parameters = self.new_environment(Some(parent), &declarations, false)?;
        self.protect(&Value::Object(parameters))?;
        let sloppy = matches!(body.first(), Some(Stmt::Sloppy));
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
            if !names.iter().any(|name| name == "arguments") {
                let arguments = self.arguments_object(args)?;
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
        Ok(local)
    }
}
