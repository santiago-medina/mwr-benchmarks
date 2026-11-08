//! Shared ECMAScript numeric conversions; no object coercion in this layer.
pub(super) fn uint32(n: f64) -> u32 {
    if !n.is_finite() || n == 0.0 {
        return 0;
    }
    n.trunc().rem_euclid(4294967296.0) as u32
}
pub(super) fn pow(base: f64, exponent: f64) -> f64 {
    if exponent == 0.0 {
        1.0
    } else if base.abs() == 1.0 && exponent.is_infinite() {
        f64::NAN
    } else {
        base.powf(exponent)
    }
}
