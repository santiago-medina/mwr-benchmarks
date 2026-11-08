//! Date's host zone is UTC. Calendar arithmetic, formatting and coercion remain separate.
use super::{
    calendar,
    objects::{Descriptor, Key},
    vm::{Engine, ScriptError, Value},
};
use crate::dual::Id;
use std::{
    collections::HashMap,
    time::{SystemTime, UNIX_EPOCH},
};
type Result<T> = std::result::Result<T, ScriptError>;
fn now() -> f64 {
    let value = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_millis() as f64,
        Err(e) => {
            let d = e.duration();
            -(d.as_millis() as f64)
                - if d.subsec_nanos() % 1000000 != 0 {
                    1.0
                } else {
                    0.0
                }
        }
    };
    calendar::clip(value)
}
fn full_year(year: f64) -> f64 {
    let integer = year.trunc();
    if (0.0..=99.0).contains(&integer) {
        1900.0 + integer
    } else {
        year
    }
}
impl Engine<'_> {
    fn date_value(&mut self, value: Value) -> Result<(Id, f64)> {
        if let Value::Object(id) = value {
            if let Some(time) = self.data(id).ok().and_then(|d| d.date) {
                return Ok((id, f64::from_bits(time)));
            }
        }
        self.throw_error("TypeError", "receiver is not a Date")
    }
    fn date_write(&mut self, id: Id, time: f64) -> Result<Value> {
        let time = calendar::clip(time);
        let mut data = self.data(id)?.clone();
        data.date = Some(time.to_bits());
        self.replace_data(id, data)?;
        Ok(Value::number(time))
    }
    fn date_make(&mut self, values: [f64; 7]) -> Result<f64> {
        let bits = values
            .iter()
            .filter(|n| n.is_finite())
            .map(|n| (((n.abs().to_bits() >> 52) & 2047) as usize).saturating_sub(1022))
            .max()
            .unwrap_or(0);
        for _ in 0..bits.saturating_mul(bits) / 256 + 1 {
            self.tick()?;
        }
        Ok(calendar::make(values))
    }
    fn date_parse(&mut self, text: &str) -> Result<f64> {
        for _ in 0..text.len() / 64 + 1 {
            self.tick()?;
        }
        Ok(calendar::parse(text))
    }
    fn date_components(&mut self, args: &[Value]) -> Result<[f64; 7]> {
        let mut values = [f64::NAN, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
        for (i, value) in args.iter().take(7).enumerate() {
            values[i] = self.to_number(value.clone())?;
        }
        values[0] = full_year(values[0]);
        Ok(values)
    }
    pub(super) fn construct_date(&mut self, target: Value, args: Vec<Value>) -> Result<Value> {
        let time = if args.is_empty() {
            now()
        } else if args.len() == 1 {
            let value = args[0].clone();
            let copy = if let Value::Object(id) = value {
                self.data(id).ok().and_then(|d| d.date)
            } else {
                None
            };
            if let Some(time) = copy {
                f64::from_bits(time)
            } else {
                let value = self.to_primitive_hint(value, "default")?;
                if let Value::String(text) = value {
                    self.date_parse(&text)?
                } else {
                    calendar::clip(self.to_number(value)?)
                }
            }
        } else {
            let values = self.date_components(&args)?;
            self.date_make(values)?
        };
        let prototype = self.get_member(target, "prototype")?;
        let prototype = if let Value::Object(id) = prototype {
            id
        } else {
            self.intrinsics.as_ref().unwrap().collection["Date"]
        };
        let id = self.new_object(Some(prototype))?;
        self.protect(&Value::Object(id))?;
        self.date_write(id, time)?;
        Ok(Value::Object(id))
    }
    pub(super) fn bootstrap_dates(
        &mut self,
        env: Id,
        objects: &mut HashMap<String, Id>,
    ) -> Result<()> {
        let i = self.intrinsics.as_ref().unwrap();
        let function = i.function;
        let object = i.object;
        let proto = self.new_object(Some(object))?;
        self.runtime.link(env, proto)?;
        objects.insert("Date.prototype".into(), proto);
        self.intrinsics
            .as_mut()
            .unwrap()
            .collection
            .insert("Date".into(), proto);
        self.define(
            objects["Date"],
            "length",
            Descriptor {
                value: Some(Value::number(7.0)),
                ..Default::default()
            },
        )?;
        for (name, length) in [("now", 0), ("parse", 1), ("UTC", 7)] {
            let full = format!("Date.{name}");
            let f = self.intrinsic_function(env, &full, Some(function))?;
            self.define(
                f,
                "length",
                Descriptor {
                    value: Some(Value::number(length as f64)),
                    ..Default::default()
                },
            )?;
            self.hidden(objects["Date"], name, Value::Object(f))?;
        }
        let mut methods: Vec<(String, usize)> = vec![];
        for suffix in [
            "Date",
            "Day",
            "FullYear",
            "Hours",
            "Milliseconds",
            "Minutes",
            "Month",
            "Seconds",
        ] {
            for prefix in ["get", "getUTC"] {
                methods.push((format!("{prefix}{suffix}"), 0));
            }
        }
        for prefix in ["set", "setUTC"] {
            for (suffix, length) in [
                ("Date", 1),
                ("FullYear", 3),
                ("Hours", 4),
                ("Milliseconds", 1),
                ("Minutes", 3),
                ("Month", 2),
                ("Seconds", 2),
            ] {
                methods.push((format!("{prefix}{suffix}"), length));
            }
        }
        for name in [
            "getTime",
            "getTimezoneOffset",
            "getYear",
            "valueOf",
            "toString",
            "toDateString",
            "toTimeString",
            "toUTCString",
            "toISOString",
            "toLocaleString",
            "toLocaleDateString",
            "toLocaleTimeString",
        ] {
            methods.push((name.into(), 0));
        }
        methods.extend([
            ("setTime".into(), 1),
            ("setYear".into(), 1),
            ("toJSON".into(), 1),
        ]);
        for (method, length) in methods {
            let full = format!("Date.prototype.{method}");
            let f = self.intrinsic_function(env, &full, Some(function))?;
            self.define(
                f,
                "length",
                Descriptor {
                    value: Some(Value::number(length as f64)),
                    ..Default::default()
                },
            )?;
            self.hidden(proto, &method, Value::Object(f))?;
            if method == "toUTCString" {
                self.hidden(proto, "toGMTString", Value::Object(f))?;
            }
        }
        let primitive =
            self.intrinsic_function(env, "Date.prototype.toPrimitive", Some(function))?;
        self.define(
            primitive,
            "name",
            Descriptor {
                value: Some(Value::String("[Symbol.toPrimitive]".into())),
                ..Default::default()
            },
        )?;
        self.define(
            proto,
            Key::Symbol(super::symbols::PRIMITIVE),
            Descriptor {
                value: Some(Value::Object(primitive)),
                configurable: Some(true),
                ..Default::default()
            },
        )?;
        Ok(())
    }
    pub(super) fn date_builtin(
        &mut self,
        name: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let arg = |i| args.get(i).cloned().unwrap_or(Value::Undefined);
        match name {
            "Date" => return Ok(Value::String(calendar::format(now(), "toString"))),
            "Date.now" => return Ok(Value::number(now())),
            "Date.parse" => {
                let text = self.to_string(arg(0))?;
                return self.date_parse(&text).map(Value::number);
            }
            "Date.UTC" => {
                let values = self.date_components(&args)?;
                return self.date_make(values).map(Value::number);
            }
            "Date.prototype.toPrimitive" => {
                if !matches!(receiver, Value::Object(_)) {
                    return self
                        .throw_error("TypeError", "Date primitive conversion requires an object");
                }
                let Value::String(hint) = arg(0) else {
                    return self.throw_error("TypeError", "invalid Date conversion hint");
                };
                return match hint.as_str() {
                    "number" => self.ordinary_to_primitive(receiver, false),
                    "string" | "default" => self.ordinary_to_primitive(receiver, true),
                    _ => self.throw_error("TypeError", "invalid Date conversion hint"),
                };
            }
            "Date.prototype.toJSON" => {
                let object = self.object_coercible(receiver)?;
                self.protect(&Value::Object(object))?;
                let primitive = self.to_primitive(Value::Object(object), false)?;
                if matches!(primitive,Value::Number(n) if !f64::from_bits(n).is_finite()) {
                    return Ok(Value::Null);
                }
                let method = self.get_member(Value::Object(object), "toISOString")?;
                self.protect(&method)?;
                return self.invoke_this(method, Value::Object(object), vec![]);
            }
            _ => {}
        }
        let method = name.rsplit('.').next().unwrap();
        let (id, time) = self.date_value(receiver)?;
        if matches!(method, "valueOf" | "getTime") {
            return Ok(Value::number(time));
        }
        if method.starts_with("to") {
            if method == "toISOString" && time.is_nan() {
                return self.throw_error("RangeError", "invalid Date time value");
            }
            return Ok(Value::String(calendar::format(time, method)));
        }
        if method == "getTimezoneOffset" {
            return Ok(Value::number(if time.is_nan() { f64::NAN } else { 0.0 }));
        }
        if method.starts_with("get") {
            if time.is_nan() {
                return Ok(Value::number(f64::NAN));
            }
            let p = calendar::parts(time);
            let suffix = method
                .strip_prefix("getUTC")
                .or_else(|| method.strip_prefix("get"))
                .unwrap();
            let value = match suffix {
                "Date" => p.day,
                "Day" => p.weekday as i64,
                "FullYear" => p.year,
                "Year" => p.year - 1900,
                "Hours" => p.hour,
                "Milliseconds" => p.millisecond,
                "Minutes" => p.minute,
                "Month" => p.month as i64,
                "Seconds" => p.second,
                _ => return Err(ScriptError::Unsupported("Date getter")),
            };
            return Ok(Value::number(value as f64));
        }
        if method == "setTime" {
            let value = self.to_number(arg(0))?;
            return self.date_write(id, value);
        }
        let suffix = method
            .strip_prefix("setUTC")
            .or_else(|| method.strip_prefix("set"))
            .ok_or(ScriptError::Unsupported("Date method"))?;
        let (start, count) = match suffix {
            "FullYear" => (0, 3),
            "Year" => (0, 1),
            "Month" => (1, 2),
            "Date" => (2, 1),
            "Hours" => (3, 4),
            "Minutes" => (4, 3),
            "Seconds" => (5, 2),
            "Milliseconds" => (6, 1),
            _ => return Err(ScriptError::Unsupported("Date setter")),
        };
        let mut values = calendar::parts(if time.is_nan() { 0.0 } else { time }).values();
        for offset in 0..count {
            if offset == 0 || offset < args.len() {
                values[start + offset] = self.to_number(arg(offset))?;
            }
        }
        if suffix == "Year" {
            if values[0].is_nan() {
                return self.date_write(id, f64::NAN);
            }
            values[0] = full_year(values[0]);
        }
        if time.is_nan() && !matches!(suffix, "FullYear" | "Year") {
            return Ok(Value::number(f64::NAN));
        }
        let time = self.date_make(values)?;
        self.date_write(id, time)
    }
}
