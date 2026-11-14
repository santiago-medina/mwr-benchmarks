//! Temporal (gc-lab/TEMPORAL.md). Compiled only by the second manifest
//! `temporal/Cargo.toml`, which adds the vetted `temporal_rs` crate.
//!
//! The JS layer (brand checks, argument coercion, options reading in the spec's
//! observable order) is ours; arithmetic, parsing and formatting come from the crate.
//!
//! Temporal strings reach the crate parsers through `JsString::lossy`: no Temporal grammar
//! (ISO 8601, time zone identifiers, units, calendars) admits a surrogate code unit, so a
//! U+FFFD stand-in for a lone surrogate is rejected exactly like the original (RangeError).
//!
//! Every crate call runs under `catch_unwind` (`Engine::tcall`): the audit by Rust
//! Unsafe Security Auditor (#92, collab #311) left panics (DoS) as the residual risk.
use super::bigint::Big;
use super::objects::{Descriptor, Key};
use super::vm::{Engine, ScriptError, Value};
use crate::dual::Id;
use std::collections::HashMap;
use std::rc::Rc;
use temporal_rs::fields::{
    CalendarFields, DateTimeFields, YearMonthCalendarFields, ZonedDateTimeFields,
};
use temporal_rs::host::{HostClock, HostHooks, HostTimeZone};
use temporal_rs::now::Now;
use temporal_rs::options::{
    DifferenceSettings, Disambiguation, DisplayCalendar, DisplayOffset, DisplayTimeZone,
    OffsetDisambiguation, Overflow, RelativeTo, RoundingIncrement, RoundingMode, RoundingOptions,
    ToStringRoundingOptions, Unit,
};
use temporal_rs::parsed_intermediates::ParsedZonedDateTime;
use temporal_rs::parsers::Precision;
use temporal_rs::partial::{PartialDate, PartialTime, PartialYearMonth, PartialZonedDateTime};
use temporal_rs::provider::TransitionDirection;
use temporal_rs::unix_time::EpochNanoseconds;

use temporal_rs::{
    Calendar, Duration, Instant, MonthCode, PlainDate, PlainDateTime, PlainMonthDay, PlainTime,
    PlainYearMonth, TemporalError, TemporalResult, TimeZone, TinyAsciiStr, UtcOffset,
    ZonedDateTime,
};

type Result<T> = std::result::Result<T, ScriptError>;

const TO_STRING_TAG: u64 = 1;

/// The internal slots of a Temporal object (all values are immutable).
#[derive(Debug)]
pub(super) enum Slot {
    Duration(Duration),
    PlainDate(PlainDate),
    PlainTime(PlainTime),
    PlainDateTime(PlainDateTime),
    Instant(Instant),
    ZonedDateTime(ZonedDateTime),
    PlainYearMonth(PlainYearMonth),
    PlainMonthDay(PlainMonthDay),
}

