//! Immutable signed-magnitude integers; arithmetic is separate from coercion and VM effects.
use std::cmp::Ordering;
pub(super) const MAX_BITS: usize = 65536;
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct Big {
    pub negative: bool,
    pub limbs: Vec<u32>,
}
#[derive(Clone, Copy)]
pub(super) enum ParseError {
    Syntax,
    Limit,
}
impl Big {
    pub fn zero() -> Self {
        Self {
            negative: false,
            limbs: vec![],
        }
    }
    fn normalized(mut self) -> Self {
        while self.limbs.last() == Some(&0) {
            self.limbs.pop();
        }
        if self.limbs.is_empty() {
            self.negative = false;
        }
        self
    }
    pub fn from_u64(n: u64) -> Self {
        Self {
            negative: false,
            limbs: vec![n as u32, (n >> 32) as u32],
        }
        .normalized()
    }
    pub fn is_zero(&self) -> bool {
        self.limbs.is_empty()
    }
    pub fn bits(&self) -> usize {
        self.limbs
            .last()
            .map(|n| (self.limbs.len() - 1) * 32 + 32 - n.leading_zeros() as usize)
            .unwrap_or(0)
    }
    pub fn neg(&self) -> Self {
        let mut n = self.clone();
        if !n.is_zero() {
            n.negative = !n.negative;
        }
        n
    }
    pub fn abs_cmp(&self, other: &Self) -> Ordering {
        self.limbs
            .len()
            .cmp(&other.limbs.len())
            .then_with(|| self.limbs.iter().rev().cmp(other.limbs.iter().rev()))
    }
    pub fn compare(&self, other: &Self) -> Ordering {
        if self.negative != other.negative {
            return self.negative.cmp(&other.negative).reverse();
        }
        let order = self.abs_cmp(other);
        if self.negative {
            order.reverse()
        } else {
            order
        }
    }
    fn abs_add(&self, other: &Self) -> Self {
        let mut limbs = vec![];
        let mut carry = 0u64;
        for i in 0..self.limbs.len().max(other.limbs.len()) {
            let n = self.limbs.get(i).copied().unwrap_or(0) as u64
                + other.limbs.get(i).copied().unwrap_or(0) as u64
                + carry;
            limbs.push(n as u32);
            carry = n >> 32;
        }
        if carry != 0 {
            limbs.push(carry as u32);
        }
        Self {
            negative: false,
            limbs,
        }
    }
    fn abs_sub(&self, other: &Self) -> Self {
        let mut limbs = vec![];
        let mut borrow = 0i64;
        for (i, a) in self.limbs.iter().enumerate() {
            let n = *a as i64 - other.limbs.get(i).copied().unwrap_or(0) as i64 - borrow;
            limbs.push(n as u32);
            borrow = i64::from(n < 0);
        }
        Self {
            negative: false,
            limbs,
        }
        .normalized()
    }
    pub fn add(&self, other: &Self) -> Self {
        let mut out = if self.negative == other.negative {
            self.abs_add(other)
        } else {
            match self.abs_cmp(other) {
                Ordering::Less => {
                    let mut n = other.abs_sub(self);
                    n.negative = other.negative;
                    return n;
                }
                _ => self.abs_sub(other),
            }
        };
        out.negative = self.negative;
        out.normalized()
    }
    pub fn sub(&self, other: &Self) -> Self {
        self.add(&other.neg())
    }
    pub fn mul(&self, other: &Self) -> Self {
        if self.is_zero() || other.is_zero() {
            return Self::zero();
        }
        let mut limbs = vec![0u32; self.limbs.len() + other.limbs.len()];
        for (i, a) in self.limbs.iter().enumerate() {
            let mut carry = 0u64;
            for (j, b) in other.limbs.iter().enumerate() {
                let n = *a as u64 * *b as u64 + limbs[i + j] as u64 + carry;
                limbs[i + j] = n as u32;
                carry = n >> 32;
            }
            limbs[i + other.limbs.len()] = carry as u32;
        }
        Self {
            negative: self.negative != other.negative,
            limbs,
        }
        .normalized()
    }
    pub fn shift_left(&self, count: usize) -> Option<Self> {
        if self.is_zero() {
            return Some(Self::zero());
        }
        if self.bits().checked_add(count)? > MAX_BITS {
            return None;
        }
        let words = count / 32;
        let shift = count % 32;
        let mut limbs = vec![0; words];
        let mut carry = 0u64;
        for n in &self.limbs {
            let n = (*n as u64) << shift | carry;
            limbs.push(n as u32);
            carry = n >> 32;
        }
        if carry != 0 {
            limbs.push(carry as u32);
        }
        Some(Self {
            negative: self.negative,
            limbs,
        })
    }
    fn lost_bits(&self, count: usize) -> bool {
        let words = (count / 32).min(self.limbs.len());
        self.limbs[..words].iter().any(|n| *n != 0)
            || (count % 32 != 0
                && self
                    .limbs
                    .get(words)
                    .is_some_and(|n| n & ((1u32 << (count % 32)) - 1) != 0))
    }
    pub fn shift_right(&self, count: usize, arithmetic: bool) -> Self {
        let words = count / 32;
        let shift = count % 32;
        let mut out = if words >= self.limbs.len() {
            Self::zero()
        } else {
            let mut limbs = vec![];
            for i in words..self.limbs.len() {
                let mut n = self.limbs[i] >> shift;
                if shift != 0 {
                    n |= self.limbs.get(i + 1).copied().unwrap_or(0) << (32 - shift);
                }
                limbs.push(n);
            }
            Self {
                negative: false,
                limbs,
            }
            .normalized()
        };
        if arithmetic && self.negative && self.lost_bits(count) {
            out = out.abs_add(&Self::from_u64(1));
        }
        out.negative = self.negative;
        out.normalized()
    }
    pub fn div_rem(&self, other: &Self) -> Option<(Self, Self)> {
        if other.is_zero() {
            return None;
        }
        let mut remainder = self.clone();
        remainder.negative = false;
        let mut divisor = other.clone();
        divisor.negative = false;
        if remainder.abs_cmp(&divisor) == Ordering::Less {
            return Some((Self::zero(), self.clone()));
        }
        let shift = remainder.bits() - divisor.bits();
        let mut shifted = divisor.shift_left(shift)?;
        let mut quotient = Self {
            negative: self.negative != other.negative,
            limbs: vec![0; shift / 32 + 1],
        };
        for bit in (0..=shift).rev() {
            if remainder.abs_cmp(&shifted) != Ordering::Less {
                remainder = remainder.abs_sub(&shifted);
                quotient.limbs[bit / 32] |= 1 << (bit % 32);
            }
            shifted = shifted.shift_right(1, false);
        }
        remainder.negative = self.negative;
        Some((quotient.normalized(), remainder.normalized()))
    }
    fn twos(&self, length: usize) -> Vec<u32> {
        let mut limbs = self.limbs.clone();
        limbs.resize(length, 0);
        if self.negative {
            let mut carry = 1u64;
            for n in &mut limbs {
                let next = (!*n) as u64 + carry;
                *n = next as u32;
                carry = next >> 32;
            }
        }
        limbs
    }
    fn from_twos(mut limbs: Vec<u32>) -> Self {
        let negative = limbs.last().is_some_and(|n| n & 0x80000000 != 0);
        if negative {
            let mut carry = 1u64;
            for n in &mut limbs {
                let next = (!*n) as u64 + carry;
                *n = next as u32;
                carry = next >> 32;
            }
        }
        Self { negative, limbs }.normalized()
    }
    pub fn bitwise(&self, other: &Self, op: &str) -> Self {
        let length = self.limbs.len().max(other.limbs.len()) + 1;
        Self::from_twos(
            self.twos(length)
                .into_iter()
                .zip(other.twos(length))
                .map(|(a, b)| match op {
                    "&" => a & b,
                    "|" => a | b,
                    _ => a ^ b,
                })
                .collect(),
        )
    }
    pub fn truncate(&self, width: usize, signed: bool) -> Self {
        if width == 0 {
            return Self::zero();
        }
        let mut limbs = self.twos((width + 31) / 32);
        // Truncate before sign extension when the requested width is smaller than the magnitude.
        limbs.truncate((width + 31) / 32);
        if width % 32 != 0 {
            let mask = (1u32 << (width % 32)) - 1;
            let last = limbs.last_mut().unwrap();
            *last &= mask;
            if signed && *last & (1u32 << ((width - 1) % 32)) != 0 {
                *last |= !mask;
            }
        }
        if signed {
            Self::from_twos(limbs)
        } else {
            Self {
                negative: false,
                limbs,
            }
            .normalized()
        }
    }
    pub fn not(&self) -> Self {
        self.add(&Self::from_u64(1)).neg()
    }
    pub fn to_u64(&self) -> Option<u64> {
        if self.negative || self.limbs.len() > 2 {
            return None;
        }
        Some(
            self.limbs.first().copied().unwrap_or(0) as u64
                | ((self.limbs.get(1).copied().unwrap_or(0) as u64) << 32),
        )
    }
    pub fn bit(&self, index: usize) -> bool {
        self.limbs
            .get(index / 32)
            .is_some_and(|n| n & (1 << (index % 32)) != 0)
    }
    pub fn from_float(n: f64) -> Option<Self> {
        if !n.is_finite() || n.trunc() != n {
            return None;
        }
        if n == 0.0 {
            return Some(Self::zero());
        }
        let bits = n.to_bits();
        let exponent = ((bits >> 52) & 2047) as i32 - 1023;
        let mantissa = (bits & ((1u64 << 52) - 1)) | (1u64 << 52);
        let mut out = if exponent >= 52 {
            Self::from_u64(mantissa).shift_left((exponent - 52) as usize)?
        } else {
            Self::from_u64(mantissa >> ((52 - exponent) as u32))
        };
        out.negative = n.is_sign_negative();
        Some(out)
    }
    pub fn to_float(&self) -> f64 {
        let bits = self.bits();
        if bits == 0 {
            return 0.0;
        }
        let n = if bits <= 53 {
            let mut n = self.clone();
            n.negative = false;
            n.to_u64().unwrap() as f64
        } else {
            let shift = bits - 53;
            let mut top = self.shift_right(shift, false);
            top.negative = false;
            let mut top = top.to_u64().unwrap();
            let mut exponent = bits - 1;
            if self.bit(shift - 1) && (self.lost_bits(shift - 1) || top & 1 != 0) {
                top += 1;
                if top == 1u64 << 53 {
                    top >>= 1;
                    exponent += 1;
                }
            }
            if exponent > 1023 {
                f64::INFINITY
            } else {
                f64::from_bits(((exponent as u64 + 1023) << 52) | (top & ((1u64 << 52) - 1)))
            }
        };
        if self.negative {
            -n
        } else {
            n
        }
    }
    pub fn parse(text: &str) -> Result<Self, ParseError> {
        let mut text = text;
        let mut negative = false;
        let mut signed = false;
        if text.starts_with(['+', '-']) {
            signed = true;
            negative = text.starts_with('-');
            text = &text[1..];
            if text.is_empty() {
                return Err(ParseError::Syntax);
            }
        }
        let radix = if text.starts_with("0x") || text.starts_with("0X") {
            16
        } else if text.starts_with("0o") || text.starts_with("0O") {
            8
        } else if text.starts_with("0b") || text.starts_with("0B") {
            2
        } else {
            10
        };
        if radix != 10 {
            if signed {
                return Err(ParseError::Syntax);
            }
            text = &text[2..];
            if text.is_empty() {
                return Err(ParseError::Syntax);
            }
        }
        let mut out = Self::zero();
        for c in text.chars() {
            let digit = c.to_digit(radix).ok_or(ParseError::Syntax)?;
            out = out
                .mul(&Self::from_u64(radix as u64))
                .add(&Self::from_u64(digit as u64));
            if out.bits() > MAX_BITS {
                return Err(ParseError::Limit);
            }
        }
        out.negative = negative;
        Ok(out.normalized())
    }
    pub fn radix_string(&self, radix: u32) -> String {
        if self.is_zero() {
            return "0".into();
        }
        let mut n = self.clone();
        n.negative = false;
        let mut chars = vec![];
        while !n.is_zero() {
            let mut carry = 0u64;
            for limb in n.limbs.iter_mut().rev() {
                let value = (carry << 32) | *limb as u64;
                *limb = (value / radix as u64) as u32;
                carry = value % radix as u64;
            }
            chars.push(char::from_digit(carry as u32, radix).unwrap());
            n = n.normalized();
        }
        if self.negative {
            chars.push('-');
        }
        chars.into_iter().rev().collect()
    }
}
