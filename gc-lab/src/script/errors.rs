//! Error construction uses the same descriptors and iterable machinery as user objects.
use super::vm::{Engine, ScriptError, Value};
type Result<T> = std::result::Result<T, ScriptError>;
impl Engine<'_> {
    pub(super) fn error_constructor(
        &mut self,
        name: &str,
        args: Vec<Value>,
        new_target: Option<Value>,
    ) -> Result<Value> {
        let default = self.intrinsics.as_ref().unwrap().errors[name];
        let prototype = if let Some(target) = new_target {
            let value = self.get_member(target, "prototype")?;
            if let Value::Object(id) = value {
                id
            } else {
                default
            }
        } else {
            default
        };
        let id = self.new_object(Some(prototype))?;
        let result = Value::Object(id);
        self.protect(&result)?;
        let mut data = self.data(id)?.clone();
        data.error_class = Some(name.into());
        self.replace_data(id, data)?;
        // §20.5.7.1 AggregateError(errors, message, options), §20.5.8.1
        // SuppressedError(error, suppressed, message): the message moves right.
        let offset = match name {
            "AggregateError" => 1,
            "SuppressedError" => 2,
            _ => 0,
        };
        let message = args.get(offset).cloned().unwrap_or(Value::Undefined);
        if message != Value::Undefined {
            let message = self.to_string(message)?;
            self.hidden(id, "message", Value::String(message))?;
        }
        if name == "SuppressedError" {
            // Steps 4-5: CreateNonEnumerableDataPropertyOrThrow "error", "suppressed".
            for (i, key) in ["error", "suppressed"].into_iter().enumerate() {
                let v = args.get(i).cloned().unwrap_or(Value::Undefined);
                self.hidden(id, key, v)?;
            }
            return Ok(result);
        }
        if let Some(Value::Object(options)) = args.get(offset + 1) {
            if self.has(*options, "cause")? {
                let cause = self.get_member(Value::Object(*options), "cause")?;
                self.hidden(id, "cause", cause)?;
            }
        }
        if name == "AggregateError" {
            let errors = self.new_array(0)?;
            self.protect(&Value::Object(errors))?;
            let mut index = 0u64;
            self.consume_iterable(args.first().cloned().unwrap_or(Value::Undefined), |e, v| {
                e.put_data(errors, index.to_string(), v)?;
                index += 1;
                Ok(())
            })?;
            self.hidden(id, "errors", Value::Object(errors))?;
        }
        Ok(result)
    }
    pub(super) fn error_to_string(&mut self, receiver: Value) -> Result<Value> {
        if !matches!(receiver, Value::Object(_)) {
            return self.throw_error("TypeError", "Error.toString requires an object");
        }
        let name = self.get_member(receiver.clone(), "name")?;
        self.protect(&name)?;
        let name = if name == Value::Undefined {
            "Error".into()
        } else {
            self.to_string(name)?
        };
        let message = self.get_member(receiver, "message")?;
        let message = if message == Value::Undefined {
            String::new()
        } else {
            self.to_string(message)?
        };
        Ok(Value::String(if name.is_empty() {
            message
        } else if message.is_empty() {
            name
        } else {
            format!("{name}: {message}")
        }))
    }
}
