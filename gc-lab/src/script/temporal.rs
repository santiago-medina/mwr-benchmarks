//! Temporal (gc-lab/TEMPORAL.md). Compiled only by the second manifest
//! `temporal/Cargo.toml`, which adds the vetted `temporal_rs` crate.
//!
//! The JS layer (brand checks, argument coercion, options reading in the spec's
//! observable order) is ours; arithmetic, parsing and formatting come from the crate.
//! Every crate call runs under `catch_unwind` (`Engine::tcall`): the audit by Rust
//! Unsafe Security Auditor (#92, collab #311) left panics (DoS) as the residual risk.
use super::objects::{Descriptor, Key};
use super::vm::{Engine, ScriptError, Value};
use crate::dual::Id;
use std::collections::HashMap;
use std::rc::Rc;
use temporal_rs::options::{
    RelativeTo, RoundingIncrement, RoundingMode, RoundingOptions, ToStringRoundingOptions, Unit,
};
use temporal_rs::parsers::Precision;
use temporal_rs::{Duration, TemporalError, TemporalResult};

type Result<T> = std::result::Result<T, ScriptError>;

const TO_STRING_TAG: u64 = 1;

/// The internal slots of a Temporal object (all values are immutable).
#[derive(Debug)]
pub(super) enum Slot {
    Duration(Duration),
}

/// Duration fields in the order of the constructor's parameters.
const DURATION_FIELDS: [&str; 10] = [
    "years",
    "months",
    "weeks",
    "days",
    "hours",
    "minutes",
    "seconds",
    "milliseconds",
    "microseconds",
    "nanoseconds",
];

/// Constructors that exist (`Temporal.<Type>`), with their `length`.
const CONSTRUCTORS: [(&str, usize); 1] = [("Duration", 0)];

pub(super) fn is_constructor(name: &str) -> bool {
    name.strip_prefix("Temporal.")
        .is_some_and(|t| CONSTRUCTORS.iter().any(|(c, _)| *c == t))
}

fn error_name(e: &TemporalError) -> &'static str {
    match e.kind() {
        temporal_rs::error::ErrorKind::Type => "TypeError",
        temporal_rs::error::ErrorKind::Syntax => "SyntaxError",
        temporal_rs::error::ErrorKind::Generic => "Error",
        _ => "RangeError",
    }
}

fn duration_field(d: &Duration, i: usize) -> f64 {
    match i {
        0 => d.years() as f64,
        1 => d.months() as f64,
        2 => d.weeks() as f64,
        3 => d.days() as f64,
        4 => d.hours() as f64,
        5 => d.minutes() as f64,
        6 => d.seconds() as f64,
        7 => d.milliseconds() as f64,
        8 => d.microseconds() as f64,
        _ => d.nanoseconds() as f64,
    }
}

/// A duration field value (already integral) as the crate's integer type; values
/// outside i64 cannot form a valid duration (§7.5.x IsValidDuration), except the
/// sub-second fields which the crate keeps in i128.
fn fields_to_duration(values: &[f64; 10]) -> std::result::Result<Duration, TemporalError> {
    let big = |v: f64| -> std::result::Result<i64, TemporalError> {
        if v.abs() >= 9.2e18 {
            Err(TemporalError::range().with_message("duration field out of range"))
        } else {
            Ok(v as i64)
        }
    };
    let huge = |v: f64| -> std::result::Result<i128, TemporalError> {
        if v.abs() >= 1.0e38 {
            Err(TemporalError::range().with_message("duration field out of range"))
        } else {
            Ok(v as i128)
        }
    };
    Duration::new(
        big(values[0])?,
        big(values[1])?,
        big(values[2])?,
        big(values[3])?,
        big(values[4])?,
        big(values[5])?,
        big(values[6])?,
        big(values[7])?,
        huge(values[8])?,
        huge(values[9])?,
    )
}

