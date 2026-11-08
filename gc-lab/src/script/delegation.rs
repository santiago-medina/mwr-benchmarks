//! Synchronous yield-star protocol; ordinary yields and delegated results stay distinct.
use super::{
    generators::Resume,
    vm::{Engine, ScriptError, Value},
};
type Result<T> = std::result::Result<T, ScriptError>;
#[derive(Clone)]
pub(super) struct Delegation {
    pub iterator: Value,
    pub next: Value,
    pub output: usize,
}
pub(super) enum Delegated {
    Yield(Value),
    Next(Value),
    Return(Value),
}
impl Engine<'_> {
    pub(super) fn delegate_call(
        &mut self,
        delegate: &Delegation,
        input: Value,
        mode: Resume,
    ) -> Result<Delegated> {
        let method = if mode == Resume::Next {
            delegate.next.clone()
        } else {
            let method = self.get_member(
                delegate.iterator.clone(),
                if mode == Resume::Throw {
                    "throw"
                } else {
                    "return"
                },
            )?;
            self.protect(&method)?;
            if matches!(method, Value::Null | Value::Undefined) {
                if mode == Resume::Return {
                    return Ok(Delegated::Return(input));
                }
                self.close_iterator(delegate.iterator.clone(), Ok(None))?;
                return self.throw_error("TypeError", "delegated iterator has no throw method");
            }
            method
        };
        let result = self.invoke_this(method, delegate.iterator.clone(), vec![input])?;
        self.protect(&result)?;
        if !matches!(result, Value::Object(_)) {
            return self.throw_error("TypeError", "delegated iterator result is not an object");
        }
        if !self.get_member(result.clone(), "done")?.truthy() {
            return Ok(Delegated::Yield(result));
        }
        let value = self.get_member(result, "value")?;
        self.protect(&value)?;
        Ok(if mode == Resume::Return {
            Delegated::Return(value)
        } else {
            Delegated::Next(value)
        })
    }
    pub(super) fn forward_generator_result(
        &mut self,
        id: crate::dual::Id,
        result: Value,
    ) -> Result<()> {
        self.protect(&result)?;
        let mut data = self.data(id)?.clone();
        let generator = data.generator.as_mut().unwrap();
        generator.state = super::generators::State::Yield;
        generator.output = Some(Ok(result));
        self.replace_data(id, data)
    }
}
