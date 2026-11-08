//! Byte codecs contain no callbacks or graph edges and are shared by DataView and typed views.
use super::{bigint::Big, vm::Value};
use std::rc::Rc;
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Element {
    I8,
    U8,
    U8Clamped,
    I16,
    U16,
    I32,
    U32,
    F16,
    F32,
    F64,
    I64,
    U64,
}
pub(super) const ELEMENTS: &[(&str, Element)] = &[
    ("Int8", Element::I8),
    ("Uint8", Element::U8),
    ("Uint8Clamped", Element::U8Clamped),
    ("Int16", Element::I16),
    ("Uint16", Element::U16),
    ("Int32", Element::I32),
    ("Uint32", Element::U32),
    ("Float16", Element::F16),
    ("Float32", Element::F32),
    ("Float64", Element::F64),
    ("BigInt64", Element::I64),
    ("BigUint64", Element::U64),
];
impl Element {
    pub fn size(self) -> usize {
        match self {
            Self::I8 | Self::U8 | Self::U8Clamped => 1,
            Self::I16 | Self::U16 | Self::F16 => 2,
            Self::I32 | Self::U32 | Self::F32 => 4,
            _ => 8,
        }
    }
    pub fn big(self) -> bool {
        matches!(self, Self::I64 | Self::U64)
    }
    pub fn read(self, bytes: &[u8], little: bool) -> Value {
        let mut raw = 0u64;
        for (index, byte) in bytes.iter().enumerate() {
            raw |= (*byte as u64)
                << if little {
                    index * 8
                } else {
                    (bytes.len() - index - 1) * 8
                };
        }
        let number = match self {
            Self::I8 => raw as u8 as i8 as f64,
            Self::U8 | Self::U8Clamped => raw as u8 as f64,
            Self::I16 => raw as u16 as i16 as f64,
            Self::U16 => raw as u16 as f64,
            Self::I32 => raw as u32 as i32 as f64,
            Self::U32 => raw as u32 as f64,
            Self::F16 => decode_half(raw as u16),
            Self::F32 => f32::from_bits(raw as u32) as f64,
            Self::F64 => f64::from_bits(raw),
            Self::I64 | Self::U64 => {
                return Value::BigInt(Rc::new(Big::from_u64(raw).truncate(64, self == Self::I64)))
            }
        };
        Value::number(number)
    }
    pub fn bytes(self, value: &Value, little: bool) -> Vec<u8> {
        let raw = if self.big() {
            let Value::BigInt(n) = value else {
                unreachable!()
            };
            n.truncate(64, false).to_u64().unwrap()
        } else {
            let Value::Number(n) = value else {
                unreachable!()
            };
            let n = f64::from_bits(*n);
            match self {
                Self::F16 => encode_half(n) as u64,
                Self::F32 => (n as f32).to_bits() as u64,
                Self::F64 => n.to_bits(),
                Self::U8Clamped => {
                    if n.is_nan() {
                        0
                    } else {
                        n.clamp(0.0, 255.0).round_ties_even() as u64
                    }
                }
                _ => super::numeric::uint32(n) as u64,
            }
        };
        (0..self.size())
            .map(|i| {
                (raw >> if little {
                    i * 8
                } else {
                    (self.size() - i - 1) * 8
                }) as u8
            })
            .collect()
    }
}
fn decode_half(bits: u16) -> f64 {
    let exponent = (bits >> 10) & 31;
    let fraction = bits & 1023;
    let value = if exponent == 31 {
        if fraction == 0 {
            f64::INFINITY
        } else {
            f64::NAN
        }
    } else if exponent == 0 {
        fraction as f64 * 2f64.powi(-24)
    } else {
        (1.0 + fraction as f64 / 1024.0) * 2f64.powi(exponent as i32 - 15)
    };
    if bits & 0x8000 != 0 {
        -value
    } else {
        value
    }
}
fn encode_half(n: f64) -> u16 {
    let n = super::numeric::f16(n);
    let sign = if n.is_sign_negative() { 0x8000 } else { 0 };
    let n = n.abs();
    if n.is_nan() {
        return sign | 0x7e00;
    }
    if n.is_infinite() {
        return sign | 0x7c00;
    }
    if n < 2f64.powi(-14) {
        return sign | (n * 2f64.powi(24)) as u16;
    }
    let exponent = n.log2().floor() as i32;
    sign | (((exponent + 15) as u16) << 10) | (((n / 2f64.powi(exponent) - 1.0) * 1024.0) as u16)
}