impl Engine<'_> {
    /// Runs a crate call; a crate error becomes the matching JS error and a panic
    /// becomes a RangeError (TEMPORAL.md "Panics").
    fn tcall<T>(&mut self, f: impl FnOnce() -> TemporalResult<T>) -> Result<T> {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => {
                let name = error_name(&e);
                let message = e.to_string();
                self.throw_error(name, &message)
            }
            Err(_) => self.throw_error("RangeError", "Temporal internal error"),
        }
    }

    fn temporal_slot(&mut self, value: &Value) -> Option<Rc<Slot>> {
        let Value::Object(id) = value else {
            return None;
        };
        if self.is_proxy(*id) {
            return None;
        }
        self.data(*id).ok().and_then(|d| d.temporal.clone())
    }

    fn this_duration(&mut self, receiver: &Value) -> Result<Duration> {
        match self.temporal_slot(receiver).as_deref() {
            Some(Slot::Duration(d)) => Ok(*d),
            _ => self.throw_error("TypeError", "receiver is not a Temporal.Duration"),
        }
    }

    /// OrdinaryCreateFromConstructor with the Temporal slot set.
    fn temporal_object(
        &mut self,
        kind: &str,
        new_target: Option<Value>,
        slot: Slot,
    ) -> Result<Value> {
        let fallback = self.intrinsics.as_ref().unwrap().collection[&format!("Temporal.{kind}")];
        let prototype = match new_target {
            Some(target) => match self.get_member(target, "prototype")? {
                Value::Object(id) => id,
                _ => fallback,
            },
            None => fallback,
        };
        let id = self.new_object(Some(prototype))?;
        self.protect(&Value::Object(id))?;
        // CreateTemporalDuration stores 𝔽(value) per field (§7.5.x): drop any
        // precision the crate's integer arithmetic kept beyond float64.
        let slot = match slot {
            Slot::Duration(d) => {
                let values: [f64; 10] = std::array::from_fn(|i| duration_field(&d, i));
                Slot::Duration(fields_to_duration(&values).unwrap_or(d))
            }
        };
        let mut data = self.data(id)?.clone();
        data.temporal = Some(Rc::new(slot));
        self.replace_data(id, data)?;
        Ok(Value::Object(id))
    }

    // ---- Options (§13.x abstract operations of the Temporal proposal) ----

    /// GetOptionsObject: undefined → a fresh empty object (no observable reads).
    fn options_object(&mut self, value: Value) -> Result<Option<Id>> {
        match value {
            Value::Undefined => Ok(None),
            Value::Object(id) => Ok(Some(id)),
            _ => self.throw_error("TypeError", "options must be an object or undefined"),
        }
    }

    /// GetOption(options, property, ~string~, values, undefined).
    fn string_option(&mut self, options: Option<Id>, property: &str) -> Result<Option<String>> {
        let Some(id) = options else {
            return Ok(None);
        };
        let value = self.get_member(Value::Object(id), property)?;
        if matches!(value, Value::Undefined) {
            return Ok(None);
        }
        self.to_string(value).map(Some)
    }

    /// GetTemporalUnitValuedOption: singular/plural unit names and "auto".
    fn unit_option(&mut self, options: Option<Id>, property: &str) -> Result<Option<Unit>> {
        match self.string_option(options, property)? {
            None => Ok(None),
            Some(s) => match s.parse::<Unit>() {
                Ok(u) => Ok(Some(u)),
                Err(_) => self.throw_error(
                    "RangeError",
                    &format!("{s} is not a valid value for {property}"),
                ),
            },
        }
    }

    /// GetRoundingModeOption.
    fn rounding_mode_option(&mut self, options: Option<Id>) -> Result<Option<RoundingMode>> {
        match self.string_option(options, "roundingMode")? {
            None => Ok(None),
            Some(s) => match s.parse::<RoundingMode>() {
                Ok(m) => Ok(Some(m)),
                Err(_) => self.throw_error(
                    "RangeError",
                    &format!("{s} is not a valid value for roundingMode"),
                ),
            },
        }
    }

    /// GetRoundingIncrementOption.
    fn increment_option(&mut self, options: Option<Id>) -> Result<Option<RoundingIncrement>> {
        let Some(id) = options else {
            return Ok(None);
        };
        let value = self.get_member(Value::Object(id), "roundingIncrement")?;
        if matches!(value, Value::Undefined) {
            return Ok(None);
        }
        let n = self.to_number(value)?;
        if !n.is_finite() {
            return self.throw_error("RangeError", "roundingIncrement must be finite");
        }
        let n = n.trunc();
        if !(1.0..=1e9).contains(&n) {
            return self.throw_error("RangeError", "roundingIncrement out of range");
        }
        self.tcall(|| RoundingIncrement::try_new(n as u32))
            .map(Some)
    }

    /// GetTemporalFractionalSecondDigitsOption.
    fn digits_option(&mut self, options: Option<Id>) -> Result<Precision> {
        let Some(id) = options else {
            return Ok(Precision::Auto);
        };
        let value = self.get_member(Value::Object(id), "fractionalSecondDigits")?;
        match value {
            Value::Undefined => Ok(Precision::Auto),
            Value::Number(bits) => {
                let n = f64::from_bits(bits);
                if !n.is_finite() {
                    return self.throw_error("RangeError", "fractionalSecondDigits must be finite");
                }
                let n = n.floor();
                if !(0.0..=9.0).contains(&n) {
                    return self.throw_error("RangeError", "fractionalSecondDigits out of range");
                }
                Ok(Precision::Digit(n as u8))
            }
            other => {
                let s = self.to_string(other)?;
                if s == "auto" {
                    Ok(Precision::Auto)
                } else {
                    self.throw_error(
                        "RangeError",
                        &format!("{s} is not a valid value for fractionalSecondDigits"),
                    )
                }
            }
        }
    }

    /// GetTemporalRelativeToOption. Plain dates and zoned date-times arrive with
    /// those types; until then only `undefined` is handled.
    fn relative_to_option(&mut self, options: Option<Id>) -> Result<Option<RelativeTo>> {
        let Some(id) = options else {
            return Ok(None);
        };
        let value = self.get_member(Value::Object(id), "relativeTo")?;
        if matches!(value, Value::Undefined) {
            return Ok(None);
        }
        Err(ScriptError::Unsupported("Temporal relativeTo"))
    }

    // ---- Duration (§7) ----

    /// ToIntegerIfIntegral.
    fn integer_if_integral(&mut self, value: Value) -> Result<f64> {
        let n = self.to_number(value)?;
        if !n.is_finite() || n.trunc() != n {
            return self.throw_error("RangeError", "value must be an integer");
        }
        Ok(n + 0.0)
    }

    /// ToTemporalPartialDurationRecord: fields read in alphabetical order.
    fn partial_duration(&mut self, value: Value) -> Result<[Option<f64>; 10]> {
        let Value::Object(_) = value else {
            return self.throw_error("TypeError", "duration-like must be an object");
        };
        let mut sorted: Vec<(usize, &str)> = DURATION_FIELDS.iter().copied().enumerate().collect();
        sorted.sort_by_key(|(_, n)| *n);
        let mut out = [None; 10];
        for (i, name) in sorted {
            let v = self.get_member(value.clone(), name)?;
            if !matches!(v, Value::Undefined) {
                out[i] = Some(self.integer_if_integral(v)?);
            }
        }
        if out.iter().all(Option::is_none) {
            return self.throw_error("TypeError", "duration-like object has no duration fields");
        }
        Ok(out)
    }

    /// ToTemporalDuration.
    fn to_duration(&mut self, value: Value) -> Result<Duration> {
        if let Some(Slot::Duration(d)) = self.temporal_slot(&value).as_deref() {
            return Ok(*d);
        }
        match value {
            Value::Object(_) => {
                let partial = self.partial_duration(value)?;
                let values = partial.map(|v| v.unwrap_or(0.0));
                self.tcall(|| fields_to_duration(&values))
            }
            Value::String(s) => self.tcall(|| Duration::from_utf8(s.as_bytes())),
            _ => self.throw_error("TypeError", "cannot convert value to a Temporal.Duration"),
        }
    }

    fn duration_builtin(
        &mut self,
        method: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Undefined);
        match method {
            "from" => {
                let d = self.to_duration(arg(0))?;
                return self.temporal_object("Duration", None, Slot::Duration(d));
            }
            "compare" => {
                let one = self.to_duration(arg(0))?;
                let two = self.to_duration(arg(1))?;
                let options = self.options_object(arg(2))?;
                let relative = self.relative_to_option(options)?;
                let ordering = self.tcall(|| one.compare(&two, relative))?;
                return Ok(Value::number(ordering as i8 as f64));
            }
            _ => {}
        }
        let method = method.strip_prefix("prototype.").unwrap_or(method);
        let d = self.this_duration(&receiver)?;
        if let Some(i) = DURATION_FIELDS.iter().position(|f| *f == method) {
            return Ok(Value::number(duration_field(&d, i) + 0.0));
        }
        let make = |engine: &mut Self, d: Duration| {
            engine.temporal_object("Duration", None, Slot::Duration(d))
        };
        match method {
            "sign" => Ok(Value::number(d.sign() as i8 as f64)),
            "blank" => Ok(Value::Bool(d.is_zero())),
            "negated" => make(self, d.negated()),
            "abs" => make(self, d.abs()),
            "with" => {
                let partial = self.partial_duration(arg(0))?;
                let mut values = [0.0; 10];
                for (i, v) in values.iter_mut().enumerate() {
                    *v = partial[i].unwrap_or_else(|| duration_field(&d, i));
                }
                let next = self.tcall(|| fields_to_duration(&values))?;
                make(self, next)
            }
            "add" | "subtract" => {
                let other = self.to_duration(arg(0))?;
                let next = if method == "add" {
                    self.tcall(|| d.add(&other))?
                } else {
                    self.tcall(|| d.subtract(&other))?
                };
                make(self, next)
            }
            "round" => {
                let round_to = arg(0);
                let options = match round_to {
                    Value::Undefined => {
                        return self.throw_error("TypeError", "round requires an argument")
                    }
                    Value::String(ref s) => {
                        let unit = match s.parse::<Unit>() {
                            Ok(u) => u,
                            Err(_) => {
                                return self.throw_error(
                                    "RangeError",
                                    &format!("{s} is not a valid value for smallestUnit"),
                                )
                            }
                        };
                        let mut opts = RoundingOptions::default();
                        opts.smallest_unit = Some(unit);
                        let next = self.tcall(|| d.round(opts, None))?;
                        return make(self, next);
                    }
                    other => self.options_object(other)?,
                };
                let largest_unit = self.unit_option(options, "largestUnit")?;
                let relative = self.relative_to_option(options)?;
                let increment = self.increment_option(options)?;
                let rounding_mode = self.rounding_mode_option(options)?;
                let smallest_unit = self.unit_option(options, "smallestUnit")?;
                let mut opts = RoundingOptions::default();
                opts.largest_unit = largest_unit;
                opts.smallest_unit = smallest_unit;
                opts.rounding_mode = rounding_mode;
                opts.increment = increment;
                let next = self.tcall(|| d.round(opts, relative))?;
                make(self, next)
            }
            "total" => {
                let total_of = arg(0);
                let (unit, relative) = match total_of {
                    Value::Undefined => {
                        return self.throw_error("TypeError", "total requires an argument")
                    }
                    Value::String(ref s) => match s.parse::<Unit>() {
                        Ok(u) => (Some(u), None),
                        Err(_) => {
                            return self.throw_error(
                                "RangeError",
                                &format!("{s} is not a valid value for unit"),
                            )
                        }
                    },
                    other => {
                        let options = self.options_object(other)?;
                        let relative = self.relative_to_option(options)?;
                        (self.unit_option(options, "unit")?, relative)
                    }
                };
                let Some(unit) = unit else {
                    return self.throw_error("RangeError", "unit is required");
                };
                if unit == Unit::Auto {
                    return self.throw_error("RangeError", "auto is not a valid value for unit");
                }
                let total = self.tcall(|| d.total(unit, relative))?;
                Ok(Value::number(total.as_inner()))
            }
            "toString" | "toJSON" | "toLocaleString" => {
                let options = if method == "toString" {
                    self.options_object(arg(0))?
                } else {
                    None
                };
                let precision = self.digits_option(options)?;
                let rounding_mode = self.rounding_mode_option(options)?;
                let smallest_unit = self.unit_option(options, "smallestUnit")?;
                if matches!(smallest_unit, Some(Unit::Hour | Unit::Minute | Unit::Auto))
                    || smallest_unit.is_some_and(|u| u.is_date_unit())
                {
                    return self.throw_error(
                        "RangeError",
                        "invalid smallestUnit for Duration.prototype.toString",
                    );
                }
                let opts = ToStringRoundingOptions {
                    precision,
                    smallest_unit,
                    rounding_mode,
                };
                let s = self.tcall(|| d.as_temporal_string(opts))?;
                Ok(Value::String(s))
            }
            "valueOf" => self.throw_error(
                "TypeError",
                "Temporal.Duration cannot be converted to a primitive",
            ),
            _ => Err(ScriptError::Unsupported("Temporal.Duration method")),
        }
    }

    // ---- Dispatch ----

    pub(super) fn construct_temporal(
        &mut self,
        name: &str,
        new_target: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Undefined);
        match name {
            "Temporal.Duration" => {
                let mut values = [0.0; 10];
                for (i, v) in values.iter_mut().enumerate() {
                    let a = arg(i);
                    if !matches!(a, Value::Undefined) {
                        *v = self.integer_if_integral(a)?;
                    }
                }
                let d = self.tcall(|| fields_to_duration(&values))?;
                self.temporal_object("Duration", Some(new_target), Slot::Duration(d))
            }
            _ => Err(ScriptError::Unsupported("Temporal constructor")),
        }
    }

    pub(super) fn temporal_builtin(
        &mut self,
        name: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        if is_constructor(name) {
            return self.throw_error("TypeError", &format!("{name} constructor requires 'new'"));
        }
        let rest = name.strip_prefix("Temporal.").unwrap_or(name);
        if let Some(method) = rest.strip_prefix("Duration.") {
            return self.duration_builtin(method, receiver, args);
        }
        Err(ScriptError::Unsupported("Temporal builtin"))
    }

    fn temporal_named(&mut self, f: Id, name: &str, length: usize) -> Result<()> {
        self.define(
            f,
            "length",
            Descriptor {
                value: Some(Value::number(length as f64)),
                configurable: Some(true),
                ..Default::default()
            },
        )?;
        self.define(
            f,
            "name",
            Descriptor {
                value: Some(Value::String(name.into())),
                configurable: Some(true),
                ..Default::default()
            },
        )
        .map(|_| ())
    }

    fn temporal_tag(&mut self, id: Id, tag: &str) -> Result<()> {
        self.define(
            id,
            Key::Symbol(TO_STRING_TAG),
            Descriptor {
                value: Some(Value::String(tag.into())),
                configurable: Some(true),
                ..Default::default()
            },
        )
        .map(|_| ())
    }

    /// Creates the `Temporal` namespace (§1.1) and its constructors.
    pub(super) fn bootstrap_temporal(
        &mut self,
        env: Id,
        objects: &mut HashMap<String, Id>,
    ) -> Result<()> {
        let i = self.intrinsics.as_ref().unwrap();
        let function = i.function;
        let object = i.object;
        let namespace = self.new_object(Some(object))?;
        self.runtime.link(env, namespace)?;
        self.temporal_tag(namespace, "Temporal")?;
        objects.insert("Temporal".into(), namespace);
        for (kind, length) in CONSTRUCTORS {
            let full = format!("Temporal.{kind}");
            let ctor = self.intrinsic_function(env, &full, Some(function))?;
            self.temporal_named(ctor, kind, length)?;
            let proto = self.new_object(Some(object))?;
            self.runtime.link(env, proto)?;
            self.define(
                ctor,
                "prototype",
                Descriptor {
                    value: Some(Value::Object(proto)),
                    ..Default::default()
                },
            )?;
            self.hidden(proto, "constructor", Value::Object(ctor))?;
            self.temporal_tag(proto, &full)?;
            self.intrinsics
                .as_mut()
                .unwrap()
                .collection
                .insert(full.clone(), proto);
            self.intrinsics
                .as_mut()
                .unwrap()
                .functions
                .insert(full.clone(), ctor);
            self.hidden(namespace, kind, Value::Object(ctor))?;
            let (statics, getters, methods) = members(kind);
            for (method, length) in statics {
                let f =
                    self.intrinsic_function(env, &format!("{full}.{method}"), Some(function))?;
                self.temporal_named(f, method, *length)?;
                self.hidden(ctor, *method, Value::Object(f))?;
            }
            for getter in getters {
                let f = self.intrinsic_function(
                    env,
                    &format!("{full}.prototype.{getter}"),
                    Some(function),
                )?;
                self.temporal_named(f, &format!("get {getter}"), 0)?;
                self.define(
                    proto,
                    *getter,
                    Descriptor {
                        get: Some(Value::Object(f)),
                        configurable: Some(true),
                        ..Default::default()
                    },
                )?;
            }
            for (method, length) in methods {
                let f = self.intrinsic_function(
                    env,
                    &format!("{full}.prototype.{method}"),
                    Some(function),
                )?;
                self.temporal_named(f, method, *length)?;
                self.hidden(proto, *method, Value::Object(f))?;
            }
        }
        Ok(())
    }
}

type Members = (
    &'static [(&'static str, usize)],
    &'static [&'static str],
    &'static [(&'static str, usize)],
);

/// (static methods, prototype getters, prototype methods) per type.
fn members(kind: &str) -> Members {
    match kind {
        "Duration" => (
            &[("from", 1), ("compare", 2)],
            &[
                "years",
                "months",
                "weeks",
                "days",
                "hours",
                "minutes",
                "seconds",
                "milliseconds",
                "microseconds",
                "nanoseconds",
                "sign",
                "blank",
            ],
            &[
                ("with", 1),
                ("negated", 0),
                ("abs", 0),
                ("add", 1),
                ("subtract", 1),
                ("round", 1),
                ("total", 1),
                ("toString", 0),
                ("toJSON", 0),
                ("toLocaleString", 0),
                ("valueOf", 0),
            ],
        ),
        _ => (&[], &[], &[]),
    }
}
