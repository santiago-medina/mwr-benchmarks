//! BigInt coercions and effects stay at the VM boundary; limb values contain no GC references.
use super::{
    bigint::{Big, ParseError, MAX_BITS},
    objects::{Descriptor, Key},
    vm::{Engine, ScriptError, Value},
};
use crate::dual::Id;
use std::{cmp::Ordering, collections::HashMap, rc::Rc};
type Result<T> = std::result::Result<T, ScriptError>;
impl Engine<'_> {
    fn big_charge(&mut self, work: usize) -> Result<()> {
        for _ in 0..work.saturating_add(63) / 64 {
            self.tick()?;
        }
        Ok(())
    }
    pub(super) fn big_value(&mut self, value: Big) -> Result<Value> {
        if value.bits() > MAX_BITS {
            return self.throw_error("RangeError", "host BigInt size limit");
        }
        Ok(Value::BigInt(Rc::new(value)))
    }
    pub(super) fn big_parse(&mut self, text: &str) -> Result<Option<Big>> {
        let text = text.trim_matches(super::strings::whitespace);
        self.big_charge(text.len().saturating_mul(text.len()) / 16 + 1)?;
        match Big::parse(text) {
            Ok(value) => Ok(Some(value)),
            Err(ParseError::Syntax) => Ok(None),
            Err(ParseError::Limit) => self.throw_error("RangeError", "host BigInt size limit"),
        }
    }
    pub(super) fn big_literal(&mut self, text: &str) -> Result<Value> {
        match self.big_parse(text)? {
            Some(value) => self.big_value(value),
            None => self.throw_error("SyntaxError", "invalid BigInt literal"),
        }
    }
    pub(super) fn to_bigint(&mut self, value: Value, allow_number: bool) -> Result<Big> {
        let value = self.to_primitive(value, false)?;
        match value {
            Value::BigInt(n) => Ok((*n).clone()),
            Value::Bool(b) => Ok(Big::from_u64(u64::from(b))),
            Value::Number(bits) if allow_number => match Big::from_float(f64::from_bits(bits)) {
                Some(value) => Ok(value),
                None => self.throw_error("RangeError", "number is not a finite integer"),
            },
            Value::String(text) => match self.big_parse(&text.lossy())? {
                Some(value) => Ok(value),
                None => self.throw_error("SyntaxError", "invalid BigInt string"),
            },
            _ => self.throw_error("TypeError", "cannot convert value to BigInt"),
        }
    }
    pub(super) fn big_string(&mut self, value: &Big, radix: u32) -> Result<String> {
        self.big_charge(value.limbs.len().saturating_mul(value.bits()) + 1)?;
        Ok(value.radix_string(radix))
    }
    pub(super) fn big_binary(&mut self, op: &str, a: Rc<Big>, b: Rc<Big>) -> Result<Value> {
        let work = if matches!(op, "*" | "/" | "%") {
            a.bits().saturating_mul(b.limbs.len() + 1)
        } else {
            a.limbs.len() + b.limbs.len() + 1
        };
        self.big_charge(work)?;
        let value = match op {
            "+" => a.add(&b),
            "-" => a.sub(&b),
            "*" => a.mul(&b),
            "/" | "%" => {
                let Some((q, r)) = a.div_rem(&b) else {
                    return self.throw_error("RangeError", "BigInt division by zero");
                };
                if op == "/" {
                    q
                } else {
                    r
                }
            }
            "&" | "|" | "^" => a.bitwise(&b, op),
            ">>>" => return self.throw_error("TypeError", "BigInt has no unsigned right shift"),
            "<<" | ">>" => {
                let right = (op == ">>") != b.negative;
                let mut count = (*b).clone();
                count.negative = false;
                let count = count.to_u64();
                if count.is_none_or(|n| n > MAX_BITS as u64) {
                    if right {
                        if a.negative {
                            Big::from_u64(1).neg()
                        } else {
                            Big::zero()
                        }
                    } else if a.is_zero() {
                        Big::zero()
                    } else {
                        return self.throw_error("RangeError", "host BigInt shift limit");
                    }
                } else {
                    let count = count.unwrap() as usize;
                    if right {
                        a.shift_right(count, true)
                    } else {
                        match a.shift_left(count) {
                            Some(value) => value,
                            None => {
                                return self.throw_error("RangeError", "host BigInt shift limit")
                            }
                        }
                    }
                }
            }
            "**" => {
                if b.negative {
                    return self.throw_error("RangeError", "negative BigInt exponent");
                }
                if b.is_zero() {
                    Big::from_u64(1)
                } else if a.is_zero() {
                    Big::zero()
                } else if a.bits() == 1 {
                    if a.negative && b.bit(0) {
                        Big::from_u64(1).neg()
                    } else {
                        Big::from_u64(1)
                    }
                } else {
                    let Some(mut exponent) = b.to_u64().filter(|n| *n <= MAX_BITS as u64) else {
                        return self.throw_error("RangeError", "host BigInt exponent limit");
                    };
                    let mut base = (*a).clone();
                    let mut value = Big::from_u64(1);
                    while exponent != 0 {
                        if exponent & 1 != 0 {
                            self.big_charge(
                                value.limbs.len().saturating_mul(base.limbs.len()) + 1,
                            )?;
                            value = value.mul(&base);
                            if value.bits() > MAX_BITS {
                                return self.throw_error("RangeError", "host BigInt size limit");
                            }
                        }
                        exponent >>= 1;
                        if exponent != 0 {
                            self.big_charge(base.limbs.len().saturating_mul(base.limbs.len()) + 1)?;
                            base = base.mul(&base);
                            if base.bits() > MAX_BITS {
                                return self.throw_error("RangeError", "host BigInt size limit");
                            }
                        }
                    }
                    value
                }
            }
            _ => return Err(ScriptError::Unsupported("BigInt operator")),
        };
        self.big_value(value)
    }
    fn compare_big_float(big: &Big, number: f64) -> Option<Ordering> {
        if number.is_nan() {
            return None;
        }
        if number == f64::INFINITY {
            return Some(Ordering::Less);
        }
        if number == f64::NEG_INFINITY {
            return Some(Ordering::Greater);
        }
        let integer = Big::from_float(number.trunc()).unwrap();
        let order = big.compare(&integer);
        Some(if order == Ordering::Equal && number.fract() != 0.0 {
            if number > 0.0 {
                Ordering::Less
            } else {
                Ordering::Greater
            }
        } else {
            order
        })
    }
    pub(super) fn big_order(&mut self, a: Value, b: Value) -> Result<Option<Ordering>> {
        match (a, b) {
            (Value::BigInt(a), Value::BigInt(b)) => Ok(Some(a.compare(&b))),
            (Value::BigInt(a), Value::String(b)) => {
                Ok(self.big_parse(&b.lossy())?.map(|b| a.compare(&b)))
            }
            (Value::String(a), Value::BigInt(b)) => {
                Ok(self.big_parse(&a.lossy())?.map(|a| a.compare(&b)))
            }
            (Value::BigInt(a), b) => Ok(Self::compare_big_float(&a, self.to_number(b)?)),
            (a, Value::BigInt(b)) => {
                Ok(Self::compare_big_float(&b, self.to_number(a)?).map(Ordering::reverse))
            }
            _ => unreachable!(),
        }
    }
    pub(super) fn abstract_equal(&mut self, a: Value, b: Value) -> Result<bool> {
        if a.strict_equal(&b)
            || matches!(
                (&a, &b),
                (Value::Null, Value::Undefined) | (Value::Undefined, Value::Null)
            )
        {
            return Ok(true);
        }
        match (a, b) {
            (Value::BigInt(a), Value::String(b)) | (Value::String(b), Value::BigInt(a)) => {
                Ok(self.big_parse(&b.lossy())?.is_some_and(|b| *a == b))
            }
            (Value::BigInt(a), Value::Number(b)) | (Value::Number(b), Value::BigInt(a)) => {
                Ok(Big::from_float(f64::from_bits(b)).is_some_and(|b| *a == b))
            }
            (Value::Number(a), Value::String(b)) | (Value::String(b), Value::Number(a)) => {
                Ok(f64::from_bits(a) == self.to_number(Value::String(b))?)
            }
            (Value::Bool(a), b) => self.abstract_equal(Value::number(if a { 1.0 } else { 0.0 }), b),
            (a, Value::Bool(b)) => self.abstract_equal(a, Value::number(if b { 1.0 } else { 0.0 })),
            (
                a @ Value::Object(_),
                b @ (Value::String(_) | Value::Number(_) | Value::BigInt(_) | Value::Symbol(_)),
            ) => {
                let a = self.to_primitive_hint(a, "default")?;
                self.protect(&a)?;
                self.abstract_equal(a, b)
            }
            (
                a @ (Value::String(_) | Value::Number(_) | Value::BigInt(_) | Value::Symbol(_)),
                b @ Value::Object(_),
            ) => {
                let b = self.to_primitive_hint(b, "default")?;
                self.protect(&b)?;
                self.abstract_equal(a, b)
            }
            _ => Ok(false),
        }
    }
    pub(super) fn bootstrap_bigint(
        &mut self,
        env: Id,
        objects: &mut HashMap<String, Id>,
    ) -> Result<()> {
        let i = self.intrinsics.as_ref().unwrap();
        let object = i.object;
        let function = i.function;
        let proto = self.new_object(Some(object))?;
        self.runtime.link(env, proto)?;
        objects.insert("BigInt.prototype".into(), proto);
        let mut data = self.data(proto)?.clone();
        data.boxed = Some(Value::BigInt(Rc::new(Big::zero())));
        self.replace_data(proto, data)?;
        self.define(
            proto,
            Key::Symbol(1),
            Descriptor {
                value: Some(Value::String("BigInt".into())),
                configurable: Some(true),
                ..Default::default()
            },
        )?;
        for method in [
            "asUintN",
            "asIntN",
            "prototype.toString",
            "prototype.valueOf",
        ] {
            let name = format!("BigInt.{method}");
            let f = self.intrinsic_function(env, &name, Some(function))?;
            self.intrinsics.as_mut().unwrap().functions.insert(name, f);
            self.hidden(
                if method.starts_with("prototype.") {
                    proto
                } else {
                    objects["BigInt"]
                },
                method.rsplit('.').next().unwrap(),
                Value::Object(f),
            )?;
        }
        Ok(())
    }
    pub(super) fn bigint_builtin(
        &mut self,
        name: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let arg = |i| args.get(i).cloned().unwrap_or(Value::Undefined);
        if name == "BigInt" {
            let value = self.to_bigint(arg(0), true)?;
            return self.big_value(value);
        }
        if matches!(name, "BigInt.asIntN" | "BigInt.asUintN") {
            let width = self.integer(arg(0))?;
            if !(0.0..=9007199254740991.0).contains(&width) {
                return self.throw_error("RangeError", "invalid BigInt bit width");
            }
            let value = self.to_bigint(arg(1), false)?;
            if width > MAX_BITS as f64 {
                if name == "BigInt.asIntN" || !value.negative {
                    return self.big_value(value);
                }
                return self.throw_error("RangeError", "host BigInt width limit");
            }
            self.big_charge(value.limbs.len() + width as usize / 32 + 1)?;
            return self.big_value(value.truncate(width as usize, name == "BigInt.asIntN"));
        }
        let value = if let Value::Object(id) = receiver {
            self.data(id)
                .ok()
                .and_then(|d| d.boxed.clone())
                .unwrap_or(Value::Undefined)
        } else {
            receiver
        };
        let Value::BigInt(value) = value else {
            return self.throw_error("TypeError", "invalid BigInt receiver");
        };
        if name == "BigInt.prototype.valueOf" {
            return Ok(Value::BigInt(value));
        }
        let radix = if arg(0) == Value::Undefined {
            10.0
        } else {
            self.integer(arg(0))?
        };
        if !(2.0..=36.0).contains(&radix) {
            return self.throw_error("RangeError", "invalid BigInt radix");
        }
        self.big_string(&value, radix as u32)
            .map(|s| Value::String(s.into()))
    }
}
