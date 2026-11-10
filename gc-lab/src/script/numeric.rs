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
    "abs",
    "acos",
    "acosh",
    "asin",
    "asinh",
    "atan",
    "atanh",
    "atan2",
    "cbrt",
    "ceil",
    "clz32",
    "cos",
    "cosh",
    "exp",
    "expm1",
    "floor",
    "fround",
    "f16round",
    "hypot",
    "imul",
    "log",
    "log1p",
    "log2",
    "log10",
    "max",
    "min",
    "pow",
    "random",
    "round",
    "sign",
    "sin",
    "sinh",
    "sqrt",
    "tan",
    "tanh",
    "trunc",
    "sumPrecise",
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
pub(super) fn f16(n: f64) -> f64 {
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
                let s = self.to_js_string(arg(0))?;
                return Ok(Value::number(parse_float(&s.scalar_prefix())));
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
            "Math.sumPrecise" => return self.sum_precise(arg(0)),
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

/// The exact sum of finite Numbers for Math.sumPrecise: a two's complement fixed-point integer
/// in units of 2^-1074 (the smallest subnormal). Every finite double is an integer multiple of
/// that unit below 2^2098, and at most 2^53 addends fit in 2^(2098+53+1) bits, so 36 limbs hold
/// any reachable sum exactly; the only rounding is the final one to a Number.
pub(super) struct ExactSum {
    limbs: [u64; 36],
}
impl ExactSum {
    pub(super) fn new() -> Self {
        Self { limbs: [0; 36] }
    }
    /// Adds a finite double exactly.
    pub(super) fn add(&mut self, x: f64) {
        let bits = x.to_bits();
        let exponent = ((bits >> 52) & 0x7ff) as usize;
        let fraction = bits & ((1 << 52) - 1);
        let (mantissa, shift) = if exponent == 0 {
            (fraction, 0)
        } else {
            (fraction | 1 << 52, exponent - 1)
        };
        if mantissa == 0 {
            return;
        }
        // x = mantissa * 2^(shift - 1074): place mantissa << shift as a 128-bit window.
        let (limb, offset) = (shift / 64, shift % 64);
        let wide = (mantissa as u128) << offset;
        let parts = [wide as u64, (wide >> 64) as u64];
        if bits >> 63 == 0 {
            let mut carry = false;
            for (i, part) in parts.iter().enumerate() {
                let (a, c1) = self.limbs[limb + i].overflowing_add(*part);
                let (b, c2) = a.overflowing_add(carry as u64);
                self.limbs[limb + i] = b;
                carry = c1 || c2;
            }
            let mut i = limb + 2;
            while carry && i < self.limbs.len() {
                let (a, c) = self.limbs[i].overflowing_add(1);
                self.limbs[i] = a;
                carry = c;
                i += 1;
            }
        } else {
            let mut borrow = false;
            for (i, part) in parts.iter().enumerate() {
                let (a, b1) = self.limbs[limb + i].overflowing_sub(*part);
                let (b, b2) = a.overflowing_sub(borrow as u64);
                self.limbs[limb + i] = b;
                borrow = b1 || b2;
            }
            let mut i = limb + 2;
            while borrow && i < self.limbs.len() {
                let (a, b) = self.limbs[i].overflowing_sub(1);
                self.limbs[i] = a;
                borrow = b;
                i += 1;
            }
        }
    }
    /// The sum rounded to the nearest Number, ties to even (§6.1.6.1 "the Number value for").
    /// A zero sum is +0 (the caller handles the all-minus-zero case).
    pub(super) fn round(&self) -> f64 {
        let mut magnitude = self.limbs;
        let negative = magnitude[35] >> 63 == 1;
        if negative {
            let mut carry = true;
            for limb in magnitude.iter_mut() {
                let (a, c) = (!*limb).overflowing_add(carry as u64);
                *limb = a;
                carry = c;
            }
        }
        let Some(top) = (0..36).rev().find(|&i| magnitude[i] != 0) else {
            return 0.0;
        };
        let bit_length = top * 64 + 64 - magnitude[top].leading_zeros() as usize;
        let bit = |i: usize| (magnitude[i / 64] >> (i % 64)) & 1;
        let result = if bit_length <= 53 {
            // Exactly representable: below 2^53 units is a subnormal or the smallest binade.
            (magnitude[0] as f64) * f64::from_bits(1)
        } else {
            let shift = bit_length - 53;
            let mut mantissa = 0u64;
            for i in (shift..bit_length).rev() {
                mantissa = mantissa << 1 | bit(i);
            }
            let guard = bit(shift - 1) == 1;
            let sticky = (0..shift - 1).any(|i| bit(i) == 1);
            let mut exponent = shift as i32 - 1074;
            if guard && (sticky || mantissa & 1 == 1) {
                mantissa += 1;
                if mantissa == 1 << 53 {
                    mantissa >>= 1;
                    exponent += 1;
                }
            }
            // mantissa * 2^exponent with exponent >= -1021: normal, so only overflow rounds.
            let mut value = mantissa as f64;
            let mut e = exponent;
            while e > 1000 {
                value *= 2f64.powi(1000);
                e -= 1000;
            }
            value * 2f64.powi(e)
        };
        if negative {
            -result
        } else {
            result
        }
    }
}
impl Engine<'_> {
    /// Math.sumPrecise ( items ) (ES2026 §21.3.2.34).
    fn sum_precise(&mut self, items: Value) -> Result<Value> {
        // 1. Perform ? RequireObjectCoercible(items).
        if matches!(items, Value::Null | Value::Undefined) {
            return self.throw_error("TypeError", "Math.sumPrecise called on null or undefined");
        }
        #[derive(PartialEq)]
        enum State {
            MinusZero,
            Finite,
            PlusInfinity,
            MinusInfinity,
            NotANumber,
        }
        let mut state = State::MinusZero;
        let mut sum = ExactSum::new();
        let mut count: u64 = 0;
        // 2-5. GetIterator(items, sync), then IteratorStepValue until done; an abrupt
        // completion from the loop body closes the iterator.
        self.consume_iterable(items, |e, value| {
            count += 1;
            if count >= 1 << 53 {
                return e.throw_error("RangeError", "too many values for Math.sumPrecise");
            }
            let Value::Number(bits) = value else {
                return e.throw_error("TypeError", "Math.sumPrecise requires Number values");
            };
            let n = f64::from_bits(bits);
            if state != State::NotANumber {
                if n.is_nan() {
                    state = State::NotANumber;
                } else if n == f64::INFINITY {
                    state = if state == State::MinusInfinity {
                        State::NotANumber
                    } else {
                        State::PlusInfinity
                    };
                } else if n == f64::NEG_INFINITY {
                    state = if state == State::PlusInfinity {
                        State::NotANumber
                    } else {
                        State::MinusInfinity
                    };
                } else if !(n == 0.0 && n.is_sign_negative())
                    && matches!(state, State::MinusZero | State::Finite)
                {
                    state = State::Finite;
                    sum.add(n);
                }
            }
            Ok(())
        })?;
        // 6-10.
        Ok(Value::number(match state {
            State::NotANumber => f64::NAN,
            State::PlusInfinity => f64::INFINITY,
            State::MinusInfinity => f64::NEG_INFINITY,
            State::MinusZero => -0.0,
            State::Finite => sum.round(),
        }))
    }
}
