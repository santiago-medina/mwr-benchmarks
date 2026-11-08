//! Dynamic constructors use the existing strict grammar and capture only the global environment.
use super::{
    parser::{self, Stmt},
    vm::{Engine, FunctionKind, ScriptError, Value},
};
type Result<T> = std::result::Result<T, ScriptError>;
impl Engine<'_> {
    pub(super) fn dynamic_function(
        &mut self,
        asynchronous: bool,
        args: Vec<Value>,
        new_target: Option<Value>,
    ) -> Result<Value> {
        let mut strings = vec![];
        for value in args {
            strings.push(self.to_string(value)?);
        }
        let body = strings.pop().unwrap_or_default();
        let parameters = strings.join(",");
        let prefix = if asynchronous {
            "async function"
        } else {
            "function"
        };
        // Parse parameter text independently so it cannot inject statements into the body.
        let header = format!("{prefix} anonymous({parameters}\n) {{}}");
        let parsed = match parser::parse(&header) {
            Ok(body) => body,
            Err(_) => {
                return self.throw_error("SyntaxError", "invalid dynamic function parameters")
            }
        };
        if !matches!(
            parsed.as_slice(),
            [Stmt::Function(..)] | [Stmt::AsyncFunction(..)]
        ) {
            return self.throw_error("SyntaxError", "invalid dynamic function parameters");
        }
        let source = format!("{prefix} anonymous({parameters}\n) {{\n{body}\n}}");
        let parsed = match parser::parse(&source) {
            Ok(body) => body,
            Err(_) => return self.throw_error("SyntaxError", "invalid dynamic function body"),
        };
        let (params, body) = match parsed.as_slice() {
            [Stmt::Function(_, params, body)] if !asynchronous => (params, body),
            [Stmt::AsyncFunction(_, params, body)] if asynchronous => (params, body),
            _ => return self.throw_error("SyntaxError", "invalid dynamic function syntax"),
        };
        let prototype = if let Some(target) = new_target {
            let value = self.get_member(target, "prototype")?;
            if let Value::Object(id) = value {
                Some(id)
            } else {
                None
            }
        } else {
            None
        };
        if let Some(prototype) = prototype {
            self.protect(&Value::Object(prototype))?;
        }
        let global = self.global.ok_or(ScriptError::Runtime(
            "dynamic function without global environment",
        ))?;
        let global = self.runtime.resolve(global)?;
        let function = self.make_function(
            global,
            Some("anonymous"),
            params,
            body,
            if asynchronous {
                FunctionKind::Async
            } else {
                FunctionKind::Ordinary
            },
            false,
        )?;
        let Value::Object(id) = function else {
            unreachable!()
        };
        let mut data = self.data(id)?.clone();
        data.source = Some(source);
        if let Some(prototype) = prototype {
            data.prototype = Some(prototype);
        }
        self.replace_data(id, data)?;
        Ok(function)
    }
    pub(super) fn function_source(&mut self, receiver: Value) -> Result<Value> {
        if !self.callable(&receiver) {
            return self.throw_error("TypeError", "Function.toString requires a callable");
        }
        if let Value::Object(id) = receiver {
            if let Some(source) = self.data(id)?.source.clone() {
                return Ok(Value::String(source));
            }
        }
        // Source availability is a host choice for pre-existing closures and native functions.
        Ok(Value::String("function () { [native code] }".into()))
    }
}
