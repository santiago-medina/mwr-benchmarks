//! Symbol identity, descriptions and registry are preserved with the engine's realm state.
use super::{
    objects::{Descriptor, Key},
    vm::{Engine, ScriptError, Value},
};
use crate::dual::Id;
use std::collections::HashMap;
type Result<T> = std::result::Result<T, ScriptError>;
pub(super) const PRIMITIVE: u64 = 3;
pub(super) const HAS_INSTANCE: u64 = 4;
pub(super) const SPREADABLE: u64 = 5;
pub(super) const UNSCOPABLES: u64 = 11;
pub(super) const WELL_KNOWN: &[&str] = &[
    "iterator",
    "toStringTag",
    "species",
    "toPrimitive",
    "hasInstance",
    "isConcatSpreadable",
    "match",
    "matchAll",
    "replace",
    "search",
    "split",
    "unscopables",
    "asyncIterator",
    "dispose",
    "asyncDispose",
];
impl Engine<'_> {
    pub(super) fn new_symbol(&mut self, description: Option<String>) -> Result<Value> {
        let id = self.next_symbol;
        self.next_symbol = id
            .checked_add(1)
            .ok_or(ScriptError::Runtime("symbol identity limit"))?;
        self.symbol_descriptions.insert(id, description);
        Ok(Value::Symbol(id))
    }
    pub(super) fn symbol_description(&self, id: u64) -> Option<String> {
        if let Some(name) = WELL_KNOWN.get(id as usize) {
            Some(format!("Symbol.{name}"))
        } else {
            self.symbol_descriptions.get(&id).cloned().flatten()
        }
    }
    pub(super) fn symbol_string(&self, id: u64) -> String {
        format!(
            "Symbol({})",
            self.symbol_description(id).unwrap_or_default()
        )
    }
    fn this_symbol(&mut self, value: Value) -> Result<u64> {
        match value {
            Value::Symbol(id) => Ok(id),
            Value::Object(id) => {
                if let Some(Value::Symbol(symbol)) =
                    self.data(id).ok().and_then(|d| d.boxed.clone())
                {
                    Ok(symbol)
                } else {
                    self.throw_error("TypeError", "invalid Symbol receiver")
                }
            }
            _ => self.throw_error("TypeError", "invalid Symbol receiver"),
        }
    }
    pub(super) fn bootstrap_symbols(
        &mut self,
        env: Id,
        objects: &mut HashMap<String, Id>,
    ) -> Result<()> {
        let i = self.intrinsics.as_ref().unwrap();
        let object = i.object;
        let function = i.function;
        let proto = self.new_object(Some(object))?;
        self.runtime.link(env, proto)?;
        objects.insert("Symbol.prototype".into(), proto);
        for method in [
            "for",
            "keyFor",
            "prototype.toString",
            "prototype.valueOf",
            "prototype.description",
            "prototype.toPrimitive",
        ] {
            let name = format!("Symbol.{method}");
            let f = self.intrinsic_function(env, &name, Some(function))?;
            self.intrinsics.as_mut().unwrap().functions.insert(name, f);
            match method {
                "for" | "keyFor" => self.hidden(objects["Symbol"], method, Value::Object(f))?,
                "prototype.description" => {
                    self.define(
                        proto,
                        "description",
                        Descriptor {
                            get: Some(Value::Object(f)),
                            configurable: Some(true),
                            ..Default::default()
                        },
                    )?;
                }
                "prototype.toPrimitive" => {
                    self.define(
                        proto,
                        Key::Symbol(PRIMITIVE),
                        Descriptor {
                            value: Some(Value::Object(f)),
                            configurable: Some(true),
                            ..Default::default()
                        },
                    )?;
                }
                _ => self.hidden(proto, method.rsplit('.').next().unwrap(), Value::Object(f))?,
            }
        }
        self.define(
            proto,
            Key::Symbol(1),
            Descriptor {
                value: Some(Value::String("Symbol".into())),
                configurable: Some(true),
                ..Default::default()
            },
        )?;
        for (id, name) in WELL_KNOWN.iter().enumerate() {
            self.define(
                objects["Symbol"],
                *name,
                Descriptor {
                    value: Some(Value::Symbol(id as u64)),
                    ..Default::default()
                },
            )?;
        }
        let instance =
            self.intrinsic_function(env, "Function.prototype.hasInstance", Some(function))?;
        self.define(
            function,
            Key::Symbol(HAS_INSTANCE),
            Descriptor {
                value: Some(Value::Object(instance)),
                ..Default::default()
            },
        )?;
        Ok(())
    }
    pub(super) fn symbol_builtin(
        &mut self,
        name: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let arg = args.first().cloned().unwrap_or(Value::Undefined);
        match name {
            "Symbol" => {
                let description = if arg == Value::Undefined {
                    None
                } else {
                    Some(self.to_string(arg)?)
                };
                self.new_symbol(description)
            }
            "Symbol.for" => {
                let key = self.to_string(arg)?;
                if let Some(id) = self.symbol_registry.get(&key) {
                    return Ok(Value::Symbol(*id));
                }
                let value = self.new_symbol(Some(key.clone()))?;
                let Value::Symbol(id) = value else {
                    unreachable!()
                };
                self.symbol_registry.insert(key, id);
                Ok(value)
            }
            "Symbol.keyFor" => {
                let Value::Symbol(id) = arg else {
                    return self.throw_error("TypeError", "Symbol.keyFor requires a symbol");
                };
                Ok(self
                    .symbol_registry
                    .iter()
                    .find(|(_, symbol)| **symbol == id)
                    .map(|(key, _)| Value::String(key.clone()))
                    .unwrap_or(Value::Undefined))
            }
            "Symbol.prototype.toString" => {
                let id = self.this_symbol(receiver)?;
                Ok(Value::String(self.symbol_string(id)))
            }
            "Symbol.prototype.valueOf" | "Symbol.prototype.toPrimitive" => {
                self.this_symbol(receiver).map(Value::Symbol)
            }
            "Symbol.prototype.description" => {
                let id = self.this_symbol(receiver)?;
                Ok(self
                    .symbol_description(id)
                    .map(Value::String)
                    .unwrap_or(Value::Undefined))
            }
            _ => Err(ScriptError::Unsupported("symbol intrinsic")),
        }
    }
}