impl Slot {
    /// The calendar of a slot that carries one ([[Calendar]]).
    fn calendar(&self) -> Option<&Calendar> {
        match self {
            Slot::ZonedDateTime(z) => Some(z.calendar()),
            Slot::PlainYearMonth(ym) => Some(ym.calendar()),
            Slot::PlainMonthDay(md) => Some(md.calendar()),
            Slot::Duration(_) | Slot::PlainTime(_) | Slot::Instant(_) => None,
            Slot::PlainDate(d) => Some(d.calendar()),
            Slot::PlainDateTime(d) => Some(d.calendar()),
        }
    }
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
const CONSTRUCTORS: [(&str, usize); 8] = [
    ("Duration", 0),
    ("PlainDate", 3),
    ("PlainTime", 0),
    ("PlainDateTime", 3),
    ("Instant", 1),
    ("ZonedDateTime", 2),
    ("PlainYearMonth", 2),
    ("PlainMonthDay", 2),
];

/// The date field names of PrepareCalendarFields for each Temporal type.
const DATE_NAMES: [&str; 4] = ["day", "month", "monthCode", "year"];
const YEAR_MONTH_NAMES: [&str; 3] = ["month", "monthCode", "year"];

/// Time fields in the order of the PlainTime constructor's parameters, with the
/// largest valid value of each (IsValidTime, §4.5.x).
const TIME_FIELDS: [(&str, f64); 6] = [
    ("hour", 23.0),
    ("minute", 59.0),
    ("second", 59.0),
    ("millisecond", 999.0),
    ("microsecond", 999.0),
    ("nanosecond", 999.0),
];

/// Which units a difference or rounding operation accepts (the spec's unit groups).
#[derive(Clone, Copy, PartialEq)]
enum UnitGroup {
    Date,
    Time,
    DateTime,
    /// year and month only (PlainYearMonth differences).
    YearMonth,
}

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
            Some(target) => {
                let key = format!("Temporal.{kind}");
                self.prototype_from_constructor(target, |i| i.collection[&key])?
            }
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
            other => other,
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
        match self.temporal_slot(&value).as_deref() {
            Some(Slot::PlainDate(d)) => return Ok(Some(RelativeTo::PlainDate(d.clone()))),
            Some(Slot::PlainDateTime(d)) => {
                return Ok(Some(RelativeTo::PlainDate(d.to_plain_date())))
            }
            Some(Slot::ZonedDateTime(z)) => return Ok(Some(RelativeTo::ZonedDateTime(z.clone()))),
            _ => {}
        }
        match value {
            Value::Object(_) => {
                let calendar = self.calendar_with_iso_default(&value)?;
                let (fields, time, extras) =
                    self.date_time_fields(&calendar, &value, false, true, &["offset", "timeZone"])?;
                if let Value::String(tz) = &extras[1] {
                    // A zoned relativeTo: InterpretISODateTimeOffset with offset
                    // ~reject~, disambiguation ~compatible~, match-exactly.
                    let tz = self.tcall(|| TimeZone::try_from_str(&tz.lossy()))?;
                    let time =
                        self.regulate_time(time.map(|v| v.unwrap_or(0.0)), Overflow::Constrain)?;
                    let offset = match &extras[0] {
                        Value::String(o) => {
                            Some(self.tcall(|| UtcOffset::from_utf8(o.lossy().as_bytes()))?)
                        }
                        _ => None,
                    };
                    let partial = PartialZonedDateTime {
                        fields: ZonedDateTimeFields {
                            calendar_fields: fields,
                            time: full_partial_time(&time),
                            offset,
                        },
                        timezone: Some(tz),
                        calendar,
                    };
                    let z = self.tcall(|| {
                        ZonedDateTime::from_partial(
                            partial,
                            Some(Overflow::Constrain),
                            Some(Disambiguation::Compatible),
                            Some(OffsetDisambiguation::Reject),
                        )
                    })?;
                    return Ok(Some(RelativeTo::ZonedDateTime(z)));
                }
                // InterpretTemporalDateTimeFields with ~constrain~; with no time zone
                // the result is a plain date.
                let mut partial = PartialDate::new();
                partial.calendar_fields = fields;
                partial.calendar = calendar;
                let date = self.tcall(|| PlainDate::from_partial(partial, None))?;
                self.regulate_time(time.map(|v| v.unwrap_or(0.0)), Overflow::Constrain)?;
                Ok(Some(RelativeTo::PlainDate(date)))
            }
            Value::String(s) => self
                .tcall(|| RelativeTo::try_from_str(&s.lossy()))
                .map(Some),
            _ => self.throw_error("TypeError", "relativeTo must be an object or a string"),
        }
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
            Value::String(s) => self.tcall(|| Duration::from_utf8(s.lossy().as_bytes())),
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
                        let unit = match s.lossy().parse::<Unit>() {
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
                    Value::String(ref s) => match s.lossy().parse::<Unit>() {
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
                Ok(Value::String(s.into()))
            }
            "valueOf" => self.throw_error(
                "TypeError",
                "Temporal.Duration cannot be converted to a primitive",
            ),
            _ => Err(ScriptError::Unsupported("Temporal.Duration method")),
        }
    }

    // ---- Shared conversions (§13 abstract operations) ----

    /// ToIntegerWithTruncation: NaN and ±∞ are RangeErrors.
    fn integer_with_truncation(&mut self, value: Value) -> Result<f64> {
        let n = self.to_number(value)?;
        if !n.is_finite() {
            return self.throw_error("RangeError", "value must be finite");
        }
        Ok(n.trunc() + 0.0)
    }

    /// ToPositiveIntegerWithTruncation.
    fn positive_integer(&mut self, value: Value) -> Result<f64> {
        let n = self.integer_with_truncation(value)?;
        if n <= 0.0 {
            return self.throw_error("RangeError", "value must be a positive integer");
        }
        Ok(n)
    }

    /// GetTemporalOverflowOption.
    fn overflow_option(&mut self, options: Option<Id>) -> Result<Overflow> {
        match self.string_option(options, "overflow")? {
            None => Ok(Overflow::Constrain),
            Some(s) => match s.as_str() {
                "constrain" => Ok(Overflow::Constrain),
                "reject" => Ok(Overflow::Reject),
                _ => self.throw_error(
                    "RangeError",
                    &format!("{s} is not a valid value for overflow"),
                ),
            },
        }
    }

    /// GetTemporalShowCalendarNameOption.
    fn show_calendar_option(&mut self, options: Option<Id>) -> Result<DisplayCalendar> {
        match self.string_option(options, "calendarName")? {
            None => Ok(DisplayCalendar::Auto),
            Some(s) => match s.as_str() {
                "auto" => Ok(DisplayCalendar::Auto),
                "always" => Ok(DisplayCalendar::Always),
                "never" => Ok(DisplayCalendar::Never),
                "critical" => Ok(DisplayCalendar::Critical),
                _ => self.throw_error(
                    "RangeError",
                    &format!("{s} is not a valid value for calendarName"),
                ),
            },
        }
    }

    /// ToTemporalCalendarIdentifier: a Temporal object's [[Calendar]], or a string
    /// parsed by ParseTemporalCalendarString and canonicalized.
    fn calendar_identifier(&mut self, value: Value) -> Result<Calendar> {
        if let Some(c) = self
            .temporal_slot(&value)
            .as_deref()
            .and_then(Slot::calendar)
        {
            return Ok(c.clone());
        }
        match value {
            Value::String(s) => self.tcall(|| s.lossy().parse::<Calendar>()),
            _ => self.throw_error("TypeError", "calendar must be a string"),
        }
    }

    /// GetTemporalCalendarIdentifierWithISODefault.
    fn calendar_with_iso_default(&mut self, item: &Value) -> Result<Calendar> {
        if let Some(c) = self.temporal_slot(item).as_deref().and_then(Slot::calendar) {
            return Ok(c.clone());
        }
        let calendar = self.get_member(item.clone(), "calendar")?;
        if matches!(calendar, Value::Undefined) {
            return Ok(Calendar::ISO);
        }
        self.calendar_identifier(calendar)
    }

    /// PrepareCalendarFields(calendar, fields, «year, month, month-code, day», «», required)
    /// for the date fields: properties are read in alphabetical order and each value
    /// is converted as soon as it is read. With `partial`, at least one must be present.
    fn calendar_fields(
        &mut self,
        calendar: &Calendar,
        item: &Value,
        partial: bool,
    ) -> Result<CalendarFields> {
        self.date_time_fields(calendar, item, partial, false, &[])
            .map(|(fields, _, _)| fields)
    }

    /// PrepareCalendarFields with the time fields too (`with_time`): the date and
    /// time properties are read in one alphabetical pass; the time values are
    /// returned in constructor order. `extra` adds `offset` (converted with
    /// ToOffsetString) and/or `timeZone` (returned as read), in that order.
    fn date_time_fields(
        &mut self,
        calendar: &Calendar,
        item: &Value,
        partial: bool,
        with_time: bool,
        extra: &[&'static str],
    ) -> Result<(CalendarFields, [Option<f64>; 6], Vec<Value>)> {
        self.prepare_fields(calendar, item, partial, &DATE_NAMES, with_time, extra)
    }

    /// PrepareCalendarFields over the date field names `base` (era/eraYear are added
    /// with `year` for calendars that have eras), plus the time fields and extras.
    fn prepare_fields(
        &mut self,
        calendar: &Calendar,
        item: &Value,
        partial: bool,
        base: &[&'static str],
        with_time: bool,
        extra: &[&'static str],
    ) -> Result<(CalendarFields, [Option<f64>; 6], Vec<Value>)> {
        let mut names = base.to_vec();
        if base.contains(&"year") && has_eras(calendar) {
            names.extend(["era", "eraYear"]);
        }
        if with_time {
            names.extend(TIME_FIELDS.iter().map(|(n, _)| *n));
        }
        names.extend(extra);
        names.sort_unstable();
        let mut extras = vec![Value::Undefined; extra.len()];
        let mut fields = CalendarFields::new();
        let mut time = [None; 6];
        let mut any = false;
        for name in names {
            let value = self.get_member(item.clone(), name)?;
            if matches!(value, Value::Undefined) {
                continue;
            }
            any = true;
            match name {
                "day" => fields.day = Some(self.positive_integer(value)?.min(255.0) as u8),
                "month" => fields.month = Some(self.positive_integer(value)?.min(255.0) as u8),
                "year" => fields.year = Some(clamp_year(self.integer_with_truncation(value)?)),
                "eraYear" => {
                    fields.era_year = Some(clamp_i32(self.integer_with_truncation(value)?))
                }
                "offset" | "timeZone" => {
                    let i = extra.iter().position(|n| *n == name).unwrap();
                    extras[i] = if name == "offset" {
                        // ToOffsetString: ToPrimitive, a String, ParseDateTimeUTCOffset.
                        let p = self.to_primitive(value, true)?;
                        let Value::String(s) = p else {
                            return self.throw_error("TypeError", "offset must be a string");
                        };
                        self.tcall(|| UtcOffset::from_utf8(s.lossy().as_bytes()))?;
                        Value::String(s)
                    } else {
                        // ToTemporalTimeZoneIdentifier, kept as its identifier string.
                        let tz = self.time_zone_identifier(value)?;
                        Value::String((self.tcall(|| tz.identifier())?).into())
                    };
                }
                _ if TIME_FIELDS.iter().any(|(n, _)| *n == name) => {
                    let i = TIME_FIELDS.iter().position(|(n, _)| *n == name).unwrap();
                    time[i] = Some(self.integer_with_truncation(value)?);
                }
                "era" => {
                    let s = self.to_string(value)?;
                    match TinyAsciiStr::<19>::try_from_str(&s) {
                        Ok(era) => fields.era = Some(era),
                        Err(_) => return self.throw_error("RangeError", "invalid era"),
                    }
                }
                _ => {
                    // ToMonthCode: ToPrimitive, a String, then ParseMonthCode.
                    let p = self.to_primitive(value, true)?;
                    let Value::String(s) = p else {
                        return self.throw_error("TypeError", "monthCode must be a string");
                    };
                    fields.month_code =
                        Some(self.tcall(|| MonthCode::try_from_utf8(s.lossy().as_bytes()))?);
                }
            }
        }
        if partial && !any {
            return self.throw_error("TypeError", "object has no date fields");
        }
        Ok((fields, time, extras))
    }

    /// ToTemporalDate(item [, options]). `options: None` means the caller passes no
    /// options argument at all, so nothing is read.
    fn to_temporal_date(&mut self, item: Value, options: Option<Value>) -> Result<PlainDate> {
        let read_options = |engine: &mut Self| -> Result<Overflow> {
            match options.clone() {
                None => Ok(Overflow::Constrain),
                Some(o) => {
                    let o = engine.options_object(o)?;
                    engine.overflow_option(o)
                }
            }
        };
        if let Value::Object(_) = item {
            match self.temporal_slot(&item).as_deref() {
                Some(Slot::PlainDate(d)) => {
                    let d = d.clone();
                    read_options(self)?;
                    return Ok(d);
                }
                Some(Slot::PlainDateTime(dt)) => {
                    let d = dt.to_plain_date();
                    read_options(self)?;
                    return Ok(d);
                }
                Some(Slot::ZonedDateTime(z)) => {
                    let d = z.to_plain_date();
                    read_options(self)?;
                    return Ok(d);
                }
                _ => {}
            }
            let calendar = self.calendar_with_iso_default(&item)?;
            let fields = self.calendar_fields(&calendar, &item, false)?;
            let overflow = read_options(self)?;
            let mut partial = PartialDate::new();
            partial.calendar_fields = fields;
            partial.calendar = calendar;
            return self.tcall(|| PlainDate::from_partial(partial, Some(overflow)));
        }
        let Value::String(s) = item else {
            return self.throw_error("TypeError", "cannot convert value to a Temporal.PlainDate");
        };
        let date = self.tcall(|| PlainDate::from_utf8(s.lossy().as_bytes()))?;
        read_options(self)?;
        Ok(date)
    }

    /// GetDifferenceSettings(operation, options, group, «», smallest default, largest
    /// default): the four options in alphabetical order, then the unit-group checks;
    /// the remaining checks (largest ≥ smallest, increment bounds) are the crate's.
    fn difference_settings(
        &mut self,
        options: Option<Id>,
        group: UnitGroup,
    ) -> Result<DifferenceSettings> {
        let largest_unit = self.unit_option(options, "largestUnit")?;
        let increment = self.increment_option(options)?;
        let rounding_mode = self.rounding_mode_option(options)?;
        let smallest_unit = self.unit_option(options, "smallestUnit")?;
        let fits = |u: Unit| match group {
            UnitGroup::Date => u.is_date_unit(),
            UnitGroup::Time => u.is_time_unit(),
            UnitGroup::DateTime => true,
            UnitGroup::YearMonth => matches!(u, Unit::Year | Unit::Month),
        };
        let bad_largest = largest_unit.is_some_and(|u| u != Unit::Auto && !fits(u));
        let bad_smallest = smallest_unit.is_some_and(|u| !fits(u));
        if bad_largest || bad_smallest {
            return self.throw_error("RangeError", "unit is not allowed for this difference");
        }
        let mut settings = DifferenceSettings::default();
        settings.largest_unit = largest_unit;
        settings.smallest_unit = smallest_unit;
        settings.rounding_mode = rounding_mode;
        settings.increment = increment;
        Ok(settings)
    }

    fn this_plain_date(&mut self, receiver: &Value) -> Result<PlainDate> {
        match self.temporal_slot(receiver).as_deref() {
            Some(Slot::PlainDate(d)) => Ok(d.clone()),
            _ => self.throw_error("TypeError", "receiver is not a Temporal.PlainDate"),
        }
    }

    /// RejectTemporalLikeObject.
    fn reject_temporal_like(&mut self, value: &Value) -> Result<()> {
        if self
            .temporal_slot(value)
            .is_some_and(|s| !matches!(*s, Slot::Duration(_)))
        {
            return self.throw_error("TypeError", "a Temporal object is not a valid argument");
        }
        for property in ["calendar", "timeZone"] {
            let v = self.get_member(value.clone(), property)?;
            if !matches!(v, Value::Undefined) {
                return self.throw_error(
                    "TypeError",
                    &format!("{property} is not allowed in a with() argument"),
                );
            }
        }
        Ok(())
    }

    // ---- PlainDate (§3) ----

    fn plain_date_builtin(
        &mut self,
        method: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Undefined);
        let make = |engine: &mut Self, d: PlainDate| {
            engine.temporal_object("PlainDate", None, Slot::PlainDate(d))
        };
        match method {
            "from" => {
                let d = self.to_temporal_date(arg(0), Some(arg(1)))?;
                return make(self, d);
            }
            "compare" => {
                let one = self.to_temporal_date(arg(0), None)?;
                let two = self.to_temporal_date(arg(1), None)?;
                return Ok(ordering_value(one.compare_iso(&two)));
            }
            _ => {}
        }
        let method = method.strip_prefix("prototype.").unwrap_or(method);
        let d = self.this_plain_date(&receiver)?;
        let opt_str = |s: Option<String>| {
            s.map(|s| Value::String(s.into()))
                .unwrap_or(Value::Undefined)
        };
        let opt_num = |n: Option<f64>| n.map(Value::number).unwrap_or(Value::Undefined);
        match method {
            "calendarId" => Ok(Value::String(d.calendar().identifier().into())),
            "era" => Ok(opt_str(d.era().map(|e| e.as_str().to_string()))),
            "eraYear" => Ok(opt_num(d.era_year().map(f64::from))),
            "year" => Ok(Value::number(d.year() as f64)),
            "month" => Ok(Value::number(d.month() as f64)),
            "monthCode" => Ok(Value::String(d.month_code().as_str().to_string().into())),
            "day" => Ok(Value::number(d.day() as f64)),
            "dayOfWeek" => Ok(Value::number(d.day_of_week() as f64)),
            "dayOfYear" => Ok(Value::number(d.day_of_year() as f64)),
            "weekOfYear" => Ok(opt_num(d.week_of_year().map(f64::from))),
            "yearOfWeek" => Ok(opt_num(d.year_of_week().map(f64::from))),
            "daysInWeek" => Ok(Value::number(d.days_in_week() as f64)),
            "daysInMonth" => Ok(Value::number(d.days_in_month() as f64)),
            "daysInYear" => Ok(Value::number(d.days_in_year() as f64)),
            "monthsInYear" => Ok(Value::number(d.months_in_year() as f64)),
            "inLeapYear" => Ok(Value::Bool(d.in_leap_year())),
            "add" | "subtract" => {
                let duration = self.to_duration(arg(0))?;
                let options = self.options_object(arg(1))?;
                let overflow = self.overflow_option(options)?;
                let next = if method == "add" {
                    self.tcall(|| d.add(&duration, Some(overflow)))?
                } else {
                    self.tcall(|| d.subtract(&duration, Some(overflow)))?
                };
                make(self, next)
            }
            "with" => {
                let like = arg(0);
                if !matches!(like, Value::Object(_)) {
                    return self.throw_error("TypeError", "with() argument must be an object");
                }
                self.reject_temporal_like(&like)?;
                let calendar = d.calendar().clone();
                let fields = self.calendar_fields(&calendar, &like, true)?;
                let options = self.options_object(arg(1))?;
                let overflow = self.overflow_option(options)?;
                let next = self.tcall(|| d.with(fields, Some(overflow)))?;
                make(self, next)
            }
            "withCalendar" => {
                let calendar = self.calendar_identifier(arg(0))?;
                make(self, d.with_calendar(calendar))
            }
            "toPlainYearMonth" => {
                let ym = self.tcall(|| d.to_plain_year_month())?;
                self.temporal_object("PlainYearMonth", None, Slot::PlainYearMonth(ym))
            }
            "toPlainMonthDay" => {
                let md = self.tcall(|| d.to_plain_month_day())?;
                self.temporal_object("PlainMonthDay", None, Slot::PlainMonthDay(md))
            }
            "toZonedDateTime" => {
                // §3.3.x Temporal.PlainDate.prototype.toZonedDateTime ( item )
                let item = arg(0);
                let (tz, time) = if let Value::Object(_) = item {
                    if self.temporal_slot(&item).is_some() {
                        (self.time_zone_identifier(item)?, Value::Undefined)
                    } else {
                        let like = self.get_member(item.clone(), "timeZone")?;
                        if matches!(like, Value::Undefined) {
                            (self.time_zone_identifier(item)?, Value::Undefined)
                        } else {
                            let tz = self.time_zone_identifier(like)?;
                            (tz, self.get_member(item, "plainTime")?)
                        }
                    }
                } else {
                    (self.time_zone_identifier(item)?, Value::Undefined)
                };
                let time = match time {
                    Value::Undefined => None,
                    t => Some(self.to_temporal_time(t, None)?),
                };
                let z = self.tcall(|| d.to_zoned_date_time(tz, time))?;
                self.temporal_object("ZonedDateTime", None, Slot::ZonedDateTime(z))
            }
            "toPlainDateTime" => {
                let time = self.time_or_midnight(arg(0))?;
                let dt = self.tcall(|| d.to_plain_date_time(Some(time)))?;
                self.temporal_object("PlainDateTime", None, Slot::PlainDateTime(dt))
            }
            "until" | "since" => {
                let other = self.to_temporal_date(arg(0), None)?;
                if d.calendar().identifier() != other.calendar().identifier() {
                    return self.throw_error("RangeError", "calendars must match");
                }
                let options = self.options_object(arg(1))?;
                let settings = self.difference_settings(options, UnitGroup::Date)?;
                let result = if method == "until" {
                    self.tcall(|| d.until(&other, settings))?
                } else {
                    self.tcall(|| d.since(&other, settings))?
                };
                self.temporal_object("Duration", None, Slot::Duration(result))
            }
            "equals" => {
                let other = self.to_temporal_date(arg(0), None)?;
                Ok(Value::Bool(
                    d.compare_iso(&other) == std::cmp::Ordering::Equal
                        && d.calendar().identifier() == other.calendar().identifier(),
                ))
            }
            "toString" | "toJSON" | "toLocaleString" => {
                let show = if method == "toString" {
                    let options = self.options_object(arg(0))?;
                    self.show_calendar_option(options)?
                } else {
                    DisplayCalendar::Auto
                };
                Ok(Value::String(d.to_ixdtf_string(show).into()))
            }
            "valueOf" => self.throw_error(
                "TypeError",
                "Temporal.PlainDate cannot be converted to a primitive",
            ),
            _ => Err(ScriptError::Unsupported("Temporal.PlainDate method")),
        }
    }

    // ---- PlainTime (§4) ----

    /// ToTemporalTimeRecord(item [, partial]): the six fields read in alphabetical
    /// order, each converted with ToIntegerWithTruncation as soon as it is read;
    /// at least one must be present. Result in constructor order.
    fn time_record(&mut self, item: &Value) -> Result<[Option<f64>; 6]> {
        let mut sorted: Vec<(usize, &str)> =
            TIME_FIELDS.iter().map(|(n, _)| *n).enumerate().collect();
        sorted.sort_by_key(|(_, n)| *n);
        let mut out = [None; 6];
        for (i, name) in sorted {
            let v = self.get_member(item.clone(), name)?;
            if !matches!(v, Value::Undefined) {
                out[i] = Some(self.integer_with_truncation(v)?);
            }
        }
        if out.iter().all(Option::is_none) {
            return self.throw_error("TypeError", "object has no time fields");
        }
        Ok(out)
    }

    /// RegulateTime: constrain clamps each field into range, reject throws.
    fn regulate_time(&mut self, values: [f64; 6], overflow: Overflow) -> Result<PlainTime> {
        let mut v = [0.0; 6];
        for (i, (value, (_, max))) in values.iter().zip(TIME_FIELDS).enumerate() {
            if overflow == Overflow::Reject && !(0.0..=max).contains(value) {
                return self.throw_error("RangeError", "time field out of range");
            }
            v[i] = value.clamp(0.0, max);
        }
        self.tcall(|| {
            PlainTime::try_new(
                v[0] as u8,
                v[1] as u8,
                v[2] as u8,
                v[3] as u16,
                v[4] as u16,
                v[5] as u16,
            )
        })
    }

    /// ToTemporalTime(item [, options]). `options: None` means no options argument.
    fn to_temporal_time(&mut self, item: Value, options: Option<Value>) -> Result<PlainTime> {
        let read_options = |engine: &mut Self| -> Result<Overflow> {
            match options.clone() {
                None => Ok(Overflow::Constrain),
                Some(o) => {
                    let o = engine.options_object(o)?;
                    engine.overflow_option(o)
                }
            }
        };
        if let Value::Object(_) = item {
            match self.temporal_slot(&item).as_deref() {
                Some(Slot::PlainTime(t)) => {
                    let t = *t;
                    read_options(self)?;
                    return Ok(t);
                }
                Some(Slot::PlainDateTime(dt)) => {
                    let t = dt.to_plain_time();
                    read_options(self)?;
                    return Ok(t);
                }
                Some(Slot::ZonedDateTime(z)) => {
                    let t = z.to_plain_time();
                    read_options(self)?;
                    return Ok(t);
                }
                _ => {}
            }
            let record = self.time_record(&item)?;
            let overflow = read_options(self)?;
            return self.regulate_time(record.map(|v| v.unwrap_or(0.0)), overflow);
        }
        let Value::String(s) = item else {
            return self.throw_error("TypeError", "cannot convert value to a Temporal.PlainTime");
        };
        let time = self.tcall(|| PlainTime::from_utf8(s.lossy().as_bytes()))?;
        read_options(self)?;
        Ok(time)
    }

    fn this_plain_time(&mut self, receiver: &Value) -> Result<PlainTime> {
        match self.temporal_slot(receiver).as_deref() {
            Some(Slot::PlainTime(t)) => Ok(*t),
            _ => self.throw_error("TypeError", "receiver is not a Temporal.PlainTime"),
        }
    }

    /// The options of `round` (§4.3.x): a string is the smallestUnit, otherwise
    /// roundingIncrement, roundingMode and the required smallestUnit in that order.
    /// `group` is the unit group the smallestUnit must belong to; `day` is allowed
    /// only where `allow_day` (PlainDateTime/ZonedDateTime).
    fn round_options(&mut self, round_to: Value, allow_day: bool) -> Result<RoundingOptions> {
        let mut opts = RoundingOptions::default();
        let smallest = match round_to {
            Value::Undefined => return self.throw_error("TypeError", "round requires an argument"),
            Value::String(ref s) => match s.lossy().parse::<Unit>() {
                Ok(u) => Some(u),
                Err(_) => {
                    return self.throw_error(
                        "RangeError",
                        &format!("{s} is not a valid value for smallestUnit"),
                    )
                }
            },
            other => {
                let options = self.options_object(other)?;
                opts.increment = self.increment_option(options)?;
                opts.rounding_mode = self.rounding_mode_option(options)?;
                self.unit_option(options, "smallestUnit")?
            }
        };
        let Some(unit) = smallest else {
            return self.throw_error("RangeError", "smallestUnit is required");
        };
        if !(unit.is_time_unit() || allow_day && unit == Unit::Day) {
            return self.throw_error("RangeError", "invalid smallestUnit for round");
        }
        opts.smallest_unit = Some(unit);
        Ok(opts)
    }

    /// The options of a time-bearing `toString` (§4.3.x): fractionalSecondDigits,
    /// roundingMode, smallestUnit (a time unit other than hour).
    fn time_string_options(&mut self, options: Option<Id>) -> Result<ToStringRoundingOptions> {
        let precision = self.digits_option(options)?;
        let rounding_mode = self.rounding_mode_option(options)?;
        let smallest_unit = self.unit_option(options, "smallestUnit")?;
        if smallest_unit.is_some_and(|u| u == Unit::Hour || !u.is_time_unit()) {
            return self.throw_error("RangeError", "invalid smallestUnit for toString");
        }
        Ok(ToStringRoundingOptions {
            precision,
            smallest_unit,
            rounding_mode,
        })
    }

    fn plain_time_builtin(
        &mut self,
        method: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Undefined);
        let make = |engine: &mut Self, t: PlainTime| {
            engine.temporal_object("PlainTime", None, Slot::PlainTime(t))
        };
        match method {
            "from" => {
                let t = self.to_temporal_time(arg(0), Some(arg(1)))?;
                return make(self, t);
            }
            "compare" => {
                let one = self.to_temporal_time(arg(0), None)?;
                let two = self.to_temporal_time(arg(1), None)?;
                return Ok(ordering_value(time_key(&one).cmp(&time_key(&two))));
            }
            _ => {}
        }
        let method = method.strip_prefix("prototype.").unwrap_or(method);
        let t = self.this_plain_time(&receiver)?;
        match method {
            "hour" => Ok(Value::number(t.hour() as f64)),
            "minute" => Ok(Value::number(t.minute() as f64)),
            "second" => Ok(Value::number(t.second() as f64)),
            "millisecond" => Ok(Value::number(t.millisecond() as f64)),
            "microsecond" => Ok(Value::number(t.microsecond() as f64)),
            "nanosecond" => Ok(Value::number(t.nanosecond() as f64)),
            "add" | "subtract" => {
                let duration = self.to_duration(arg(0))?;
                let next = if method == "add" {
                    self.tcall(|| t.add(&duration))?
                } else {
                    self.tcall(|| t.subtract(&duration))?
                };
                make(self, next)
            }
            "with" => {
                let like = arg(0);
                if !matches!(like, Value::Object(_)) {
                    return self.throw_error("TypeError", "with() argument must be an object");
                }
                self.reject_temporal_like(&like)?;
                let partial = self.time_record(&like)?;
                let options = self.options_object(arg(1))?;
                let overflow = self.overflow_option(options)?;
                let current = time_values(&t);
                let merged: [f64; 6] = std::array::from_fn(|i| partial[i].unwrap_or(current[i]));
                let next = self.regulate_time(merged, overflow)?;
                make(self, next)
            }
            "until" | "since" => {
                let other = self.to_temporal_time(arg(0), None)?;
                let options = self.options_object(arg(1))?;
                let settings = self.difference_settings(options, UnitGroup::Time)?;
                let result = if method == "until" {
                    self.tcall(|| t.until(&other, settings))?
                } else {
                    self.tcall(|| t.since(&other, settings))?
                };
                self.temporal_object("Duration", None, Slot::Duration(result))
            }
            "round" => {
                let opts = self.round_options(arg(0), false)?;
                let next = self.tcall(|| t.round(opts))?;
                make(self, next)
            }
            "equals" => {
                let other = self.to_temporal_time(arg(0), None)?;
                Ok(Value::Bool(time_key(&t) == time_key(&other)))
            }
            "toString" | "toJSON" | "toLocaleString" => {
                let options = if method == "toString" {
                    self.options_object(arg(0))?
                } else {
                    None
                };
                let opts = self.time_string_options(options)?;
                Ok(Value::String(
                    (self.tcall(|| t.to_ixdtf_string(opts))?).into(),
                ))
            }
            "valueOf" => self.throw_error(
                "TypeError",
                "Temporal.PlainTime cannot be converted to a primitive",
            ),
            _ => Err(ScriptError::Unsupported("Temporal.PlainTime method")),
        }
    }

    // ---- PlainDateTime (§5) ----

    /// ToTimeRecordOrMidnight.
    fn time_or_midnight(&mut self, item: Value) -> Result<PlainTime> {
        if matches!(item, Value::Undefined) {
            return Ok(PlainTime::default());
        }
        self.to_temporal_time(item, None)
    }

    /// ToTemporalDateTime(item [, options]). `options: None` means no options argument.
    fn to_temporal_date_time(
        &mut self,
        item: Value,
        options: Option<Value>,
    ) -> Result<PlainDateTime> {
        let read_options = |engine: &mut Self| -> Result<Overflow> {
            match options.clone() {
                None => Ok(Overflow::Constrain),
                Some(o) => {
                    let o = engine.options_object(o)?;
                    engine.overflow_option(o)
                }
            }
        };
        if let Value::Object(_) = item {
            match self.temporal_slot(&item).as_deref() {
                Some(Slot::PlainDateTime(dt)) => {
                    let dt = dt.clone();
                    read_options(self)?;
                    return Ok(dt);
                }
                Some(Slot::PlainDate(d)) => {
                    let d = d.clone();
                    read_options(self)?;
                    return self.tcall(|| d.to_plain_date_time(None));
                }
                Some(Slot::ZonedDateTime(z)) => {
                    let dt = z.to_plain_date_time();
                    read_options(self)?;
                    return Ok(dt);
                }
                _ => {}
            }
            let calendar = self.calendar_with_iso_default(&item)?;
            let (fields, time, _) = self.date_time_fields(&calendar, &item, false, true, &[])?;
            let overflow = read_options(self)?;
            // InterpretTemporalDateTimeFields: the date, then RegulateTime.
            let mut partial = PartialDate::new();
            partial.calendar_fields = fields;
            partial.calendar = calendar;
            let date = self.tcall(|| PlainDate::from_partial(partial, Some(overflow)))?;
            let time = self.regulate_time(time.map(|v| v.unwrap_or(0.0)), overflow)?;
            return self.tcall(|| PlainDateTime::from_date_and_time(date, time));
        }
        let Value::String(s) = item else {
            return self.throw_error(
                "TypeError",
                "cannot convert value to a Temporal.PlainDateTime",
            );
        };
        let dt = self.tcall(|| PlainDateTime::from_utf8(s.lossy().as_bytes()))?;
        read_options(self)?;
        Ok(dt)
    }

    fn this_plain_date_time(&mut self, receiver: &Value) -> Result<PlainDateTime> {
        match self.temporal_slot(receiver).as_deref() {
            Some(Slot::PlainDateTime(d)) => Ok(d.clone()),
            _ => self.throw_error("TypeError", "receiver is not a Temporal.PlainDateTime"),
        }
    }

    fn plain_date_time_builtin(
        &mut self,
        method: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Undefined);
        let make = |engine: &mut Self, d: PlainDateTime| {
            engine.temporal_object("PlainDateTime", None, Slot::PlainDateTime(d))
        };
        match method {
            "from" => {
                let d = self.to_temporal_date_time(arg(0), Some(arg(1)))?;
                return make(self, d);
            }
            "compare" => {
                let one = self.to_temporal_date_time(arg(0), None)?;
                let two = self.to_temporal_date_time(arg(1), None)?;
                return Ok(ordering_value(one.compare_iso(&two)));
            }
            _ => {}
        }
        let method = method.strip_prefix("prototype.").unwrap_or(method);
        let d = self.this_plain_date_time(&receiver)?;
        let opt_str = |s: Option<String>| {
            s.map(|s| Value::String(s.into()))
                .unwrap_or(Value::Undefined)
        };
        let opt_num = |n: Option<f64>| n.map(Value::number).unwrap_or(Value::Undefined);
        match method {
            "calendarId" => Ok(Value::String(d.calendar().identifier().into())),
            "era" => Ok(opt_str(d.era().map(|e| e.as_str().to_string()))),
            "eraYear" => Ok(opt_num(d.era_year().map(f64::from))),
            "year" => Ok(Value::number(d.year() as f64)),
            "month" => Ok(Value::number(d.month() as f64)),
            "monthCode" => Ok(Value::String(d.month_code().as_str().to_string().into())),
            "day" => Ok(Value::number(d.day() as f64)),
            "hour" => Ok(Value::number(d.hour() as f64)),
            "minute" => Ok(Value::number(d.minute() as f64)),
            "second" => Ok(Value::number(d.second() as f64)),
            "millisecond" => Ok(Value::number(d.millisecond() as f64)),
            "microsecond" => Ok(Value::number(d.microsecond() as f64)),
            "nanosecond" => Ok(Value::number(d.nanosecond() as f64)),
            "dayOfWeek" => Ok(Value::number(d.day_of_week() as f64)),
            "dayOfYear" => Ok(Value::number(d.day_of_year() as f64)),
            "weekOfYear" => Ok(opt_num(d.week_of_year().map(f64::from))),
            "yearOfWeek" => Ok(opt_num(d.year_of_week().map(f64::from))),
            "daysInWeek" => Ok(Value::number(d.days_in_week() as f64)),
            "daysInMonth" => Ok(Value::number(d.days_in_month() as f64)),
            "daysInYear" => Ok(Value::number(d.days_in_year() as f64)),
            "monthsInYear" => Ok(Value::number(d.months_in_year() as f64)),
            "inLeapYear" => Ok(Value::Bool(d.in_leap_year())),
            "add" | "subtract" => {
                let duration = self.to_duration(arg(0))?;
                let options = self.options_object(arg(1))?;
                let overflow = self.overflow_option(options)?;
                let next = if method == "add" {
                    self.tcall(|| d.add(&duration, Some(overflow)))?
                } else {
                    self.tcall(|| d.subtract(&duration, Some(overflow)))?
                };
                make(self, next)
            }
            "with" => {
                let like = arg(0);
                if !matches!(like, Value::Object(_)) {
                    return self.throw_error("TypeError", "with() argument must be an object");
                }
                self.reject_temporal_like(&like)?;
                let calendar = d.calendar().clone();
                let (fields, time, _) = self.date_time_fields(&calendar, &like, true, true, &[])?;
                let options = self.options_object(arg(1))?;
                let overflow = self.overflow_option(options)?;
                let current = time_values(&d.to_plain_time());
                let merged: [f64; 6] = std::array::from_fn(|i| time[i].unwrap_or(current[i]));
                let t = self.regulate_time(merged, overflow)?;
                let partial = DateTimeFields {
                    calendar_fields: fields,
                    time: full_partial_time(&t),
                };
                let next = self.tcall(|| d.with(partial, Some(overflow)))?;
                make(self, next)
            }
            "withPlainTime" => {
                let time = self.time_or_midnight(arg(0))?;
                let next = self.tcall(|| d.with_time(Some(time)))?;
                make(self, next)
            }
            "withCalendar" => {
                let calendar = self.calendar_identifier(arg(0))?;
                make(self, d.with_calendar(calendar))
            }
            "until" | "since" => {
                let other = self.to_temporal_date_time(arg(0), None)?;
                if d.calendar().identifier() != other.calendar().identifier() {
                    return self.throw_error("RangeError", "calendars must match");
                }
                let options = self.options_object(arg(1))?;
                let settings = self.difference_settings(options, UnitGroup::DateTime)?;
                let result = if method == "until" {
                    self.tcall(|| d.until(&other, settings))?
                } else {
                    self.tcall(|| d.since(&other, settings))?
                };
                self.temporal_object("Duration", None, Slot::Duration(result))
            }
            "round" => {
                let opts = self.round_options(arg(0), true)?;
                let next = self.tcall(|| d.round(opts))?;
                make(self, next)
            }
            "equals" => {
                let other = self.to_temporal_date_time(arg(0), None)?;
                Ok(Value::Bool(
                    d.compare_iso(&other) == std::cmp::Ordering::Equal
                        && d.calendar().identifier() == other.calendar().identifier(),
                ))
            }
            "toPlainDate" => {
                self.temporal_object("PlainDate", None, Slot::PlainDate(d.to_plain_date()))
            }
            "toZonedDateTime" => {
                let tz = self.time_zone_identifier(arg(0))?;
                let options = self.options_object(arg(1))?;
                let disambiguation = self.disambiguation_option(options)?;
                let z = self.tcall(|| d.to_zoned_date_time(tz, disambiguation))?;
                self.temporal_object("ZonedDateTime", None, Slot::ZonedDateTime(z))
            }
            "toPlainTime" => {
                self.temporal_object("PlainTime", None, Slot::PlainTime(d.to_plain_time()))
            }
            "toString" | "toJSON" | "toLocaleString" => {
                let (opts, show) = if method == "toString" {
                    let options = self.options_object(arg(0))?;
                    let show = self.show_calendar_option(options)?;
                    (self.time_string_options(options)?, show)
                } else {
                    (ToStringRoundingOptions::default(), DisplayCalendar::Auto)
                };
                Ok(Value::String(
                    (self.tcall(|| d.to_ixdtf_string(opts, show))?).into(),
                ))
            }
            "valueOf" => self.throw_error(
                "TypeError",
                "Temporal.PlainDateTime cannot be converted to a primitive",
            ),
            _ => Err(ScriptError::Unsupported("Temporal.PlainDateTime method")),
        }
    }

    // ---- Instant (§8) ----

    /// An epoch-nanoseconds BigInt as i128 (None when it does not fit, which is
    /// out of the valid range anyway).
    fn big_i128(big: &Big) -> Option<i128> {
        big.radix_string(10).parse().ok()
    }

    fn i128_value(&mut self, n: i128) -> Result<Value> {
        let Ok(big) = Big::parse(&n.to_string()) else {
            return self.throw_error("RangeError", "epoch nanoseconds out of range");
        };
        self.big_value(big)
    }

    /// Validated epoch nanoseconds → Instant (IsValidEpochNanoseconds, RangeError).
    fn instant_from_big(&mut self, big: &Big) -> Result<Instant> {
        match Self::big_i128(big) {
            Some(n) => self.tcall(|| Instant::try_new(n)),
            None => self.throw_error("RangeError", "epoch nanoseconds out of range"),
        }
    }

    /// ToTemporalTimeZoneIdentifier.
    fn time_zone_identifier(&mut self, value: Value) -> Result<TimeZone> {
        if let Some(Slot::ZonedDateTime(z)) = self.temporal_slot(&value).as_deref() {
            return Ok(z.time_zone().clone());
        }
        let Value::String(s) = value else {
            return self.throw_error("TypeError", "time zone must be a string");
        };
        self.tcall(|| TimeZone::try_from_str(&s.lossy()))
    }

    /// ToTemporalInstant.
    fn to_temporal_instant(&mut self, item: Value) -> Result<Instant> {
        let item = match item {
            Value::Object(_) => {
                match self.temporal_slot(&item).as_deref() {
                    Some(Slot::Instant(i)) => return Ok(*i),
                    Some(Slot::ZonedDateTime(z)) => return Ok(z.to_instant()),
                    _ => {}
                }
                self.to_primitive(item, true)?
            }
            other => other,
        };
        let Value::String(s) = item else {
            return self.throw_error("TypeError", "cannot convert value to a Temporal.Instant");
        };
        self.tcall(|| Instant::from_utf8(s.lossy().as_bytes()))
    }

    /// Date.prototype.toTemporalInstant (§21.4.4.44): convert the internal
    /// clipped millisecond timestamp directly, without calling user methods.
    pub(super) fn date_to_temporal_instant(&mut self, receiver: Value) -> Result<Value> {
        let (_, millis) = self.date_value(receiver)?;
        if !millis.is_finite() {
            return self.throw_error("RangeError", "invalid Date time value");
        }
        let instant = self.tcall(|| Instant::from_epoch_milliseconds(millis as i64))?;
        self.temporal_object("Instant", None, Slot::Instant(instant))
    }

    fn this_instant(&mut self, receiver: &Value) -> Result<Instant> {
        match self.temporal_slot(receiver).as_deref() {
            Some(Slot::Instant(i)) => Ok(*i),
            _ => self.throw_error("TypeError", "receiver is not a Temporal.Instant"),
        }
    }

    fn instant_builtin(
        &mut self,
        method: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Undefined);
        let make = |engine: &mut Self, i: Instant| {
            engine.temporal_object("Instant", None, Slot::Instant(i))
        };
        match method {
            "from" => {
                let i = self.to_temporal_instant(arg(0))?;
                return make(self, i);
            }
            "fromEpochMilliseconds" => {
                // ToNumber, NumberToBigInt (RangeError unless integral), × 10⁶.
                let n = self.to_number(arg(0))?;
                if !n.is_finite() || n.trunc() != n {
                    return self.throw_error("RangeError", "epoch milliseconds must be an integer");
                }
                if n.abs() > 8.64e15 {
                    return self.throw_error("RangeError", "epoch milliseconds out of range");
                }
                let i = self.tcall(|| Instant::from_epoch_milliseconds(n as i64))?;
                return make(self, i);
            }
            "fromEpochNanoseconds" => {
                let big = self.to_bigint(arg(0), false)?;
                let i = self.instant_from_big(&big)?;
                return make(self, i);
            }
            "compare" => {
                let one = self.to_temporal_instant(arg(0))?;
                let two = self.to_temporal_instant(arg(1))?;
                return Ok(ordering_value(one.as_i128().cmp(&two.as_i128())));
            }
            _ => {}
        }
        let method = method.strip_prefix("prototype.").unwrap_or(method);
        let i = self.this_instant(&receiver)?;
        match method {
            "epochMilliseconds" => Ok(Value::number(i.epoch_milliseconds() as f64)),
            "epochNanoseconds" => self.i128_value(i.as_i128()),
            "add" | "subtract" => {
                let duration = self.to_duration(arg(0))?;
                let next = if method == "add" {
                    self.tcall(|| i.add(&duration))?
                } else {
                    self.tcall(|| i.subtract(&duration))?
                };
                make(self, next)
            }
            "until" | "since" => {
                let other = self.to_temporal_instant(arg(0))?;
                let options = self.options_object(arg(1))?;
                let settings = self.difference_settings(options, UnitGroup::Time)?;
                let result = if method == "until" {
                    self.tcall(|| i.until(&other, settings))?
                } else {
                    self.tcall(|| i.since(&other, settings))?
                };
                self.temporal_object("Duration", None, Slot::Duration(result))
            }
            "round" => {
                let opts = self.round_options(arg(0), false)?;
                let next = self.tcall(|| i.round(opts))?;
                make(self, next)
            }
            "equals" => {
                let other = self.to_temporal_instant(arg(0))?;
                Ok(Value::Bool(i.as_i128() == other.as_i128()))
            }
            "toString" | "toJSON" | "toLocaleString" => {
                // §8.3.x: digits, roundingMode, smallestUnit, timeZone are read first;
                // the unit is validated before the time zone is converted.
                let mut opts = ToStringRoundingOptions::default();
                let mut zone = None;
                if method == "toString" {
                    let options = self.options_object(arg(0))?;
                    opts.precision = self.digits_option(options)?;
                    opts.rounding_mode = self.rounding_mode_option(options)?;
                    opts.smallest_unit = self.unit_option(options, "smallestUnit")?;
                    let tz = match options {
                        Some(id) => self.get_member(Value::Object(id), "timeZone")?,
                        None => Value::Undefined,
                    };
                    if opts
                        .smallest_unit
                        .is_some_and(|u| u == Unit::Hour || !u.is_time_unit())
                    {
                        return self.throw_error("RangeError", "invalid smallestUnit for toString");
                    }
                    if !matches!(tz, Value::Undefined) {
                        zone = Some(self.time_zone_identifier(tz)?);
                    }
                }
                Ok(Value::String(
                    (self.tcall(|| i.to_ixdtf_string(zone, opts))?).into(),
                ))
            }
            "toZonedDateTimeISO" => {
                let tz = self.time_zone_identifier(arg(0))?;
                let z = self.tcall(|| i.to_zoned_date_time_iso(tz))?;
                self.temporal_object("ZonedDateTime", None, Slot::ZonedDateTime(z))
            }
            "valueOf" => self.throw_error(
                "TypeError",
                "Temporal.Instant cannot be converted to a primitive",
            ),
            _ => Err(ScriptError::Unsupported("Temporal.Instant method")),
        }
    }

    // ---- ZonedDateTime (§6) ----

    /// GetTemporalDisambiguationOption.
    fn disambiguation_option(&mut self, options: Option<Id>) -> Result<Disambiguation> {
        match self.string_option(options, "disambiguation")? {
            None => Ok(Disambiguation::Compatible),
            Some(s) => match s.as_str() {
                "compatible" => Ok(Disambiguation::Compatible),
                "earlier" => Ok(Disambiguation::Earlier),
                "later" => Ok(Disambiguation::Later),
                "reject" => Ok(Disambiguation::Reject),
                _ => self.throw_error(
                    "RangeError",
                    &format!("{s} is not a valid value for disambiguation"),
                ),
            },
        }
    }

    /// GetTemporalOffsetOption(options, fallback).
    fn offset_option(
        &mut self,
        options: Option<Id>,
        fallback: OffsetDisambiguation,
    ) -> Result<OffsetDisambiguation> {
        match self.string_option(options, "offset")? {
            None => Ok(fallback),
            Some(s) => match s.as_str() {
                "prefer" => Ok(OffsetDisambiguation::Prefer),
                "use" => Ok(OffsetDisambiguation::Use),
                "ignore" => Ok(OffsetDisambiguation::Ignore),
                "reject" => Ok(OffsetDisambiguation::Reject),
                _ => self.throw_error(
                    "RangeError",
                    &format!("{s} is not a valid value for offset"),
                ),
            },
        }
    }

    /// The disambiguation, offset and overflow options, read in that order.
    fn zoned_options(
        &mut self,
        options: Option<Value>,
        fallback: OffsetDisambiguation,
    ) -> Result<(Disambiguation, OffsetDisambiguation, Overflow)> {
        let options = match options {
            None => None,
            Some(o) => self.options_object(o)?,
        };
        let disambiguation = self.disambiguation_option(options)?;
        let offset = self.offset_option(options, fallback)?;
        let overflow = self.overflow_option(options)?;
        Ok((disambiguation, offset, overflow))
    }

    /// ToTemporalZonedDateTime(item [, options]). `options: None` means no options
    /// argument, so nothing is read.
    fn to_temporal_zoned_date_time(
        &mut self,
        item: Value,
        options: Option<Value>,
    ) -> Result<ZonedDateTime> {
        if let Value::Object(_) = item {
            if let Some(Slot::ZonedDateTime(z)) = self.temporal_slot(&item).as_deref() {
                let z = z.clone();
                self.zoned_options(options, OffsetDisambiguation::Reject)?;
                return Ok(z);
            }
            let calendar = self.calendar_with_iso_default(&item)?;
            let (fields, time, extras) =
                self.date_time_fields(&calendar, &item, false, true, &["offset", "timeZone"])?;
            let Value::String(tz) = &extras[1] else {
                return self.throw_error("TypeError", "timeZone is required");
            };
            let tz = self.tcall(|| TimeZone::try_from_str(&tz.lossy()))?;
            let offset = match &extras[0] {
                Value::String(o) => {
                    Some(self.tcall(|| UtcOffset::from_utf8(o.lossy().as_bytes()))?)
                }
                _ => None,
            };
            let (disambiguation, offset_option, overflow) =
                self.zoned_options(options, OffsetDisambiguation::Reject)?;
            // InterpretTemporalDateTimeFields: the date (by the crate), then RegulateTime.
            let mut probe = PartialDate::new();
            probe.calendar_fields = fields.clone();
            probe.calendar = calendar.clone();
            self.tcall(|| PlainDate::from_partial(probe, Some(overflow)))?;
            let time = self.regulate_time(time.map(|v| v.unwrap_or(0.0)), overflow)?;
            let partial = PartialZonedDateTime {
                fields: ZonedDateTimeFields {
                    calendar_fields: fields,
                    time: full_partial_time(&time),
                    offset,
                },
                timezone: Some(tz),
                calendar,
            };
            return self.tcall(|| {
                ZonedDateTime::from_partial(
                    partial,
                    Some(overflow),
                    Some(disambiguation),
                    Some(offset_option),
                )
            });
        }
        let Value::String(s) = item else {
            return self.throw_error(
                "TypeError",
                "cannot convert value to a Temporal.ZonedDateTime",
            );
        };
        let parsed = self.tcall(|| ParsedZonedDateTime::from_utf8(s.lossy().as_bytes()))?;
        let (disambiguation, offset_option, _) =
            self.zoned_options(options, OffsetDisambiguation::Reject)?;
        self.tcall(|| ZonedDateTime::from_parsed(parsed, disambiguation, offset_option))
    }

    fn this_zoned_date_time(&mut self, receiver: &Value) -> Result<ZonedDateTime> {
        match self.temporal_slot(receiver).as_deref() {
            Some(Slot::ZonedDateTime(z)) => Ok(z.clone()),
            _ => self.throw_error("TypeError", "receiver is not a Temporal.ZonedDateTime"),
        }
    }

    fn zoned_date_time_builtin(
        &mut self,
        method: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Undefined);
        let make = |engine: &mut Self, z: ZonedDateTime| {
            engine.temporal_object("ZonedDateTime", None, Slot::ZonedDateTime(z))
        };
        match method {
            "from" => {
                let z = self.to_temporal_zoned_date_time(arg(0), Some(arg(1)))?;
                return make(self, z);
            }
            "compare" => {
                let one = self.to_temporal_zoned_date_time(arg(0), None)?;
                let two = self.to_temporal_zoned_date_time(arg(1), None)?;
                return Ok(ordering_value(one.compare_instant(&two)));
            }
            _ => {}
        }
        let method = method.strip_prefix("prototype.").unwrap_or(method);
        let z = self.this_zoned_date_time(&receiver)?;
        let opt_str = |s: Option<String>| {
            s.map(|s| Value::String(s.into()))
                .unwrap_or(Value::Undefined)
        };
        let opt_num = |n: Option<f64>| n.map(Value::number).unwrap_or(Value::Undefined);
        match method {
            "calendarId" => Ok(Value::String(z.calendar().identifier().into())),
            "timeZoneId" => Ok(Value::String(
                (self.tcall(|| z.time_zone().identifier())?).into(),
            )),
            "era" => Ok(opt_str(z.era().map(|e| e.as_str().to_string()))),
            "eraYear" => Ok(opt_num(z.era_year().map(f64::from))),
            "year" => Ok(Value::number(z.year() as f64)),
            "month" => Ok(Value::number(z.month() as f64)),
            "monthCode" => Ok(Value::String(z.month_code().as_str().to_string().into())),
            "day" => Ok(Value::number(z.day() as f64)),
            "hour" => Ok(Value::number(z.hour() as f64)),
            "minute" => Ok(Value::number(z.minute() as f64)),
            "second" => Ok(Value::number(z.second() as f64)),
            "millisecond" => Ok(Value::number(z.millisecond() as f64)),
            "microsecond" => Ok(Value::number(z.microsecond() as f64)),
            "nanosecond" => Ok(Value::number(z.nanosecond() as f64)),
            "epochMilliseconds" => Ok(Value::number(z.epoch_milliseconds() as f64)),
            "epochNanoseconds" => self.i128_value(z.to_instant().as_i128()),
            "dayOfWeek" => Ok(Value::number(z.day_of_week() as f64)),
            "dayOfYear" => Ok(Value::number(z.day_of_year() as f64)),
            "weekOfYear" => Ok(opt_num(z.week_of_year().map(f64::from))),
            "yearOfWeek" => Ok(opt_num(z.year_of_week().map(f64::from))),
            "hoursInDay" => Ok(Value::number(self.tcall(|| z.hours_in_day())?)),
            "daysInWeek" => Ok(Value::number(z.days_in_week() as f64)),
            "daysInMonth" => Ok(Value::number(z.days_in_month() as f64)),
            "daysInYear" => Ok(Value::number(z.days_in_year() as f64)),
            "monthsInYear" => Ok(Value::number(z.months_in_year() as f64)),
            "inLeapYear" => Ok(Value::Bool(z.in_leap_year())),
            "offsetNanoseconds" => Ok(Value::number(z.offset_nanoseconds() as f64)),
            "offset" => Ok(Value::String(z.offset().into())),
            "with" => {
                // §6.3.x Temporal.ZonedDateTime.prototype.with ( like [ , options ] )
                let like = arg(0);
                if !matches!(like, Value::Object(_)) {
                    return self.throw_error("TypeError", "with() argument must be an object");
                }
                self.reject_temporal_like(&like)?;
                let calendar = z.calendar().clone();
                let (fields, time, extras) =
                    self.date_time_fields(&calendar, &like, true, true, &["offset"])?;
                let (disambiguation, offset_option, overflow) =
                    self.zoned_options(Some(arg(1)), OffsetDisambiguation::Prefer)?;
                let current = time_values(&z.to_plain_time());
                let merged: [f64; 6] = std::array::from_fn(|i| time[i].unwrap_or(current[i]));
                let t = self.regulate_time(merged, overflow)?;
                let offset = match &extras[0] {
                    Value::String(o) => {
                        Some(self.tcall(|| UtcOffset::from_utf8(o.lossy().as_bytes()))?)
                    }
                    _ => None,
                };
                let partial = ZonedDateTimeFields {
                    calendar_fields: fields,
                    time: full_partial_time(&t),
                    offset,
                };
                let next = self.tcall(|| {
                    z.with(
                        partial,
                        Some(disambiguation),
                        Some(offset_option),
                        Some(overflow),
                    )
                })?;
                make(self, next)
            }
            "withPlainTime" => {
                let time = match arg(0) {
                    Value::Undefined => None,
                    t => Some(self.to_temporal_time(t, None)?),
                };
                let next = self.tcall(|| z.with_plain_time(time))?;
                make(self, next)
            }
            "withTimeZone" => {
                let tz = self.time_zone_identifier(arg(0))?;
                let next = self.tcall(|| z.with_timezone(tz))?;
                make(self, next)
            }
            "withCalendar" => {
                let calendar = self.calendar_identifier(arg(0))?;
                make(self, z.with_calendar(calendar))
            }
            "add" | "subtract" => {
                let duration = self.to_duration(arg(0))?;
                let options = self.options_object(arg(1))?;
                let overflow = self.overflow_option(options)?;
                let next = if method == "add" {
                    self.tcall(|| z.add(&duration, Some(overflow)))?
                } else {
                    self.tcall(|| z.subtract(&duration, Some(overflow)))?
                };
                make(self, next)
            }
            "until" | "since" => {
                let other = self.to_temporal_zoned_date_time(arg(0), None)?;
                if z.calendar().identifier() != other.calendar().identifier() {
                    return self.throw_error("RangeError", "calendars must match");
                }
                let options = self.options_object(arg(1))?;
                let settings = self.difference_settings(options, UnitGroup::DateTime)?;
                let result = if method == "until" {
                    self.tcall(|| z.until(&other, settings))?
                } else {
                    self.tcall(|| z.since(&other, settings))?
                };
                self.temporal_object("Duration", None, Slot::Duration(result))
            }
            "round" => {
                let opts = self.round_options(arg(0), true)?;
                let next = self.tcall(|| z.round(opts))?;
                make(self, next)
            }
            "equals" => {
                let other = self.to_temporal_zoned_date_time(arg(0), None)?;
                Ok(Value::Bool(self.tcall(|| z.equals(&other))?))
            }
            "startOfDay" => {
                let next = self.tcall(|| z.start_of_day())?;
                make(self, next)
            }
            "getTimeZoneTransition" => {
                // GetDirectionOption: a string is shorthand for { direction }.
                let direction = match arg(0) {
                    Value::Undefined => {
                        return self.throw_error("TypeError", "direction is required")
                    }
                    Value::String(s) => Some(s.to_string()),
                    other => {
                        let options = self.options_object(other)?;
                        self.string_option(options, "direction")?
                    }
                };
                let direction = match direction.as_deref() {
                    Some("next") => TransitionDirection::Next,
                    Some("previous") => TransitionDirection::Previous,
                    Some(s) => {
                        return self.throw_error(
                            "RangeError",
                            &format!("{s} is not a valid value for direction"),
                        )
                    }
                    None => return self.throw_error("RangeError", "direction is required"),
                };
                match self.tcall(|| z.get_time_zone_transition(direction))? {
                    Some(next) => make(self, next),
                    None => Ok(Value::Null),
                }
            }
            "toInstant" => self.temporal_object("Instant", None, Slot::Instant(z.to_instant())),
            "toPlainDate" => {
                self.temporal_object("PlainDate", None, Slot::PlainDate(z.to_plain_date()))
            }
            "toPlainTime" => {
                self.temporal_object("PlainTime", None, Slot::PlainTime(z.to_plain_time()))
            }
            "toPlainDateTime" => self.temporal_object(
                "PlainDateTime",
                None,
                Slot::PlainDateTime(z.to_plain_date_time()),
            ),
            "toString" | "toJSON" | "toLocaleString" => {
                // Options in alphabetical order: calendarName, fractionalSecondDigits,
                // offset, roundingMode, smallestUnit, timeZoneName.
                let mut show = DisplayCalendar::Auto;
                let mut offset = DisplayOffset::Auto;
                let mut zone = DisplayTimeZone::Auto;
                let mut opts = ToStringRoundingOptions::default();
                if method == "toString" {
                    let options = self.options_object(arg(0))?;
                    show = self.show_calendar_option(options)?;
                    opts.precision = self.digits_option(options)?;
                    offset = match self.string_option(options, "offset")?.as_deref() {
                        None | Some("auto") => DisplayOffset::Auto,
                        Some("never") => DisplayOffset::Never,
                        Some(s) => {
                            return self.throw_error(
                                "RangeError",
                                &format!("{s} is not a valid value for offset"),
                            )
                        }
                    };
                    opts.rounding_mode = self.rounding_mode_option(options)?;
                    opts.smallest_unit = self.unit_option(options, "smallestUnit")?;
                    zone = match self.string_option(options, "timeZoneName")?.as_deref() {
                        None | Some("auto") => DisplayTimeZone::Auto,
                        Some("never") => DisplayTimeZone::Never,
                        Some("critical") => DisplayTimeZone::Critical,
                        Some(s) => {
                            return self.throw_error(
                                "RangeError",
                                &format!("{s} is not a valid value for timeZoneName"),
                            )
                        }
                    };
                    if opts
                        .smallest_unit
                        .is_some_and(|u| u == Unit::Hour || !u.is_time_unit())
                    {
                        return self.throw_error("RangeError", "invalid smallestUnit for toString");
                    }
                }
                Ok(Value::String(
                    (self.tcall(|| z.to_ixdtf_string(offset, zone, show, opts))?).into(),
                ))
            }
            "valueOf" => self.throw_error(
                "TypeError",
                "Temporal.ZonedDateTime cannot be converted to a primitive",
            ),
            _ => Err(ScriptError::Unsupported("Temporal.ZonedDateTime method")),
        }
    }

    // ---- PlainYearMonth (§9) and PlainMonthDay (§10) ----

    /// ToTemporalYearMonth(item [, options]).
    fn to_temporal_year_month(
        &mut self,
        item: Value,
        options: Option<Value>,
    ) -> Result<PlainYearMonth> {
        let read_options = |engine: &mut Self| -> Result<Overflow> {
            match options.clone() {
                None => Ok(Overflow::Constrain),
                Some(o) => {
                    let o = engine.options_object(o)?;
                    engine.overflow_option(o)
                }
            }
        };
        if let Value::Object(_) = item {
            if let Some(Slot::PlainYearMonth(ym)) = self.temporal_slot(&item).as_deref() {
                let ym = ym.clone();
                read_options(self)?;
                return Ok(ym);
            }
            let calendar = self.calendar_with_iso_default(&item)?;
            let (fields, _, _) =
                self.prepare_fields(&calendar, &item, false, &YEAR_MONTH_NAMES, false, &[])?;
            let overflow = read_options(self)?;
            let partial = PartialYearMonth {
                calendar_fields: YearMonthCalendarFields::from(fields),
                calendar,
            };
            return self.tcall(|| PlainYearMonth::from_partial(partial, Some(overflow)));
        }
        let Value::String(s) = item else {
            return self.throw_error(
                "TypeError",
                "cannot convert value to a Temporal.PlainYearMonth",
            );
        };
        let ym = self.tcall(|| PlainYearMonth::from_utf8(s.lossy().as_bytes()))?;
        read_options(self)?;
        Ok(ym)
    }

    /// ToTemporalMonthDay(item [, options]).
    fn to_temporal_month_day(
        &mut self,
        item: Value,
        options: Option<Value>,
    ) -> Result<PlainMonthDay> {
        let read_options = |engine: &mut Self| -> Result<Overflow> {
            match options.clone() {
                None => Ok(Overflow::Constrain),
                Some(o) => {
                    let o = engine.options_object(o)?;
                    engine.overflow_option(o)
                }
            }
        };
        if let Value::Object(_) = item {
            if let Some(Slot::PlainMonthDay(md)) = self.temporal_slot(&item).as_deref() {
                let md = md.clone();
                read_options(self)?;
                return Ok(md);
            }
            let calendar = self.calendar_with_iso_default(&item)?;
            let fields = self.calendar_fields(&calendar, &item, false)?;
            let overflow = read_options(self)?;
            let mut partial = PartialDate::new();
            partial.calendar_fields = fields;
            partial.calendar = calendar;
            return self.tcall(|| PlainMonthDay::from_partial(partial, Some(overflow)));
        }
        let Value::String(s) = item else {
            return self.throw_error(
                "TypeError",
                "cannot convert value to a Temporal.PlainMonthDay",
            );
        };
        let md = self.tcall(|| PlainMonthDay::from_utf8(s.lossy().as_bytes()))?;
        read_options(self)?;
        Ok(md)
    }

    fn plain_year_month_builtin(
        &mut self,
        method: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Undefined);
        let make = |engine: &mut Self, ym: PlainYearMonth| {
            engine.temporal_object("PlainYearMonth", None, Slot::PlainYearMonth(ym))
        };
        match method {
            "from" => {
                let ym = self.to_temporal_year_month(arg(0), Some(arg(1)))?;
                return make(self, ym);
            }
            "compare" => {
                let one = self.to_temporal_year_month(arg(0), None)?;
                let two = self.to_temporal_year_month(arg(1), None)?;
                return Ok(ordering_value(one.compare_iso(&two)));
            }
            _ => {}
        }
        let method = method.strip_prefix("prototype.").unwrap_or(method);
        let ym = match self.temporal_slot(&receiver).as_deref() {
            Some(Slot::PlainYearMonth(ym)) => ym.clone(),
            _ => return self.throw_error("TypeError", "receiver is not a Temporal.PlainYearMonth"),
        };
        let opt_str = |s: Option<String>| {
            s.map(|s| Value::String(s.into()))
                .unwrap_or(Value::Undefined)
        };
        let opt_num = |n: Option<f64>| n.map(Value::number).unwrap_or(Value::Undefined);
        match method {
            "calendarId" => Ok(Value::String(ym.calendar().identifier().into())),
            "era" => Ok(opt_str(ym.era().map(|e| e.as_str().to_string()))),
            "eraYear" => Ok(opt_num(ym.era_year().map(f64::from))),
            "year" => Ok(Value::number(ym.year() as f64)),
            "month" => Ok(Value::number(ym.month() as f64)),
            "monthCode" => Ok(Value::String(ym.month_code().as_str().to_string().into())),
            "daysInYear" => Ok(Value::number(ym.days_in_year() as f64)),
            "daysInMonth" => Ok(Value::number(ym.days_in_month() as f64)),
            "monthsInYear" => Ok(Value::number(ym.months_in_year() as f64)),
            "inLeapYear" => Ok(Value::Bool(ym.in_leap_year())),
            "with" => {
                let like = arg(0);
                if !matches!(like, Value::Object(_)) {
                    return self.throw_error("TypeError", "with() argument must be an object");
                }
                self.reject_temporal_like(&like)?;
                let calendar = ym.calendar().clone();
                let (fields, _, _) =
                    self.prepare_fields(&calendar, &like, true, &YEAR_MONTH_NAMES, false, &[])?;
                let options = self.options_object(arg(1))?;
                let overflow = self.overflow_option(options)?;
                let next =
                    self.tcall(|| ym.with(YearMonthCalendarFields::from(fields), Some(overflow)))?;
                make(self, next)
            }
            "add" | "subtract" => {
                let duration = self.to_duration(arg(0))?;
                let options = self.options_object(arg(1))?;
                let overflow = self.overflow_option(options)?;
                let next = if method == "add" {
                    self.tcall(|| ym.add(&duration, overflow))?
                } else {
                    self.tcall(|| ym.subtract(&duration, overflow))?
                };
                make(self, next)
            }
            "until" | "since" => {
                let other = self.to_temporal_year_month(arg(0), None)?;
                if ym.calendar().identifier() != other.calendar().identifier() {
                    return self.throw_error("RangeError", "calendars must match");
                }
                let options = self.options_object(arg(1))?;
                let settings = self.difference_settings(options, UnitGroup::YearMonth)?;
                let result = if method == "until" {
                    self.tcall(|| ym.until(&other, settings))?
                } else {
                    self.tcall(|| ym.since(&other, settings))?
                };
                self.temporal_object("Duration", None, Slot::Duration(result))
            }
            "equals" => {
                let other = self.to_temporal_year_month(arg(0), None)?;
                Ok(Value::Bool(
                    ym.compare_iso(&other) == std::cmp::Ordering::Equal
                        && ym.calendar().identifier() == other.calendar().identifier(),
                ))
            }
            "toPlainDate" => {
                let item = arg(0);
                if !matches!(item, Value::Object(_)) {
                    return self.throw_error("TypeError", "toPlainDate argument must be an object");
                }
                let calendar = ym.calendar().clone();
                let (fields, _, _) =
                    self.prepare_fields(&calendar, &item, false, &["day"], false, &[])?;
                let date = self.tcall(|| ym.to_plain_date(Some(fields)))?;
                self.temporal_object("PlainDate", None, Slot::PlainDate(date))
            }
            "toString" | "toJSON" | "toLocaleString" => {
                let show = if method == "toString" {
                    let options = self.options_object(arg(0))?;
                    self.show_calendar_option(options)?
                } else {
                    DisplayCalendar::Auto
                };
                Ok(Value::String(ym.to_ixdtf_string(show).into()))
            }
            "valueOf" => self.throw_error(
                "TypeError",
                "Temporal.PlainYearMonth cannot be converted to a primitive",
            ),
            _ => Err(ScriptError::Unsupported("Temporal.PlainYearMonth method")),
        }
    }

    fn plain_month_day_builtin(
        &mut self,
        method: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Undefined);
        let make = |engine: &mut Self, md: PlainMonthDay| {
            engine.temporal_object("PlainMonthDay", None, Slot::PlainMonthDay(md))
        };
        if method == "from" {
            let md = self.to_temporal_month_day(arg(0), Some(arg(1)))?;
            return make(self, md);
        }
        let method = method.strip_prefix("prototype.").unwrap_or(method);
        let md = match self.temporal_slot(&receiver).as_deref() {
            Some(Slot::PlainMonthDay(md)) => md.clone(),
            _ => return self.throw_error("TypeError", "receiver is not a Temporal.PlainMonthDay"),
        };
        match method {
            "calendarId" => Ok(Value::String(md.calendar().identifier().into())),
            "monthCode" => Ok(Value::String(md.month_code().as_str().to_string().into())),
            "day" => Ok(Value::number(md.day() as f64)),
            "with" => {
                let like = arg(0);
                if !matches!(like, Value::Object(_)) {
                    return self.throw_error("TypeError", "with() argument must be an object");
                }
                self.reject_temporal_like(&like)?;
                let calendar = md.calendar().clone();
                let fields = self.calendar_fields(&calendar, &like, true)?;
                let options = self.options_object(arg(1))?;
                let overflow = self.overflow_option(options)?;
                let next = self.tcall(|| md.with(fields, Some(overflow)))?;
                make(self, next)
            }
            "equals" => {
                let other = self.to_temporal_month_day(arg(0), None)?;
                Ok(Value::Bool(
                    md.to_ixdtf_string(DisplayCalendar::Always)
                        == other.to_ixdtf_string(DisplayCalendar::Always),
                ))
            }
            "toPlainDate" => {
                let item = arg(0);
                if !matches!(item, Value::Object(_)) {
                    return self.throw_error("TypeError", "toPlainDate argument must be an object");
                }
                let calendar = md.calendar().clone();
                let (fields, _, _) =
                    self.prepare_fields(&calendar, &item, false, &["year"], false, &[])?;
                let date = self.tcall(|| md.to_plain_date(Some(fields)))?;
                self.temporal_object("PlainDate", None, Slot::PlainDate(date))
            }
            "toString" | "toJSON" | "toLocaleString" => {
                let show = if method == "toString" {
                    let options = self.options_object(arg(0))?;
                    self.show_calendar_option(options)?
                } else {
                    DisplayCalendar::Auto
                };
                Ok(Value::String(md.to_ixdtf_string(show).into()))
            }
            "valueOf" => self.throw_error(
                "TypeError",
                "Temporal.PlainMonthDay cannot be converted to a primitive",
            ),
            _ => Err(ScriptError::Unsupported("Temporal.PlainMonthDay method")),
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
            "Temporal.PlainDate" => {
                // §3.1.1 Temporal.PlainDate ( isoYear, isoMonth, isoDay [ , calendar ] )
                let y = self.integer_with_truncation(arg(0))?;
                let m = self.integer_with_truncation(arg(1))?;
                let d = self.integer_with_truncation(arg(2))?;
                let calendar = match arg(3) {
                    Value::Undefined => Calendar::ISO,
                    Value::String(s) => {
                        self.tcall(|| Calendar::try_from_utf8(s.lossy().as_bytes()))?
                    }
                    _ => return self.throw_error("TypeError", "calendar must be a string"),
                };
                if !(1.0..=12.0).contains(&m) || !(1.0..=31.0).contains(&d) {
                    return self.throw_error("RangeError", "invalid ISO date");
                }
                let date =
                    self.tcall(|| PlainDate::try_new(clamp_i32(y), m as u8, d as u8, calendar))?;
                self.temporal_object("PlainDate", Some(new_target), Slot::PlainDate(date))
            }
            "Temporal.PlainTime" => {
                // §4.1.1 Temporal.PlainTime ( [ hour [ , minute [ , second [ , millisecond
                // [ , microsecond [ , nanosecond ] ] ] ] ] ] )
                let mut values = [0.0; 6];
                for (i, v) in values.iter_mut().enumerate() {
                    let a = arg(i);
                    if !matches!(a, Value::Undefined) {
                        *v = self.integer_with_truncation(a)?;
                    }
                }
                let time = self.regulate_time(values, Overflow::Reject)?;
                self.temporal_object("PlainTime", Some(new_target), Slot::PlainTime(time))
            }
            "Temporal.PlainDateTime" => {
                // §5.1.1 Temporal.PlainDateTime ( isoYear, isoMonth, isoDay [ , hour [ ,
                // minute [ , second [ , millisecond [ , microsecond [ , nanosecond [ ,
                // calendar ] ] ] ] ] ] ] )
                let y = self.integer_with_truncation(arg(0))?;
                let m = self.integer_with_truncation(arg(1))?;
                let d = self.integer_with_truncation(arg(2))?;
                let mut time = [0.0; 6];
                for (i, v) in time.iter_mut().enumerate() {
                    let a = arg(3 + i);
                    if !matches!(a, Value::Undefined) {
                        *v = self.integer_with_truncation(a)?;
                    }
                }
                let calendar = match arg(9) {
                    Value::Undefined => Calendar::ISO,
                    Value::String(s) => {
                        self.tcall(|| Calendar::try_from_utf8(s.lossy().as_bytes()))?
                    }
                    _ => return self.throw_error("TypeError", "calendar must be a string"),
                };
                if !(1.0..=12.0).contains(&m) || !(1.0..=31.0).contains(&d) {
                    return self.throw_error("RangeError", "invalid ISO date");
                }
                let t = self.regulate_time(time, Overflow::Reject)?;
                let dt = self.tcall(|| {
                    PlainDateTime::try_new(
                        clamp_i32(y),
                        m as u8,
                        d as u8,
                        t.hour(),
                        t.minute(),
                        t.second(),
                        t.millisecond(),
                        t.microsecond(),
                        t.nanosecond(),
                        calendar,
                    )
                })?;
                self.temporal_object("PlainDateTime", Some(new_target), Slot::PlainDateTime(dt))
            }
            "Temporal.Instant" => {
                // §8.1.1 Temporal.Instant ( epochNanoseconds )
                let big = self.to_bigint(arg(0), false)?;
                let i = self.instant_from_big(&big)?;
                self.temporal_object("Instant", Some(new_target), Slot::Instant(i))
            }
            "Temporal.ZonedDateTime" => {
                // §6.1.1 Temporal.ZonedDateTime ( epochNanoseconds, timeZone [ , calendar ] )
                let big = self.to_bigint(arg(0), false)?;
                let i = self.instant_from_big(&big)?;
                let Value::String(tz) = arg(1) else {
                    return self.throw_error("TypeError", "time zone must be a string");
                };
                // ParseTimeZoneIdentifier: identifiers only, no ISO strings.
                let tz = self.tcall(|| TimeZone::try_from_identifier_str(&tz.lossy()))?;
                let calendar = match arg(2) {
                    Value::Undefined => Calendar::ISO,
                    Value::String(s) => {
                        self.tcall(|| Calendar::try_from_utf8(s.lossy().as_bytes()))?
                    }
                    _ => return self.throw_error("TypeError", "calendar must be a string"),
                };
                let z = self.tcall(|| ZonedDateTime::try_new_from_instant(i, tz, calendar))?;
                self.temporal_object("ZonedDateTime", Some(new_target), Slot::ZonedDateTime(z))
            }
            "Temporal.PlainYearMonth" => {
                // §9.1.1 Temporal.PlainYearMonth ( isoYear, isoMonth [ , calendar [ ,
                // referenceISODay ] ] )
                let y = self.integer_with_truncation(arg(0))?;
                let m = self.integer_with_truncation(arg(1))?;
                let calendar = match arg(2) {
                    Value::Undefined => Calendar::ISO,
                    Value::String(s) => {
                        self.tcall(|| Calendar::try_from_utf8(s.lossy().as_bytes()))?
                    }
                    _ => return self.throw_error("TypeError", "calendar must be a string"),
                };
                let day = match arg(3) {
                    Value::Undefined => 1.0,
                    v => self.integer_with_truncation(v)?,
                };
                if !(1.0..=12.0).contains(&m) || !(1.0..=31.0).contains(&day) {
                    return self.throw_error("RangeError", "invalid ISO date");
                }
                let ym = self.tcall(|| {
                    PlainYearMonth::try_new(clamp_i32(y), m as u8, Some(day as u8), calendar)
                })?;
                self.temporal_object("PlainYearMonth", Some(new_target), Slot::PlainYearMonth(ym))
            }
            "Temporal.PlainMonthDay" => {
                // §10.1.1 Temporal.PlainMonthDay ( isoMonth, isoDay [ , calendar [ ,
                // referenceISOYear ] ] )
                let m = self.integer_with_truncation(arg(0))?;
                let d = self.integer_with_truncation(arg(1))?;
                let calendar = match arg(2) {
                    Value::Undefined => Calendar::ISO,
                    Value::String(s) => {
                        self.tcall(|| Calendar::try_from_utf8(s.lossy().as_bytes()))?
                    }
                    _ => return self.throw_error("TypeError", "calendar must be a string"),
                };
                let year = match arg(3) {
                    Value::Undefined => 1972.0,
                    v => self.integer_with_truncation(v)?,
                };
                if !(1.0..=12.0).contains(&m) || !(1.0..=31.0).contains(&d) {
                    return self.throw_error("RangeError", "invalid ISO date");
                }
                let md = self.tcall(|| {
                    PlainMonthDay::new_with_overflow(
                        m as u8,
                        d as u8,
                        calendar,
                        Overflow::Reject,
                        Some(clamp_i32(year)),
                    )
                })?;
                self.temporal_object("PlainMonthDay", Some(new_target), Slot::PlainMonthDay(md))
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
        if let Some(method) = rest.strip_prefix("Now.") {
            return self.now_builtin(method, args);
        }
        if let Some(method) = rest.strip_prefix("Duration.") {
            return self.duration_builtin(method, receiver, args);
        }
        if let Some(method) = rest.strip_prefix("PlainDate.") {
            return self.plain_date_builtin(method, receiver, args);
        }
        if let Some(method) = rest.strip_prefix("PlainYearMonth.") {
            return self.plain_year_month_builtin(method, receiver, args);
        }
        if let Some(method) = rest.strip_prefix("PlainMonthDay.") {
            return self.plain_month_day_builtin(method, receiver, args);
        }
        if let Some(method) = rest.strip_prefix("ZonedDateTime.") {
            return self.zoned_date_time_builtin(method, receiver, args);
        }
        if let Some(method) = rest.strip_prefix("Instant.") {
            return self.instant_builtin(method, receiver, args);
        }
        if let Some(method) = rest.strip_prefix("PlainTime.") {
            return self.plain_time_builtin(method, receiver, args);
        }
        if let Some(method) = rest.strip_prefix("PlainDateTime.") {
            return self.plain_date_time_builtin(method, receiver, args);
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
        // §2 The Temporal.Now object: an ordinary object, not a constructor.
        let now = self.new_object(Some(object))?;
        self.runtime.link(env, now)?;
        self.temporal_tag(now, "Temporal.Now")?;
        self.hidden(namespace, "Now", Value::Object(now))?;
        for method in NOW_METHODS {
            let f =
                self.intrinsic_function(env, &format!("Temporal.Now.{method}"), Some(function))?;
            self.temporal_named(f, method, 0)?;
            self.hidden(now, method, Value::Object(f))?;
        }
        Ok(())
    }

    /// §2.2 Temporal.Now functions. The host clock is the system clock and the
    /// system time zone is UTC (SystemTimeZoneIdentifier is implementation-defined).
    fn now_builtin(&mut self, method: &str, args: Vec<Value>) -> Result<Value> {
        let arg = args.into_iter().next().unwrap_or(Value::Undefined);
        let tz = match method {
            "instant" | "timeZoneId" => None,
            _ if matches!(arg, Value::Undefined) => None,
            _ => Some(self.time_zone_identifier(arg)?),
        };
        let now = || Now::new(SystemHost);
        match method {
            "instant" => {
                let i = self.tcall(|| now().instant())?;
                self.temporal_object("Instant", None, Slot::Instant(i))
            }
            "timeZoneId" => {
                let tz = self.tcall(|| now().time_zone())?;
                let id = self.tcall(|| tz.identifier())?;
                Ok(Value::String(id.as_str().into()))
            }
            "zonedDateTimeISO" => {
                let z = self.tcall(|| now().zoned_date_time_iso(tz))?;
                self.temporal_object("ZonedDateTime", None, Slot::ZonedDateTime(z))
            }
            "plainDateTimeISO" => {
                let d = self.tcall(|| now().plain_date_time_iso(tz))?;
                self.temporal_object("PlainDateTime", None, Slot::PlainDateTime(d))
            }
            "plainDateISO" => {
                let d = self.tcall(|| now().plain_date_iso(tz))?;
                self.temporal_object("PlainDate", None, Slot::PlainDate(d))
            }
            "plainTimeISO" => {
                let t = self.tcall(|| now().plain_time_iso(tz))?;
                self.temporal_object("PlainTime", None, Slot::PlainTime(t))
            }
            _ => Err(ScriptError::Unsupported("Temporal.Now builtin")),
        }
    }
}

const NOW_METHODS: [&str; 6] = [
    "instant",
    "timeZoneId",
    "zonedDateTimeISO",
    "plainDateTimeISO",
    "plainDateISO",
    "plainTimeISO",
];

/// Host hooks for Temporal.Now: the system clock and a UTC system time zone.
struct SystemHost;

impl HostClock for SystemHost {
    fn get_host_epoch_nanoseconds(&self) -> TemporalResult<EpochNanoseconds> {
        let ns = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => d.as_nanos() as i128,
            Err(e) => -(e.duration().as_nanos() as i128),
        };
        Ok(EpochNanoseconds::from(ns))
    }
}

