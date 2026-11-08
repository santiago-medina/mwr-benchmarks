//! Shared ECMAScript numeric conversions; no object coercion in this layer.
pub(super) fn uint32(n: f64) -> u32 {
    if !n.is_finite() || n == 0.0 {
        return 0;
    }
    n.trunc().rem_euclid(4294967296.0) as u32
}
pub(super) fn pow(base: f64, exponent: f64) -> f64 {
    if exponent.is_nan() {
        f64::NAN
    } else if exponent == 0.0 {
        1.0
    } else if base.abs() == 1.0 && exponent.is_infinite() {
        f64::NAN
    } else {
        base.powf(exponent)
    }
}

use super::{
    objects::{Descriptor, Key},
    vm::{Engine, ScriptError, Value},
};
use crate::dual::Id;
use std::{
    cell::Cell,
    collections::{hash_map::RandomState, HashMap},
    hash::BuildHasher,
};
type Result<T> = std::result::Result<T, ScriptError>;
pub(super) const MATH_METHODS: &[&str] = &[
    "abs", "acos", "acosh", "asin", "asinh", "atan", "atanh", "atan2", "cbrt", "ceil", "clz32",
    "cos", "cosh", "exp", "expm1", "floor", "fround", "f16round", "hypot", "imul", "log", "log1p",
    "log2", "log10", "max", "min", "pow", "random", "round", "sign", "sin", "sinh", "sqrt", "tan",
    "tanh", "trunc",
];
thread_local! {static RANDOM: (RandomState,Cell<u64>) = (RandomState::new(),Cell::new(0));}
fn random() -> f64 {
    RANDOM.with(|(hash, counter)| {
        let n = counter.get();
        counter.set(n.wrapping_add(1));
        (hash.hash_one(n) >> 11) as f64 / 9007199254740992.0
    })
}
fn round(n: f64) -> f64 {
    if !n.is_finite() || n == 0.0 || n.abs() >= 4503599627370496.0 {
        return n;
    }
    let floor = n.floor();
    let out = if n - floor >= 0.5 { floor + 1.0 } else { floor };
    if out == 0.0 {
        0.0f64.copysign(n)
    } else {
        out
    }
}
fn f16(n: f64) -> f64 {
    if n.is_nan() || n == 0.0 {
        return n;
    }
    let a = n.abs();
    if a >= 65520.0 {
        return f64::INFINITY.copysign(n);
    }
    let step = if a < 0.00006103515625 {
        2f64.powi(-24)
    } else {
        2f64.powi(a.log2().floor() as i32 - 10)
    };
    ((a / step).round_ties_even() * step).copysign(n)
}
fn parse_float(s: &str) -> f64 {
    let s = s.trim_start_matches(|c: char| c == '\u{feff}' || c != '\u{85}' && c.is_whitespace());
    let bytes = s.as_bytes();
    let mut i = 0;
    if bytes.first().is_some_and(|c| matches!(c, b'+' | b'-')) {
        i += 1;
    }
    if s[i..].starts_with("Infinity") {
        return if bytes.first() == Some(&b'-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }
    let mut digits = 0;
    while bytes.get(i).is_some_and(u8::is_ascii_digit) {
        i += 1;
        digits += 1;
    }
    if bytes.get(i) == Some(&b'.') {
        i += 1;
        while bytes.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return f64::NAN;
    }
    let end = i;
    if bytes.get(i).is_some_and(|c| matches!(c, b'e' | b'E')) {
        i += 1;
        if bytes.get(i).is_some_and(|c| matches!(c, b'+' | b'-')) {
            i += 1;
        }
        let start = i;
        while bytes.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if start == i {
            i = end;
        }
    }
    s[..i].parse().unwrap_or(f64::NAN)
}
impl Engine<'_> {
    pub(super) fn bootstrap_math(
        &mut self,
        env: Id,
        objects: &mut HashMap<String, Id>,
    ) -> Result<()> {
        let intrinsics = self.intrinsics.as_ref().unwrap();
        let object = intrinsics.object;
        let function = intrinsics.function;
        let math = self.new_object(Some(object))?;
        self.runtime.link(env, math)?;
        objects.insert("Math".into(), math);
        self.define(
            math,
            Key::Symbol(1),
            Descriptor {
                value: Some(Value::String("Math".into())),
                configurable: Some(true),
                ..Default::default()
            },
        )?;
        for (key, n) in [
            ("E", std::f64::consts::E),
            ("LN10", std::f64::consts::LN_10),
            ("LN2", std::f64::consts::LN_2),
            ("LOG10E", std::f64::consts::LOG10_E),
            ("LOG2E", std::f64::consts::LOG2_E),
            ("PI", std::f64::consts::PI),
            ("SQRT1_2", std::f64::consts::FRAC_1_SQRT_2),
            ("SQRT2", std::f64::consts::SQRT_2),
        ] {
            self.define(
                math,
                key,
                Descriptor {
                    value: Some(Value::number(n)),
                    ..Default::default()
                },
            )?;
        }
        for method in MATH_METHODS {
            let name = format!("Math.{method}");
            let f = self.intrinsic_function(env, &name, Some(function))?;
            self.intrinsics.as_mut().unwrap().functions.insert(name, f);
            self.hidden(math, *method, Value::Object(f))?;
        }
        for method in ["parseInt", "parseFloat"] {
            let f = self.intrinsics.as_ref().unwrap().functions[method];
            self.hidden(objects["Number"], method, Value::Object(f))?;
        }
        Ok(())
    }
    pub(super) fn numeric_builtin(&mut self, name: &str, args: Vec<Value>) -> Result<Value> {
        let arg = |i| args.get(i).cloned().unwrap_or(Value::Undefined);
        match name {
            "parseFloat" => {
                let s = self.to_string(arg(0))?;
                return Ok(Value::number(parse_float(&s)));
            }
            "isNaN" => return Ok(Value::Bool(self.to_number(arg(0))?.is_nan())),
            "isFinite" => return Ok(Value::Bool(self.to_number(arg(0))?.is_finite())),
            "Number.isNaN" | "Number.isFinite" | "Number.isInteger" | "Number.isSafeInteger" => {
                let yes = if let Value::Number(bits) = arg(0) {
                    let n = f64::from_bits(bits);
                    match name {
                        "Number.isNaN" => n.is_nan(),
                        "Number.isFinite" => n.is_finite(),
                        "Number.isInteger" => n.is_finite() && n.trunc() == n,
                        _ => n.is_finite() && n.trunc() == n && n.abs() <= 9007199254740991.0,
                    }
                } else {
                    false
                };
                return Ok(Value::Bool(yes));
            }
            "Math.random" => return Ok(Value::number(random())),
            "Math.max" | "Math.min" | "Math.hypot" => {
                let mut numbers = vec![];
                for v in args {
                    numbers.push(self.to_number(v)?);
                }
                let result = if name == "Math.hypot" {
                    if numbers.iter().any(|n| n.is_infinite()) {
                        f64::INFINITY
                    } else if numbers.iter().any(|n| n.is_nan()) {
                        f64::NAN
                    } else {
                        let scale = numbers.iter().map(|n| n.abs()).fold(0.0, f64::max);
                        if scale == 0.0 {
                            0.0
                        } else {
                            let mut sum = 0.0;
                            let mut correction = 0.0;
                            for n in numbers {
                                let term = (n / scale).powi(2) - correction;
                                let next = sum + term;
                                correction = (next - sum) - term;
                                sum = next;
                            }
                            scale * sum.sqrt()
                        }
                    }
                } else if numbers.iter().any(|n| n.is_nan()) {
                    f64::NAN
                } else {
                    let max = name == "Math.max";
                    let mut out = if max {
                        f64::NEG_INFINITY
                    } else {
                        f64::INFINITY
                    };
                    for n in numbers {
                        if (max && n > out)
                            || (!max && n < out)
                            || (n == 0.0
                                && out == 0.0
                                && if max {
                                    n.is_sign_positive()
                                } else {
                                    n.is_sign_negative()
                                })
                        {
                            out = n;
                        }
                    }
                    out
                };
                return Ok(Value::number(result));
            }
            _ => {}
        }
        let x = self.to_number(arg(0))?;
        let out = match name {
            "Math.abs" => x.abs(),
            "Math.acos" => x.acos(),
            "Math.acosh" => x.acosh(),
            "Math.asin" => x.asin(),
            "Math.asinh" => x.asinh(),
            "Math.atan" => x.atan(),
            "Math.atanh" => x.atanh(),
            "Math.atan2" => x.atan2(self.to_number(arg(1))?),
            "Math.cbrt" => x.cbrt(),
            "Math.ceil" => x.ceil(),
            "Math.clz32" => uint32(x).leading_zeros() as f64,
            "Math.cos" => x.cos(),
            "Math.cosh" => x.cosh(),
            "Math.exp" => x.exp(),
            "Math.expm1" => x.exp_m1(),
            "Math.floor" => x.floor(),
            "Math.fround" => (x as f32) as f64,
            "Math.f16round" => f16(x),
            "Math.imul" => (uint32(x).wrapping_mul(uint32(self.to_number(arg(1))?)) as i32) as f64,
            "Math.log" => x.ln(),
            "Math.log1p" => x.ln_1p(),
            "Math.log2" => x.log2(),
            "Math.log10" => x.log10(),
            "Math.pow" => pow(x, self.to_number(arg(1))?),
            "Math.round" => round(x),
            "Math.sign" => {
                if x == 0.0 || x.is_nan() {
                    x
                } else {
                    x.signum()
                }
            }
            "Math.sin" => x.sin(),
            "Math.sinh" => x.sinh(),
            "Math.sqrt" => x.sqrt(),
            "Math.tan" => x.tan(),
            "Math.tanh" => x.tanh(),
            "Math.trunc" => x.trunc(),
            _ => return Err(ScriptError::Unsupported("numeric intrinsic")),
        };
        Ok(Value::number(out))
    }
}

// StringNumericLiteral decimal grammar, distinct from parseFloat's prefix grammar.
pub(super) fn decimal_number(text: &str) -> f64 {
    let bytes = text.as_bytes();
    let mut i = 0;
    if bytes.first().is_some_and(|c| matches!(c, b'+' | b'-')) {
        i += 1;
    }
    if &text[i..] == "Infinity" {
        return if bytes.first() == Some(&b'-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }
    let mut digits = 0;
    while bytes.get(i).is_some_and(u8::is_ascii_digit) {
        i += 1;
        digits += 1;
    }
    if bytes.get(i) == Some(&b'.') {
        i += 1;
        while bytes.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return f64::NAN;
    }
    if bytes.get(i).is_some_and(|c| matches!(c, b'e' | b'E')) {
        i += 1;
        if bytes.get(i).is_some_and(|c| matches!(c, b'+' | b'-')) {
            i += 1;
        }
        let start = i;
        while bytes.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            return f64::NAN;
        }
    }
    if i != bytes.len() {
        f64::NAN
    } else {
        text.parse().unwrap_or(f64::NAN)
    }
}
