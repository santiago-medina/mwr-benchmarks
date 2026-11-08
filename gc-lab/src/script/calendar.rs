//! Gregorian arithmetic uses the integer core so large cancelling components do not lose precision.
use super::bigint::Big;
pub(super) const DAY: f64 = 86400000.0;
pub(super) const WEEK: &[&str] = &["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
pub(super) const MONTH: &[&str] = &[
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
pub(super) fn clip(time: f64) -> f64 {
    if !time.is_finite() || time.abs() > 8640000000000000.0 {
        f64::NAN
    } else if time == 0.0 {
        0.0
    } else {
        time.trunc()
    }
}
fn small(n: i64) -> Big {
    let value = Big::from_u64(n.unsigned_abs());
    if n < 0 {
        value.neg()
    } else {
        value
    }
}
fn floor_div(n: &Big, d: u64) -> Big {
    let (q, r) = n.div_rem(&Big::from_u64(d)).unwrap();
    if r.negative {
        q.sub(&Big::from_u64(1))
    } else {
        q
    }
}
fn multiple(n: &Big, d: u64) -> bool {
    n.div_rem(&Big::from_u64(d)).unwrap().1.is_zero()
}
fn unclipped(values: [f64; 7]) -> f64 {
    if values.iter().any(|n| !n.is_finite()) {
        return f64::NAN;
    }
    let values: Vec<_> = values
        .into_iter()
        .map(|n| Big::from_float(n.trunc()).unwrap())
        .collect();
    let month_year = floor_div(&values[1], 12);
    let year = values[0].add(&month_year);
    let month = values[1].sub(&month_year.mul(&small(12))).to_u64().unwrap() as usize;
    let mut days = year.sub(&small(1970)).mul(&small(365));
    days = days
        .add(&floor_div(&year.sub(&small(1969)), 4))
        .sub(&floor_div(&year.sub(&small(1901)), 100))
        .add(&floor_div(&year.sub(&small(1601)), 400));
    let leap = multiple(&year, 4) && (!multiple(&year, 100) || multiple(&year, 400));
    let prior = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334][month]
        + i64::from(leap && month >= 2);
    days = days.add(&small(prior)).add(&values[2]).sub(&small(1));
    let time = days
        .mul(&small(86400000))
        .add(&values[3].mul(&small(3600000)))
        .add(&values[4].mul(&small(60000)))
        .add(&values[5].mul(&small(1000)))
        .add(&values[6]);
    time.to_float()
}
pub(super) fn make(values: [f64; 7]) -> f64 {
    clip(unclipped(values))
}
#[derive(Clone, Copy)]
pub(super) struct Parts {
    pub year: i64,
    pub month: usize,
    pub day: i64,
    pub weekday: usize,
    pub hour: i64,
    pub minute: i64,
    pub second: i64,
    pub millisecond: i64,
}
impl Parts {
    pub fn values(self) -> [f64; 7] {
        [
            self.year as f64,
            self.month as f64,
            self.day as f64,
            self.hour as f64,
            self.minute as f64,
            self.second as f64,
            self.millisecond as f64,
        ]
    }
}
pub(super) fn parts(time: f64) -> Parts {
    let days = (time / DAY).floor() as i64;
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    let within = time.rem_euclid(DAY) as i64;
    Parts {
        year,
        month: month as usize - 1,
        day,
        weekday: (days + 4).rem_euclid(7) as usize,
        hour: within / 3600000,
        minute: within / 60000 % 60,
        second: within / 1000 % 60,
        millisecond: within % 1000,
    }
}
fn year_text(year: i64) -> String {
    if year < 0 {
        format!("-{:04}", year.unsigned_abs())
    } else {
        format!("{year:04}")
    }
}
pub(super) fn format(time: f64, kind: &str) -> String {
    if !time.is_finite() {
        return "Invalid Date".into();
    }
    let p = parts(time);
    let date = format!(
        "{} {} {:02} {}",
        WEEK[p.weekday],
        MONTH[p.month],
        p.day,
        year_text(p.year)
    );
    let clock = format!("{:02}:{:02}:{:02}", p.hour, p.minute, p.second);
    match kind {
        "toISOString" => {
            let year = if (0..=9999).contains(&p.year) {
                format!("{:04}", p.year)
            } else {
                format!(
                    "{}{:06}",
                    if p.year < 0 { "-" } else { "+" },
                    p.year.unsigned_abs()
                )
            };
            format!(
                "{year}-{:02}-{:02}T{clock}.{:03}Z",
                p.month + 1,
                p.day,
                p.millisecond
            )
        }
        "toUTCString" => format!(
            "{}, {:02} {} {} {clock} GMT",
            WEEK[p.weekday],
            p.day,
            MONTH[p.month],
            year_text(p.year)
        ),
        "toDateString" | "toLocaleDateString" => date,
        "toTimeString" | "toLocaleTimeString" => {
            format!("{clock} GMT+0000 (Coordinated Universal Time)")
        }
        _ => format!("{date} {clock} GMT+0000 (Coordinated Universal Time)"),
    }
}
struct Iso<'a> {
    bytes: &'a [u8],
    index: usize,
}
impl Iso<'_> {
    fn take(&mut self, c: u8) -> bool {
        if self.bytes.get(self.index) == Some(&c) {
            self.index += 1;
            true
        } else {
            false
        }
    }
    fn digits(&mut self, count: usize) -> Option<i64> {
        let mut n = 0;
        for _ in 0..count {
            let c = *self.bytes.get(self.index)?;
            if !c.is_ascii_digit() {
                return None;
            }
            self.index += 1;
            n = n * 10 + (c - b'0') as i64;
        }
        Some(n)
    }
}
fn iso(text: &str) -> Option<f64> {
    let mut p = Iso {
        bytes: text.as_bytes(),
        index: 0,
    };
    let sign = if p.take(b'+') {
        1
    } else if p.take(b'-') {
        -1
    } else {
        0
    };
    let year = p.digits(if sign == 0 { 4 } else { 6 })?;
    if sign < 0 && year == 0 {
        return None;
    }
    let year = if sign < 0 { -year } else { year };
    let mut month = 1;
    let mut day = 1;
    let mut full = false;
    if p.take(b'-') {
        month = p.digits(2)?;
        if p.take(b'-') {
            day = p.digits(2)?;
            full = true;
        }
    }
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut hour = 0;
    let mut minute = 0;
    let mut second = 0;
    let mut millisecond = 0;
    let mut offset = 0;
    if p.take(b'T') {
        if !full {
            return None;
        }
        hour = p.digits(2)?;
        if !p.take(b':') {
            return None;
        }
        minute = p.digits(2)?;
        if p.take(b':') {
            second = p.digits(2)?;
            if p.take(b'.') {
                millisecond = p.digits(3)?;
            }
        }
        if hour > 24
            || minute > 59
            || second > 59
            || hour == 24 && (minute != 0 || second != 0 || millisecond != 0)
        {
            return None;
        }
        if !p.take(b'Z') {
            let sign = if p.take(b'+') {
                1
            } else if p.take(b'-') {
                -1
            } else {
                0
            };
            if sign != 0 {
                let h = p.digits(2)?;
                if !p.take(b':') {
                    return None;
                }
                let m = p.digits(2)?;
                if h > 23 || m > 59 {
                    return None;
                }
                offset = sign * (h * 60 + m);
            }
        }
    }
    if p.index != p.bytes.len() {
        return None;
    }
    let value = unclipped([
        year as f64,
        (month - 1) as f64,
        day as f64,
        hour as f64,
        minute as f64,
        second as f64,
        millisecond as f64,
    ]);
    Some(clip(value - offset as f64 * 60000.0))
}
pub(super) fn parse(text: &str) -> f64 {
    let text = text.trim_matches(super::strings::whitespace);
    if let Some(value) = iso(text) {
        return value;
    }
    // Support the engine's own human-readable and UTC formats for Date.parse round trips.
    let words: Vec<_> = text.split_whitespace().collect();
    if words.len() < 6 {
        return f64::NAN;
    }
    let (day, month, year, clock, zone) = if words[0].ends_with(',') {
        (words[1], words[2], words[3], words[4], words[5])
    } else {
        (words[2], words[1], words[3], words[4], words[5])
    };
    if !matches!(zone, "GMT" | "GMT+0000") {
        return f64::NAN;
    }
    let Some(month) = MONTH.iter().position(|m| *m == month) else {
        return f64::NAN;
    };
    let (Ok(day), Ok(year)) = (day.parse::<f64>(), year.parse::<f64>()) else {
        return f64::NAN;
    };
    let clock: Vec<_> = clock.split(':').collect();
    if clock.len() != 3 {
        return f64::NAN;
    }
    let (Ok(hour), Ok(minute), Ok(second)) = (
        clock[0].parse::<f64>(),
        clock[1].parse::<f64>(),
        clock[2].parse::<f64>(),
    ) else {
        return f64::NAN;
    };
    if year.abs() > 999999.0
        || !(1.0..=31.0).contains(&day)
        || !(0.0..=23.0).contains(&hour)
        || !(0.0..=59.0).contains(&minute)
        || !(0.0..=59.0).contains(&second)
    {
        return f64::NAN;
    }
    make([year, month as f64, day, hour, minute, second, 0.0])
}
