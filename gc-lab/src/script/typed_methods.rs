//! Typed-array algorithms use internal lengths, shared species and the common stable sorter.
use super::{
    buffer_elements::Element,
    objects::{Descriptor, Key},
    typed_arrays::{Typed, NAMES},
    vm::{Engine, ScriptError, Value},
};
use crate::dual::Id;
type Result<T> = std::result::Result<T, ScriptError>;
impl Engine<'_> {
    pub(super) fn bootstrap_typed_methods(
        &mut self,
        env: Id,
        base: Id,
        proto: Id,
        function: Id,
    ) -> Result<()> {
        for (method, length) in [
            ("at", 1),
            ("join", 1),
            ("includes", 1),
            ("indexOf", 1),
            ("lastIndexOf", 1),
            ("forEach", 1),
            ("map", 1),
            ("filter", 1),
            ("some", 1),
            ("every", 1),
            ("find", 1),
            ("findIndex", 1),
            ("findLast", 1),
            ("findLastIndex", 1),
            ("reduce", 1),
            ("reduceRight", 1),
            ("sort", 1),
            ("toSorted", 1),
        ] {
            let full = format!("%TypedArray%.prototype.{method}");
            let f = self.intrinsic_function(env, &full, Some(function))?;
            self.define(
                f,
                "length",
                Descriptor {
                    value: Some(Value::number(length as f64)),
                    ..Default::default()
                },
            )?;
            self.hidden(proto, method, Value::Object(f))?;
        }
        let _ = base;
        Ok(())
    }
    pub(super) fn typed_default(&mut self, kind: Element, length: u64) -> Result<Value> {
        let name = NAMES.iter().find(|(_, k)| *k == kind).unwrap().0;
        let proto = self.intrinsics.as_ref().unwrap().collection[name];
        self.typed_owned(proto, kind, length)
    }
    pub(super) fn typed_species(
        &mut self,
        id: Id,
        kind: Element,
        args: Vec<Value>,
        minimum: Option<usize>,
    ) -> Result<(Value, Typed)> {
        let name = NAMES.iter().find(|(_, k)| *k == kind).unwrap().0;
        let default = Value::Object(self.intrinsics.as_ref().unwrap().functions[name]);
        let constructor = self.get_member(Value::Object(id), "constructor")?;
        self.protect(&constructor)?;
        let constructor = if constructor == Value::Undefined {
            default
        } else {
            if !matches!(constructor, Value::Object(_)) {
                return self.throw_error("TypeError", "typed-array constructor is not an object");
            }
            let species = self.get_member(constructor, Key::Symbol(2))?;
            if matches!(species, Value::Null | Value::Undefined) {
                default
            } else {
                species
            }
        };
        self.protect(&constructor)?;
        let result = self.construct(constructor, args)?;
        self.protect(&result)?;
        let (_, typed, length) = self.typed_record(result.clone())?;
        if typed.kind.big() != kind.big() {
            return self.throw_error("TypeError", "typed-array species content type differs");
        }
        if minimum.is_some_and(|min| length < min) {
            return self.throw_error("TypeError", "typed-array species result is too short");
        }
        Ok((result, typed))
    }
    pub(super) fn typed_method(
        &mut self,
        method: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let arg = |i| args.get(i).cloned().unwrap_or(Value::Undefined);
        let (id, typed, length) = self.typed_record(receiver.clone())?;
        let len = length as u64;
        match method {
            "at" => {
                let n = self.integer(arg(0))?;
                let n = if n < 0.0 { length as f64 + n } else { n };
                if n < 0.0 || n >= length as f64 {
                    return Ok(Value::Undefined);
                }
                self.typed_get(&typed, n)
            }
            "join" => {
                let separator = if arg(0) == Value::Undefined {
                    ",".into()
                } else {
                    self.to_string(arg(0))?
                };
                let mut text = String::new();
                for index in 0..length {
                    self.tick()?;
                    if index != 0 {
                        text.push_str(&separator);
                    }
                    let value = self.typed_get(&typed, index as f64)?;
                    if value != Value::Undefined {
                        text.push_str(&self.to_string(value)?);
                    }
                    if text.len() > 1048576 {
                        return self.throw_error("RangeError", "host string size limit");
                    }
                }
                Ok(Value::String(text))
            }
            "indexOf" | "includes" | "lastIndexOf" => {
                if length == 0 {
                    return Ok(if method == "includes" {
                        Value::Bool(false)
                    } else {
                        Value::number(-1.0)
                    });
                }
                let reverse = method == "lastIndexOf";
                let n = if reverse && args.len() < 2 {
                    (length - 1) as f64
                } else {
                    self.integer(arg(1))?
                };
                let start = if reverse {
                    if n < 0.0 {
                        length as f64 + n
                    } else {
                        n.min((length - 1) as f64)
                    }
                } else {
                    if n < 0.0 {
                        (length as f64 + n).max(0.0)
                    } else {
                        n
                    }
                };
                if start < 0.0 || start >= length as f64 {
                    return Ok(if method == "includes" {
                        Value::Bool(false)
                    } else {
                        Value::number(-1.0)
                    });
                }
                let mut index = start as usize;
                loop {
                    self.tick()?;
                    let value = self.typed_get(&typed, index as f64)?;
                    let found = if method == "includes" {
                        value.same_value_zero(&arg(0))
                    } else {
                        self.typed_valid(&typed, index as f64)? && value.strict_equal(&arg(0))
                    };
                    if found {
                        return Ok(if method == "includes" {
                            Value::Bool(true)
                        } else {
                            Value::number(index as f64)
                        });
                    }
                    if reverse {
                        if index == 0 {
                            break;
                        }
                        index -= 1;
                    } else {
                        index += 1;
                        if index >= length {
                            break;
                        }
                    }
                }
                Ok(if method == "includes" {
                    Value::Bool(false)
                } else {
                    Value::number(-1.0)
                })
            }
            "forEach" | "map" | "filter" | "some" | "every" | "find" | "findIndex" | "findLast"
            | "findLastIndex" => {
                let callback = arg(0);
                if !self.callable(&callback) {
                    return self.throw_error("TypeError", "typed-array callback is not callable");
                }
                let this = arg(1);
                let mut selected = vec![];
                let output = if method == "map" {
                    Some(self.typed_species(
                        id,
                        typed.kind,
                        vec![Value::number(length as f64)],
                        Some(length),
                    )?)
                } else {
                    None
                };
                let reverse = method.starts_with("findLast");
                for position in 0..length {
                    self.tick()?;
                    let index = if reverse {
                        length - position - 1
                    } else {
                        position
                    };
                    let result = self.frame(|e| {
                        let value = e.typed_get(&typed, index as f64)?;
                        let result = e.invoke_this(
                            callback.clone(),
                            this.clone(),
                            vec![value.clone(), Value::number(index as f64), receiver.clone()],
                        )?;
                        e.protect(&result)?;
                        match method {
                            "map" => {
                                e.typed_write(&output.as_ref().unwrap().1, index as f64, result)?;
                            }
                            "filter" => {
                                if result.truthy() {
                                    selected.push(value);
                                }
                            }
                            "some" if result.truthy() => return Ok(Some(Value::Bool(true))),
                            "every" if !result.truthy() => return Ok(Some(Value::Bool(false))),
                            method if method.starts_with("find") && result.truthy() => {
                                return Ok(Some(if method.ends_with("Index") {
                                    Value::number(index as f64)
                                } else {
                                    value
                                }))
                            }
                            _ => {}
                        }
                        Ok(None)
                    })?;
                    if let Some(result) = result {
                        return Ok(result);
                    }
                }
                if method == "filter" {
                    let (result, out) = self.typed_species(
                        id,
                        typed.kind,
                        vec![Value::number(selected.len() as f64)],
                        Some(selected.len()),
                    )?;
                    for (index, value) in selected.into_iter().enumerate() {
                        self.tick()?;
                        self.typed_write(&out, index as f64, value)?;
                    }
                    return Ok(result);
                }
                Ok(match method {
                    "map" => output.unwrap().0,
                    "some" => Value::Bool(false),
                    "every" => Value::Bool(true),
                    "findIndex" | "findLastIndex" => Value::number(-1.0),
                    _ => Value::Undefined,
                })
            }
            "reduce" | "reduceRight" => {
                let callback = arg(0);
                if !self.callable(&callback) {
                    return self.throw_error("TypeError", "typed-array reducer is not callable");
                }
                if length == 0 && args.len() < 2 {
                    return self.throw_error(
                        "TypeError",
                        "reduce without an initial value on empty input",
                    );
                }
                let carrier = self.new_object(None)?;
                self.protect(&Value::Object(carrier))?;
                let reverse = method == "reduceRight";
                let mut start = 0;
                let initial = if args.len() >= 2 {
                    arg(1)
                } else {
                    start = 1;
                    self.typed_get(&typed, if reverse { (length - 1) as f64 } else { 0.0 })?
                };
                self.put_data(carrier, "value", initial)?;
                for position in start..length {
                    self.tick()?;
                    let index = if reverse {
                        length - position - 1
                    } else {
                        position
                    };
                    self.frame(|e| {
                        let accumulator = e.get_member(Value::Object(carrier), "value")?;
                        let value = e.typed_get(&typed, index as f64)?;
                        let result = e.invoke(
                            callback.clone(),
                            vec![
                                accumulator,
                                value,
                                Value::number(index as f64),
                                receiver.clone(),
                            ],
                        )?;
                        e.put_data(carrier, "value", result)
                    })?;
                }
                self.get_member(Value::Object(carrier), "value")
            }
            "sort" | "toSorted" => {
                let callback = arg(0);
                if callback != Value::Undefined && !self.callable(&callback) {
                    return self.throw_error("TypeError", "typed-array comparator is not callable");
                }
                let output = if method == "toSorted" {
                    self.typed_default(typed.kind, len)?
                } else {
                    receiver
                };
                let (_, out, _) = self.typed_record(output.clone())?;
                let mut values = vec![];
                for index in 0..length {
                    self.tick()?;
                    values.push(self.typed_get(&typed, index as f64)?);
                }
                self.stable_sort_values(&mut values, |e, a, b| e.typed_compare(&callback, a, b))?;
                for (index, value) in values.into_iter().enumerate() {
                    self.tick()?;
                    self.typed_write(&out, index as f64, value)?;
                }
                Ok(output)
            }
            _ => Err(ScriptError::Unsupported("typed-array method")),
        }
    }
    fn typed_compare(&mut self, callback: &Value, a: &Value, b: &Value) -> Result<i8> {
        if *callback != Value::Undefined {
            let result = self.invoke(callback.clone(), vec![a.clone(), b.clone()])?;
            let n = self.to_number(result)?;
            return Ok(if n.is_nan() || n == 0.0 {
                0
            } else if n < 0.0 {
                -1
            } else {
                1
            });
        }
        if let (Value::BigInt(a), Value::BigInt(b)) = (a, b) {
            return Ok(match a.compare(b) {
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Greater => 1,
                _ => 0,
            });
        }
        let (Value::Number(a), Value::Number(b)) = (a, b) else {
            return Err(ScriptError::Runtime("invalid typed-array sort value"));
        };
        let (a, b) = (f64::from_bits(*a), f64::from_bits(*b));
        Ok(if a.is_nan() {
            if b.is_nan() {
                0
            } else {
                1
            }
        } else if b.is_nan() {
            -1
        } else if a == 0.0 && b == 0.0 && a.is_sign_negative() != b.is_sign_negative() {
            if a.is_sign_negative() {
                -1
            } else {
                1
            }
        } else if a < b {
            -1
        } else if a > b {
            1
        } else {
            0
        })
    }
}
