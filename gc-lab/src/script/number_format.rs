//! Number.prototype.toFixed / toExponential / toPrecision (ECMA-262 §21.1.3.3,
//! §21.1.3.2, §21.1.3.5). Digits come from exact integer arithmetic on the
//! double's mantissa and exponent, so "the n closest to x × 10^k, the larger
//! on a tie" is computed exactly rather than through float rounding.
use super::{
    bigint::Big,
    vm::{Engine, ScriptError, Value},
};
type Result<T> = std::result::Result<T, ScriptError>;

fn pow10(k: u32) -> Big {
    let mut r = Big::from_u64(1);
    let ten = Big::from_u64(10);
    for _ in 0..k {
        r = r.mul(&ten);
    }
    r
}

/// x as an exact fraction num / den, for a finite x > 0.
fn rational(x: f64) -> (Big, Big) {
    let bits = x.to_bits();
    let exp = ((bits >> 52) & 0x7ff) as i32;
    let frac = bits & ((1u64 << 52) - 1);
    let (mantissa, e2) = if exp == 0 {
        (frac, -1074)
    } else {
        (frac | (1u64 << 52), exp - 1075)
    };
    let one = Big::from_u64(1);
    if e2 >= 0 {
        let num = Big::from_u64(mantissa).shift_left(e2 as usize);
        (num.expect("bounded shift"), one)
    } else {
        let den = one.shift_left((-e2) as usize).expect("bounded shift");
        (Big::from_u64(mantissa), den)
    }
}

/// The integer n for which n − x × 10^k is closest to zero (the larger n on a
/// tie), for a finite x > 0.
fn scaled_round(x: f64, k: i32) -> Big {
    let (mut num, mut den) = rational(x);
    if k >= 0 {
        num = num.mul(&pow10(k as u32));
    } else {
        den = den.mul(&pow10((-k) as u32));
    }
    // floor((2·num + den) / (2·den)) rounds half up.
    let two = Big::from_u64(2);
    let top = num.mul(&two).add(&den);
    let (q, _) = top.div_rem(&den.mul(&two)).expect("non-zero divisor");
    q
}

/// The exact floor(log10(x)) for a finite x > 0.
fn decimal_exponent(x: f64) -> i32 {
    let (num, den) = rational(x);
    // Is 10^e ≤ x?
    let at_least = |e: i32| {
        if e >= 0 {
            num.compare(&den.mul(&pow10(e as u32))) != std::cmp::Ordering::Less
        } else {
            num.mul(&pow10((-e) as u32)).compare(&den) != std::cmp::Ordering::Less
        }
    };
    let mut e = x.log10().floor() as i32;
    while !at_least(e) {
        e -= 1;
    }
    while at_least(e + 1) {
        e += 1;
    }
    e
}

/// n and e with 10^(p−1) ≤ n < 10^p and n × 10^(e−p+1) closest to x (x > 0).
fn precision_digits(x: f64, p: u32) -> (String, i32) {
    let e = decimal_exponent(x);
    let digits = scaled_round(x, p as i32 - 1 - e).radix_string(10);
    if digits.len() as u32 > p {
        // Rounding carried into a new decade: n = 10^(p−1), one decade up.
        (format!("1{}", "0".repeat(p as usize - 1)), e + 1)
    } else {
        (digits, e)
    }
}

/// The shortest round-trip digits of x > 0 and its decimal exponent
/// (the n, e, f of Number::toString, §6.1.6.1.20).
fn shortest_digits(x: f64) -> (String, i32) {
    let s = format!("{x:e}");
    let (mantissa, exp) = s.split_once('e').unwrap();
    (mantissa.replace('.', ""), exp.parse().unwrap())
}

fn exponent_suffix(e: i32) -> String {
    if e >= 0 {
        format!("e+{e}")
    } else {
        format!("e-{}", -e)
    }
}

fn with_point(digits: &str) -> String {
    if digits.len() == 1 {
        digits.to_string()
    } else {
        format!("{}.{}", &digits[..1], &digits[1..])
    }
}

/// Number::toString(x, radix) for radix ≠ 10 (§6.1.6.1.20 step 4: an
/// implementation-approximated generalisation). The fraction is emitted until
/// the remaining digits cannot change which double is denoted (the
/// shortest-round-trip rule), rounding the last digit half to even.
fn radix_string(x: f64, radix: u32) -> String {
    if x == 0.0 {
        return "0".into();
    }
    let sign = if x < 0.0 { "-" } else { "" };
    let x = x.abs();
    let mut integer = x.trunc();
    let mut fraction = x - integer;
    let next = f64::from_bits(x.to_bits() + 1);
    let mut delta = (0.5 * (next - x)).max(f64::from_bits(1));
    let r = radix as f64;
    let mut digits: Vec<u32> = Vec::new();
    if fraction >= delta {
        loop {
            fraction *= r;
            delta *= r;
            let digit = fraction.floor();
            digits.push(digit as u32);
            fraction -= digit;
            if (fraction > 0.5 || (fraction == 0.5 && (digit as u32) & 1 == 1))
                && fraction + delta > 1.0
            {
                // Round up, propagating carries into the integer part.
                loop {
                    match digits.pop() {
                        None => {
                            integer += 1.0;
                            break;
                        }
                        Some(d) if d + 1 < radix => {
                            digits.push(d + 1);
                            break;
                        }
                        Some(_) => {}
                    }
                }
                break;
            }
            if fraction < delta {
                break;
            }
        }
    }
    let int_digits = Big::from_float(integer)
        .map(|b| b.radix_string(radix))
        .unwrap_or_else(|| "0".into());
    let mut out = format!("{sign}{int_digits}");
    if !digits.is_empty() {
        out.push('.');
        out.extend(
            digits
                .iter()
                .map(|d| std::char::from_digit(*d, radix).unwrap()),
        );
    }
    out
}