impl HostTimeZone for SystemHost {
    fn get_host_time_zone(
        &self,
        _: &(impl temporal_rs::provider::TimeZoneProvider + ?Sized),
    ) -> TemporalResult<TimeZone> {
        Ok(TimeZone::utc())
    }
}

impl HostHooks for SystemHost {}

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
        "PlainDate" => (
            &[("from", 1), ("compare", 2)],
            &[
                "calendarId",
                "era",
                "eraYear",
                "year",
                "month",
                "monthCode",
                "day",
                "dayOfWeek",
                "dayOfYear",
                "weekOfYear",
                "yearOfWeek",
                "daysInWeek",
                "daysInMonth",
                "daysInYear",
                "monthsInYear",
                "inLeapYear",
            ],
            &[
                ("toPlainYearMonth", 0),
                ("toPlainMonthDay", 0),
                ("add", 1),
                ("subtract", 1),
                ("with", 1),
                ("withCalendar", 1),
                ("until", 1),
                ("since", 1),
                ("equals", 1),
                ("toPlainDateTime", 0),
                ("toZonedDateTime", 1),
                ("toString", 0),
                ("toLocaleString", 0),
                ("toJSON", 0),
                ("valueOf", 0),
            ],
        ),
        "PlainDateTime" => (
            &[("from", 1), ("compare", 2)],
            &[
                "calendarId",
                "era",
                "eraYear",
                "year",
                "month",
                "monthCode",
                "day",
                "hour",
                "minute",
                "second",
                "millisecond",
                "microsecond",
                "nanosecond",
                "dayOfWeek",
                "dayOfYear",
                "weekOfYear",
                "yearOfWeek",
                "daysInWeek",
                "daysInMonth",
                "daysInYear",
                "monthsInYear",
                "inLeapYear",
            ],
            &[
                ("with", 1),
                ("withPlainTime", 0),
                ("withCalendar", 1),
                ("add", 1),
                ("subtract", 1),
                ("until", 1),
                ("since", 1),
                ("round", 1),
                ("equals", 1),
                ("toString", 0),
                ("toLocaleString", 0),
                ("toJSON", 0),
                ("valueOf", 0),
                ("toZonedDateTime", 1),
                ("toPlainDate", 0),
                ("toPlainTime", 0),
            ],
        ),
        "PlainYearMonth" => (
            &[("from", 1), ("compare", 2)],
            &[
                "calendarId",
                "era",
                "eraYear",
                "year",
                "month",
                "monthCode",
                "daysInYear",
                "daysInMonth",
                "monthsInYear",
                "inLeapYear",
            ],
            &[
                ("with", 1),
                ("add", 1),
                ("subtract", 1),
                ("until", 1),
                ("since", 1),
                ("equals", 1),
                ("toString", 0),
                ("toLocaleString", 0),
                ("toJSON", 0),
                ("valueOf", 0),
                ("toPlainDate", 1),
            ],
        ),
        "PlainMonthDay" => (
            &[("from", 1)],
            &["calendarId", "monthCode", "day"],
            &[
                ("with", 1),
                ("equals", 1),
                ("toString", 0),
                ("toLocaleString", 0),
                ("toJSON", 0),
                ("valueOf", 0),
                ("toPlainDate", 1),
            ],
        ),
        "ZonedDateTime" => (
            &[("from", 1), ("compare", 2)],
            &[
                "calendarId",
                "timeZoneId",
                "era",
                "eraYear",
                "year",
                "month",
                "monthCode",
                "day",
                "hour",
                "minute",
                "second",
                "millisecond",
                "microsecond",
                "nanosecond",
                "epochMilliseconds",
                "epochNanoseconds",
                "dayOfWeek",
                "dayOfYear",
                "weekOfYear",
                "yearOfWeek",
                "hoursInDay",
                "daysInWeek",
                "daysInMonth",
                "daysInYear",
                "monthsInYear",
                "inLeapYear",
                "offsetNanoseconds",
                "offset",
            ],
            &[
                ("with", 1),
                ("withPlainTime", 0),
                ("withTimeZone", 1),
                ("withCalendar", 1),
                ("add", 1),
                ("subtract", 1),
                ("until", 1),
                ("since", 1),
                ("round", 1),
                ("equals", 1),
                ("toString", 0),
                ("toLocaleString", 0),
                ("toJSON", 0),
                ("valueOf", 0),
                ("startOfDay", 0),
                ("getTimeZoneTransition", 1),
                ("toInstant", 0),
                ("toPlainDate", 0),
                ("toPlainTime", 0),
                ("toPlainDateTime", 0),
            ],
        ),
        "Instant" => (
            &[
                ("from", 1),
                ("fromEpochMilliseconds", 1),
                ("fromEpochNanoseconds", 1),
                ("compare", 2),
            ],
            &["epochMilliseconds", "epochNanoseconds"],
            &[
                ("add", 1),
                ("subtract", 1),
                ("until", 1),
                ("since", 1),
                ("round", 1),
                ("equals", 1),
                ("toString", 0),
                ("toLocaleString", 0),
                ("toJSON", 0),
                ("valueOf", 0),
                ("toZonedDateTimeISO", 1),
            ],
        ),
        "PlainTime" => (
            &[("from", 1), ("compare", 2)],
            &[
                "hour",
                "minute",
                "second",
                "millisecond",
                "microsecond",
                "nanosecond",
            ],
            &[
                ("add", 1),
                ("subtract", 1),
                ("with", 1),
                ("until", 1),
                ("since", 1),
                ("round", 1),
                ("equals", 1),
                ("toString", 0),
                ("toLocaleString", 0),
                ("toJSON", 0),
                ("valueOf", 0),
            ],
        ),
        _ => (&[], &[], &[]),
    }
}

