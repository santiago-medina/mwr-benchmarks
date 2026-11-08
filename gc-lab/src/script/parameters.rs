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
        let declarations: Vec<_> = params
            .iter()
            .map(|p| Stmt::Let(p.name.clone(), Expr::Undefined))
            .collect();
        let parameters = self.new_environment(Some(parent), &declarations, false)?;
        self.protect(&Value::Object(parameters))?;
        if !matches!(kind, FunctionKind::Arrow | FunctionKind::AsyncArrow) {
            self.initialize(parameters, "this", receiver)?;
            if !params.iter().any(|p| p.name == "arguments") {
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
            self.initialize(parameters, &param.name, value)?;
        }
        let local = self.new_environment(Some(parameters), body, true)?;
        self.protect(&Value::Object(local))?;
        let mut names = vec![];
        parser::var_names(body, &mut names);
        for name in names {
            if params.iter().any(|p| p.name == name)
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
