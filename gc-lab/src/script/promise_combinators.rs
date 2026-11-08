//! Aggregate jobs retain all state through traced cells, including hostile thenables.
use super::{
    promises::{Capability, Cell, Internal},
    vm::{Engine, ScriptError, Value},
};
use crate::dual::Id;
type Result<T> = std::result::Result<T, ScriptError>;
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    All,
    Settled,
    Any,
}
#[derive(Clone)]
pub(super) struct Aggregate {
    pub capability: Capability,
    pub values: Id,
    pub remaining: u64,
    pub mode: Mode,
}
impl Aggregate {
    pub fn edges(&self) -> Vec<Id> {
        std::iter::once(self.values)
            .chain(self.capability.values().iter().filter_map(Value::id))
            .collect()
    }
}
impl Engine<'_> {
    fn aggregate_state(&self, id: Id) -> Result<Aggregate> {
        let Some(Cell::Aggregate(state)) = self.data(id)?.cell.clone() else {
            return Err(ScriptError::Runtime("missing aggregate cell"));
        };
        Ok(state)
    }
    fn aggregate_save(&mut self, id: Id, state: Aggregate) -> Result<()> {
        let mut data = self.data(id)?.clone();
        data.cell = Some(Cell::Aggregate(state));
        self.replace_data(id, data)
    }
    fn aggregate_done(&mut self, id: Id) -> Result<()> {
        let mut state = self.aggregate_state(id)?;
        state.remaining -= 1;
        let done = state.remaining == 0;
        self.aggregate_save(id, state.clone())?;
        if done {
            if state.mode == Mode::Any {
                let error = self.error_constructor(
                    "AggregateError",
                    vec![Value::Object(state.values)],
                    None,
                )?;
                self.protect(&error)?;
                self.invoke(state.capability.reject, vec![error])?;
            } else {
                self.invoke(state.capability.resolve, vec![Value::Object(state.values)])?;
            }
        }
        Ok(())
    }
    pub(super) fn aggregate_element(
        &mut self,
        group: Id,
        guard: Id,
        index: u64,
        reject: bool,
        value: Value,
    ) -> Result<Value> {
        let mut data = self.data(guard)?.clone();
        let Some(Cell::Called(called)) = data.cell.as_mut() else {
            return Err(ScriptError::Runtime("missing aggregate guard"));
        };
        if *called {
            return Ok(Value::Undefined);
        }
        *called = true;
        self.replace_data(guard, data)?;
        let state = self.aggregate_state(group)?;
        let value = if state.mode == Mode::Settled {
            let id = self.new_object(Some(self.intrinsics.as_ref().unwrap().object))?;
            self.protect(&Value::Object(id))?;
            self.put_data(
                id,
                "status",
                Value::String(if reject { "rejected" } else { "fulfilled" }.into()),
            )?;
            self.put_data(id, if reject { "reason" } else { "value" }, value)?;
            Value::Object(id)
        } else {
            value
        };
        self.put_data(state.values, index.to_string(), value)?;
        self.aggregate_done(group)?;
        Ok(Value::Undefined)
    }
    pub(super) fn promise_combinator(
        &mut self,
        name: &str,
        constructor: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let cap = self.capability(constructor.clone())?;
        if name == "Promise.withResolvers" {
            let id = self.new_object(Some(self.intrinsics.as_ref().unwrap().object))?;
            self.protect(&Value::Object(id))?;
            self.put_data(id, "promise", cap.promise)?;
            self.put_data(id, "resolve", cap.resolve)?;
            self.put_data(id, "reject", cap.reject)?;
            return Ok(Value::Object(id));
        }
        if name == "Promise.try" {
            let callback = args.first().cloned().unwrap_or(Value::Undefined);
            match self.invoke(callback, args.into_iter().skip(1).collect()) {
                Ok(value) => {
                    self.protect(&value)?;
                    self.invoke(cap.resolve, vec![value])?;
                }
                Err(ScriptError::Thrown(error)) => {
                    self.protect(&error.value)?;
                    self.invoke(cap.reject, vec![error.value])?;
                }
                Err(error) => return Err(error),
            }
            return Ok(cap.promise);
        }
        let outcome = self.promise_combine(
            name,
            constructor,
            args.first().cloned().unwrap_or(Value::Undefined),
            cap.clone(),
        );
        match outcome {
            Ok(()) => {}
            Err(ScriptError::Thrown(e)) => {
                self.protect(&e.value)?;
                self.invoke(cap.reject, vec![e.value])?;
            }
            Err(e) => return Err(e),
        }
        Ok(cap.promise)
    }
    fn promise_combine(
        &mut self,
        name: &str,
        constructor: Value,
        input: Value,
        cap: Capability,
    ) -> Result<()> {
        let resolve = self.get_member(constructor.clone(), "resolve")?;
        self.protect(&resolve)?;
        if !self.callable(&resolve) {
            return self.throw_error("TypeError", "Promise resolve method is not callable");
        }
        let group = if name == "Promise.race" {
            None
        } else {
            let values = self.new_array(0)?;
            self.protect(&Value::Object(values))?;
            let id = self.new_object(None)?;
            self.protect(&Value::Object(id))?;
            self.aggregate_save(
                id,
                Aggregate {
                    capability: cap.clone(),
                    values,
                    remaining: 1,
                    mode: if name == "Promise.all" {
                        Mode::All
                    } else if name == "Promise.any" {
                        Mode::Any
                    } else {
                        Mode::Settled
                    },
                },
            )?;
            Some(id)
        };
        let mut index = 0u64;
        self.consume_iterable(input, |e, v| {
            if index >= 9007199254740991 {
                return e.throw_error("RangeError", "too many Promise elements");
            }
            let promise = e.invoke_this(resolve.clone(), constructor.clone(), vec![v])?;
            e.protect(&promise)?;
            let (fulfilled, rejected) = if let Some(group) = group {
                let mut state = e.aggregate_state(group)?;
                e.put_data(state.values, index.to_string(), Value::Undefined)?;
                state.remaining += 1;
                e.aggregate_save(group, state.clone())?;
                let guard = e.new_object(None)?;
                e.protect(&Value::Object(guard))?;
                let mut data = e.data(guard)?.clone();
                data.cell = Some(Cell::Called(false));
                e.replace_data(guard, data)?;
                let mut element = |reject| {
                    e.internal_function(
                        Internal::AggregateElement {
                            group,
                            guard,
                            index,
                            reject,
                        },
                        1,
                    )
                };
                match state.mode {
                    Mode::All => (element(false)?, cap.reject.clone()),
                    Mode::Any => (cap.resolve.clone(), element(true)?),
                    Mode::Settled => (element(false)?, element(true)?),
                }
            } else {
                (cap.resolve.clone(), cap.reject.clone())
            };
            let then = e.get_member(promise.clone(), "then")?;
            e.protect(&then)?;
            e.invoke_this(then, promise, vec![fulfilled, rejected])?;
            index += 1;
            Ok(())
        })?;
        if let Some(group) = group {
            self.aggregate_done(group)?;
        }
        Ok(())
    }
}