/// The fields of a time in constructor order.
fn full_partial_time(t: &PlainTime) -> PartialTime {
    PartialTime::new()
        .with_hour(Some(t.hour()))
        .with_minute(Some(t.minute()))
        .with_second(Some(t.second()))
        .with_millisecond(Some(t.millisecond()))
        .with_microsecond(Some(t.microsecond()))
        .with_nanosecond(Some(t.nanosecond()))
}

fn time_values(t: &PlainTime) -> [f64; 6] {
    [
        t.hour() as f64,
        t.minute() as f64,
        t.second() as f64,
        t.millisecond() as f64,
        t.microsecond() as f64,
        t.nanosecond() as f64,
    ]
}

/// CompareTimeRecord key: the fields compared lexicographically.
fn time_key(t: &PlainTime) -> (u8, u8, u8, u16, u16, u16) {
    (
        t.hour(),
        t.minute(),
        t.second(),
        t.millisecond(),
        t.microsecond(),
        t.nanosecond(),
    )
}

/// Saturating conversion: out-of-range years fail the crate's ISODateWithinLimits.
/// Clamps a year far outside the representable range to a year with the same
/// position in the 400-year Gregorian cycle, so leap-year checks during overflow
/// handling (e.g. PlainMonthDay's reference year, §10.2.1) still see the right
/// parity; the date is rejected later by the range check if it is used.
fn clamp_year(v: f64) -> i32 {
    const LIMIT: f64 = 1e9;
    if v.abs() <= LIMIT {
        return v as i32;
    }
    let cycle = v.rem_euclid(400.0);
    if v > 0.0 {
        (LIMIT + cycle) as i32
    } else {
        (-LIMIT - 400.0 + cycle) as i32
    }
}

fn clamp_i32(v: f64) -> i32 {
    v.clamp(i32::MIN as f64, i32::MAX as f64) as i32
}

/// Calendars with `era`/`eraYear` fields (CalendarExtraFields, intl402 §15.x).
fn has_eras(calendar: &Calendar) -> bool {
    !matches!(calendar.identifier(), "iso8601" | "chinese" | "dangi")
}

fn ordering_value(o: std::cmp::Ordering) -> Value {
    Value::number(o as i8 as f64)
}
