//! Copying array operations reuse descriptor paths, species and the stable sorter.
use super::{
    objects::Key,
    vm::{Engine, ScriptError, Value},
};
use crate::dual::Id;
type Result<T> = std::result::Result<T, ScriptError>;
impl Engine<'_> {
    fn copy_array(&mut self, len: u64) -> Result<Id> {
        if len > u32::MAX as u64 {
            return self.throw_error("RangeError", "array length exceeds supported range");
        }
        let id = self.new_array(len as u32)?;
        self.protect(&Value::Object(id))?;
        Ok(id)
    }
    fn flatten_into(
        &mut self,
        out: Id,
        source: Id,
        len: u64,
        depth: f64,
        index: &mut u64,
        mapper: Option<(Value, Value)>,
        nesting: usize,
    ) -> Result<()> {
        if nesting > 256 {
            return self.throw_error("RangeError", "host array flattening depth limit");
        }
        for k in 0..len {
            self.tick()?;
            if !self.has(source, k.to_string())? {
                continue;
            }
            self.frame(|e| {
                let value = e.get_member(Value::Object(source), k.to_string())?;
                e.protect(&value)?;
                let value = if let Some((callback, this)) = &mapper {
                    e.invoke_this(
                        callback.clone(),
                        this.clone(),
                        vec![value, Value::number(k as f64), Value::Object(source)],
                    )?
                } else {
                    value
                };
                e.protect(&value)?;
                if depth > 0.0 {
                    if let Value::Object(id) = value {
                        if e.data(id)?.array {
                            let len = e.length_of(id)?;
                            return e.flatten_into(
                                out,
                                id,
                                len,
                                depth - 1.0,
                                index,
                                None,
                                nesting + 1,
                            );
                        }
                    }
                }
                if *index >= 9007199254740991 {
                    return e.throw_error("TypeError", "flattened output exceeds safe length");
                }
                e.put_data(out, index.to_string(), value)?;
                *index += 1;
                Ok(())
            })?;
        }
        Ok(())
    }
    pub(super) fn array_copy(
        &mut self,
        name: &str,
        id: Id,
        len: u64,
        args: Vec<Value>,
    ) -> Result<Value> {
        let arg = |i| args.get(i).cloned().unwrap_or(Value::Undefined);
        let receiver = Value::Object(id);
        let name = name.rsplit('.').next().unwrap();
        match name {
            "concat" => {
                let out = self.array_species_create(id, 0)?;
                self.protect(&Value::Object(out))?;
                let mut index = 0u64;
                for item in std::iter::once(receiver).chain(args) {
                    self.tick()?;
                    let spread = if let Value::Object(source) = item {
                        let spread =
                            self.get_member(item.clone(), Key::Symbol(super::symbols::SPREADABLE))?;
                        if spread == Value::Undefined {
                            // §23.1.3.2.1 IsConcatSpreadable step 4: IsArray(O).
                            self.is_array(&Value::Object(source))?
                        } else {
                            spread.truthy()
                        }
                    } else {
                        false
                    };
                    if spread {
                        let Value::Object(source) = item else {
                            unreachable!()
                        };
                        let len = self.length_of(source)?;
                        if index + len > 9007199254740991 {
                            return self
                                .throw_error("TypeError", "concat output exceeds safe length");
                        }
                        for k in 0..len {
                            self.tick()?;
                            if self.has(source, k.to_string())? {
                                let value =
                                    self.get_member(Value::Object(source), k.to_string())?;
                                self.put_data(out, index.to_string(), value)?;
                            }
                            index += 1;
                        }
                    } else {
                        if index >= 9007199254740991 {
                            return self
                                .throw_error("TypeError", "concat output exceeds safe length");
                        }
                        self.put_data(out, index.to_string(), item)?;
                        index += 1;
                    }
                }
                self.set_member(Value::Object(out), "length", Value::number(index as f64))?;
                Ok(Value::Object(out))
            }
            "flat" | "flatMap" => {
                let mapper = if name == "flatMap" {
                    let f = arg(0);
                    if !self.callable(&f) {
                        return self.throw_error("TypeError", "flatMap callback is not callable");
                    }
                    Some((f, arg(1)))
                } else {
                    None
                };
                let depth = if name == "flatMap" || arg(0) == Value::Undefined {
                    1.0
                } else {
                    self.integer(arg(0))?.max(0.0)
                };
                let out = self.array_species_create(id, 0)?;
                self.protect(&Value::Object(out))?;
                let mut index = 0;
                self.flatten_into(out, id, len, depth, &mut index, mapper, 0)?;
                Ok(Value::Object(out))
            }
            "toReversed" | "toSorted" | "with" | "toSpliced" => {
                if name == "toSorted" && arg(0) != Value::Undefined && !self.callable(&arg(0)) {
                    return self.throw_error("TypeError", "sort comparison is not callable");
                }
                let replace = if name == "with" {
                    let n = self.integer(arg(0))?;
                    let n = if n < 0.0 { len as f64 + n } else { n };
                    if n < 0.0 || n >= len as f64 {
                        return self.throw_error("RangeError", "replacement index is out of range");
                    }
                    Some(n as u64)
                } else {
                    None
                };
                let (start, delete, insert) = if name == "toSpliced" {
                    let start = self.relative_index(arg(0), len)?;
                    let delete = if args.is_empty() {
                        0
                    } else if args.len() == 1 {
                        len - start
                    } else {
                        self.integer(arg(1))?.max(0.0).min((len - start) as f64) as u64
                    };
                    (start, delete, args.len().saturating_sub(2) as u64)
                } else {
                    (0, 0, 0)
                };
                let count = len - delete + insert;
                if count > 9007199254740991 {
                    return self.throw_error("TypeError", "copied output exceeds safe length");
                }
                let out = self.copy_array(count)?;
                for k in 0..count {
                    self.tick()?;
                    let value = if replace == Some(k) {
                        arg(1)
                    } else if name == "toSpliced" && k >= start && k < start + insert {
                        args[(k - start) as usize + 2].clone()
                    } else {
                        let source = if name == "toReversed" {
                            len - k - 1
                        } else if name == "toSpliced" && k >= start + insert {
                            k + delete - insert
                        } else {
                            k
                        };
                        self.get_member(receiver.clone(), source.to_string())?
                    };
                    self.put_data(out, k.to_string(), value)?;
                }
                if name == "toSorted" {
                    self.array_extended("sort", out, count, args)?;
                }
                Ok(Value::Object(out))
            }
            _ => Err(ScriptError::Unsupported("array operation")),
        }
    }
}
