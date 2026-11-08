//! Shared spread paths root accumulated values and preserve ordinary property semantics.
use super::{
    objects::Key,
    parser::Expr,
    vm::{Engine, ScriptError, Value},
};
use crate::dual::Id;
type Result<T> = std::result::Result<T, ScriptError>;
impl Engine<'_> {
    pub(super) fn append_array(
        &mut self,
        target: Id,
        value: Option<Value>,
        spread: bool,
    ) -> Result<()> {
        let mut index = self.length_of(target)?;
        if spread {
            let value = value.unwrap();
            self.protect(&value)?;
            self.consume_iterable(value, |e, value| {
                if index >= u32::MAX as u64 {
                    return e.throw_error("RangeError", "array accumulation length limit");
                }
                e.put_data(target, index.to_string(), value)?;
                index += 1;
                Ok(())
            })?;
        } else {
            if index >= u32::MAX as u64 {
                return self.throw_error("RangeError", "array accumulation length limit");
            }
            if let Some(value) = value {
                self.put_data(target, index.to_string(), value)?;
            } else {
                self.set_member(
                    Value::Object(target),
                    "length",
                    Value::number((index + 1) as f64),
                )?;
            }
        }
        Ok(())
    }
    pub(super) fn array_literal(&mut self, elements: &[Option<Expr>], env: Id) -> Result<Value> {
        let array = self.new_array(0)?;
        self.protect(&Value::Object(array))?;
        for expression in elements {
            self.frame(|e| match expression {
                None => e.append_array(array, None, false),
                Some(Expr::Spread(inner)) => {
                    let value = e.eval(inner, env)?;
                    e.append_array(array, Some(value), true)
                }
                Some(expression) => {
                    let value = e.eval(expression, env)?;
                    e.append_array(array, Some(value), false)
                }
            })?;
        }
        Ok(Value::Object(array))
    }
    pub(super) fn argument_values(&mut self, array: Id) -> Result<Vec<Value>> {
        let length = self.length_of(array)?;
        let mut values = vec![];
        for index in 0..length {
            self.tick()?;
            values.push(self.get_member(Value::Object(array), index.to_string())?);
        }
        Ok(values)
    }
    pub(super) fn spread_arguments(&mut self, expressions: &[Expr], env: Id) -> Result<Vec<Value>> {
        let array = self.new_array(0)?;
        self.protect(&Value::Object(array))?;
        for expression in expressions {
            self.frame(|e| {
                let (expression, spread) = match expression {
                    Expr::Spread(inner) => (inner.as_ref(), true),
                    expression => (expression, false),
                };
                let value = e.eval(expression, env)?;
                e.append_array(array, Some(value), spread)
            })?;
        }
        self.argument_values(array)
    }
    pub(super) fn copy_properties(
        &mut self,
        target: Id,
        source: Value,
        excluded: &[Key],
    ) -> Result<()> {
        if matches!(source, Value::Null | Value::Undefined) {
            return Ok(());
        }
        let source = self.object_coercible(source)?;
        self.protect(&Value::Object(source))?;
        for key in self.own_keys(source)? {
            self.tick()?;
            if excluded.contains(&key) {
                continue;
            }
            if self.own(source, &key)?.is_some_and(|p| p.enumerable) {
                self.frame(|e| {
                    let value = e.get_member(Value::Object(source), &key)?;
                    e.protect(&value)?;
                    e.put_data(target, key, value)
                })?;
            }
        }
        Ok(())
    }
}