impl Engine<'_> {
    pub(super) fn number_format(&mut self, method: &str, x: f64, arg: Value) -> Result<Value> {
        let text = match method {
            // §21.1.3.4 Number.prototype.toLocaleString without ECMA-402: the
            // implementation-defined form is Number::toString(x, 10).
            "toLocaleString" => return self.to_string(Value::number(x)).map(Value::String),
            // §21.1.3.6 Number.prototype.toString
            "toString" => {
                let radix = if matches!(arg, Value::Undefined) {
                    10.0
                } else {
                    self.integer(arg)?
                };
                if !(2.0..=36.0).contains(&radix) {
                    return self
                        .throw_error("RangeError", "toString() radix must be between 2 and 36");
                }
                if radix == 10.0 || !x.is_finite() {
                    return self.to_string(Value::number(x)).map(Value::String);
                }
                radix_string(x, radix as u32)
            }
            // §21.1.3.3 Number.prototype.toFixed
            "toFixed" => {
                let f = self.integer(arg)?;
                if !f.is_finite() || !(0.0..=100.0).contains(&f) {
                    return self.throw_error(
                        "RangeError",
                        "toFixed() digits argument must be between 0 and 100",
                    );
                }
                if !x.is_finite() {
                    return self.to_string(Value::number(x)).map(Value::String);
                }
                let f = f as usize;
                let sign = if x < 0.0 { "-" } else { "" };
                let x = x.abs();
                let m = if x >= 1e21 {
                    self.to_string(Value::number(x))?.to_string()
                } else {
                    let mut m = if x == 0.0 {
                        "0".to_string()
                    } else {
                        scaled_round(x, f as i32).radix_string(10)
                    };
                    if f != 0 {
                        let mut k = m.len();
                        if k <= f {
                            m = "0".repeat(f + 1 - k) + &m;
                            k = f + 1;
                        }
                        m = format!("{}.{}", &m[..k - f], &m[k - f..]);
                    }
                    m
                };
                format!("{sign}{m}")
            }
            // §21.1.3.2 Number.prototype.toExponential
            "toExponential" => {
                let undefined = matches!(arg, Value::Undefined);
                let f = self.integer(arg)?;
                if !x.is_finite() {
                    return self.to_string(Value::number(x)).map(Value::String);
                }
                if !f.is_finite() || !(0.0..=100.0).contains(&f) {
                    return self.throw_error(
                        "RangeError",
                        "toExponential() argument must be between 0 and 100",
                    );
                }
                let sign = if x < 0.0 { "-" } else { "" };
                let x = x.abs();
                let (digits, e) = if x == 0.0 {
                    ("0".repeat(f as usize + 1), 0)
                } else if undefined {
                    shortest_digits(x)
                } else {
                    precision_digits(x, f as u32 + 1)
                };
                format!("{sign}{}{}", with_point(&digits), exponent_suffix(e))
            }
            // §21.1.3.5 Number.prototype.toPrecision
            _ => {
                if matches!(arg, Value::Undefined) {
                    return self.to_string(Value::number(x)).map(Value::String);
                }
                let p = self.integer(arg)?;
                if !x.is_finite() {
                    return self.to_string(Value::number(x)).map(Value::String);
                }
                if !p.is_finite() || !(1.0..=100.0).contains(&p) {
                    return self.throw_error(
                        "RangeError",
                        "toPrecision() argument must be between 1 and 100",
                    );
                }
                let p = p as u32;
                let sign = if x < 0.0 { "-" } else { "" };
                let x = x.abs();
                let (m, e) = if x == 0.0 {
                    ("0".repeat(p as usize), 0)
                } else {
                    precision_digits(x, p)
                };
                let m = if x != 0.0 && (e < -6 || e >= p as i32) {
                    format!("{}{}", with_point(&m), exponent_suffix(e))
                } else if e == p as i32 - 1 {
                    m
                } else if e >= 0 {
                    let cut = e as usize + 1;
                    format!("{}.{}", &m[..cut], &m[cut..])
                } else {
                    format!("0.{}{m}", "0".repeat((-(e + 1)) as usize))
                };
                format!("{sign}{m}")
            }
        };
        Ok(Value::String(text.into()))
    }
}
