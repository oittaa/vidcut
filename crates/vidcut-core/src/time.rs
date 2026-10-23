use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TimeError {
    #[error("invalid time {0:?}: {1}")]
    Parse(String, &'static str),
    #[error("{0} is not set")]
    Unset(&'static str),
    #[error("relative time {0:?} needs a base here")]
    NoBase(String),
    #[error("invalid frame rate {0:?}")]
    Rate(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rational {
    num: i64,
    den: i64,
}

pub const STANDARD_RATES: [(i64, i64); 13] = [
    (24000, 1001),
    (24, 1),
    (25, 1),
    (30000, 1001),
    (30, 1),
    (48000, 1001),
    (48, 1),
    (50, 1),
    (60000, 1001),
    (60, 1),
    (100, 1),
    (120000, 1001),
    (120, 1),
];

fn gcd(a: i64, b: i64) -> i64 {
    if b == 0 { a.abs() } else { gcd(b, a % b) }
}

impl Rational {
    pub fn new(num: i64, den: i64) -> Result<Self, TimeError> {
        if num <= 0 || den <= 0 {
            return Err(TimeError::Rate(format!("{num}/{den}")));
        }
        let g = gcd(num, den);
        Ok(Self {
            num: num / g,
            den: den / g,
        })
    }

    pub fn num(self) -> i64 {
        self.num
    }

    pub fn den(self) -> i64 {
        self.den
    }

    pub fn as_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }

    pub fn frames_to_secs(self, frames: i64) -> f64 {
        frames as f64 * self.den as f64 / self.num as f64
    }

    pub fn secs_to_frames(self, secs: f64) -> i64 {
        (secs * self.num as f64 / self.den as f64).round() as i64
    }

    pub fn decimal(self) -> String {
        if self.den == 1 {
            return self.num.to_string();
        }
        let s = format!("{:.3}", self.as_f64());
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }

    pub fn is_whole(self) -> bool {
        self.den == 1
    }

    pub fn timecode_base(self) -> i64 {
        ((self.num + self.den / 2) / self.den).max(1)
    }

    pub fn dropped_per_minute(self) -> i64 {
        match (self.den, self.timecode_base()) {
            (1001, b @ (30 | 60)) => b / 15,
            _ => 0,
        }
    }

    pub fn has_drop_frame(self) -> bool {
        self.dropped_per_minute() > 0
    }

    pub fn snap_standard(rate: f64) -> Option<Self> {
        STANDARD_RATES
            .iter()
            .map(|&(n, d)| Self { num: n, den: d })
            .find(|r| ((r.as_f64() - rate) / r.as_f64()).abs() < 0.001)
    }

    pub fn frame_to_sample(self, frame: i64, sample_rate: u32) -> i64 {
        let n = i128::from(frame) * i128::from(sample_rate) * i128::from(self.den);
        let d = i128::from(self.num);
        ((2 * n + d).div_euclid(2 * d)) as i64
    }
}

impl fmt::Display for Rational {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.den == 1 {
            write!(f, "{}", self.num)
        } else {
            write!(f, "{}/{}", self.num, self.den)
        }
    }
}

impl FromStr for Rational {
    type Err = TimeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad = || TimeError::Rate(s.to_string());
        if let Some((n, d)) = s.split_once('/') {
            return Self::new(
                n.trim().parse().map_err(|_| bad())?,
                d.trim().parse().map_err(|_| bad())?,
            );
        }
        if let Ok(n) = s.trim().parse::<i64>() {
            return Self::new(n, 1);
        }
        let v: f64 = s.trim().parse().map_err(|_| bad())?;
        if !(v.is_finite() && v > 0.0) {
            return Err(bad());
        }
        if let Some(r) = Self::snap_standard(v) {
            return Ok(r);
        }
        let decimals = s
            .trim()
            .split_once('.')
            .map_or(0, |(_, frac)| frac.len() as u32);
        let scale = 10i64.pow(decimals.min(6));
        Self::new((v * scale as f64).round() as i64, scale)
    }
}

impl serde::Serialize for Rational {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for Rational {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl schemars::JsonSchema for Rational {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Rational".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "description": "Frame rate as `num/den` (e.g. `30000/1001`), an integer (`25`) or a decimal snapped to a standard rate (`29.97`).",
            "examples": ["30000/1001", "25", "29.97"]
        })
    }
}

pub fn format_timecode(frames: i64, fps: Rational, start_tc: i64, drop: bool) -> String {
    let base = fps.timecode_base();
    let total = frames + start_tc;
    let (sign, t) = if total < 0 {
        ("-", -total)
    } else {
        ("", total)
    };
    let dropped = if drop { fps.dropped_per_minute() } else { 0 };
    let t = label_frames(t, fps, drop);
    let ff = t % base;
    let s = t / base;
    format!(
        "{sign}{:02}:{:02}:{:02}{}{:02}",
        s / 3600,
        (s / 60) % 60,
        s % 60,
        if dropped > 0 { ';' } else { ':' },
        ff
    )
}

pub fn label_frames(frames: i64, fps: Rational, drop: bool) -> i64 {
    let dropped = if drop { fps.dropped_per_minute() } else { 0 };
    if dropped == 0 {
        return frames;
    }
    let base = fps.timecode_base();
    let (per_ten, per_minute) = (base * 600 - dropped * 9, base * 60 - dropped);
    let (tens, rest) = (frames / per_ten, frames % per_ten);
    let minutes = if rest > dropped {
        (rest - dropped) / per_minute
    } else {
        0
    };
    frames + dropped * (9 * tens + minutes)
}

pub fn timecode_frames(h: i64, m: i64, s: i64, f: i64, fps: Rational, drop: bool) -> Option<i64> {
    let base = fps.timecode_base();
    if h < 0 || !(0..60).contains(&m) || !(0..60).contains(&s) || !(0..base).contains(&f) {
        return None;
    }
    let dropped = if drop { fps.dropped_per_minute() } else { 0 };
    if s == 0 && f < dropped && m % 10 != 0 {
        return None;
    }
    let minutes = h * 60 + m;
    Some((minutes * 60 + s) * base + f - dropped * (minutes - minutes / 10))
}

pub fn parse_frame_number(text: &str) -> Option<i64> {
    let text = text.trim();
    if text.contains([':', ';']) {
        return None;
    }
    let (neg, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n = digits.parse::<i64>().ok()?;
    Some(if neg { -n } else { n })
}

pub fn parse_counter(text: &str, fps: Rational, drop: bool, start_tc: i64) -> Option<i64> {
    let text = text.trim();
    let (neg, body) = match text.strip_prefix('-') {
        Some(rest) => (true, rest.trim_start()),
        None => (false, text),
    };
    let fields: Vec<i64> = body
        .split([':', ';'])
        .map(|f| f.trim().parse().ok())
        .collect::<Option<_>>()?;
    let [h, m, s, f] = fields[..] else {
        return None;
    };
    let label = timecode_frames(h, m, s, f, fps, drop)?;
    let frames = if neg { -label } else { label };
    Some(frames - start_tc)
}

pub fn parse_media_timecode(s: &str, fps: Rational) -> Option<i64> {
    let s = s.trim();
    let drop = s.contains([';', '.']);
    let fields: Vec<i64> = s
        .split([':', ';', '.'])
        .map(|f| f.parse().ok())
        .collect::<Option<_>>()?;
    let [h, m, sec, f] = fields[..] else {
        return None;
    };
    let base = fps.timecode_base();
    if fields.iter().any(|v| *v < 0) || m > 59 || sec > 59 || f >= base {
        return None;
    }
    let nominal = ((h * 60 + m) * 60 + sec) * base + f;
    let dropped = if drop { fps.dropped_per_minute() } else { 0 };
    let minutes = h * 60 + m;
    Some(nominal - dropped * (minutes - minutes / 10))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keyword {
    Start,
    End,
    In,
    Out,
    Playhead,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TimeSpec {
    Frames(i64),
    Seconds(f64),
    Timecode(i64, i64, i64, i64, bool),
    Keyword(Keyword),
    Relative(i64, Box<TimeSpec>),
}

impl TimeSpec {
    pub fn parse(s: &str) -> Result<Self, TimeError> {
        let t = s.trim();
        let err = |why| TimeError::Parse(s.to_string(), why);
        if t.is_empty() {
            return Err(err("empty"));
        }
        match t {
            "start" => return Ok(Self::Keyword(Keyword::Start)),
            "end" => return Ok(Self::Keyword(Keyword::End)),
            "in" => return Ok(Self::Keyword(Keyword::In)),
            "out" => return Ok(Self::Keyword(Keyword::Out)),
            "playhead" => return Ok(Self::Keyword(Keyword::Playhead)),
            _ => {}
        }
        if let Some(rest) = t.strip_prefix('+') {
            return Ok(Self::Relative(1, Box::new(Self::parse_absolute(rest, s)?)));
        }
        if let Some(rest) = t.strip_prefix('-') {
            return Ok(Self::Relative(-1, Box::new(Self::parse_absolute(rest, s)?)));
        }
        Self::parse_absolute(t, s)
    }

    fn parse_absolute(t: &str, original: &str) -> Result<Self, TimeError> {
        let err = |why| TimeError::Parse(original.to_string(), why);
        if t.contains([':', ';']) {
            let drop = t.contains(';');
            let fields: Vec<&str> = t.split([':', ';']).collect();
            let int = |f: &str| {
                f.parse::<i64>()
                    .map_err(|_| err("timecode fields must be integers"))
            };
            if fields.len() == 4 {
                let (h, m, sec, fr) = (
                    int(fields[0])?,
                    int(fields[1])?,
                    int(fields[2])?,
                    int(fields[3])?,
                );
                if m >= 60 || sec >= 60 || h < 0 || m < 0 || sec < 0 || fr < 0 {
                    return Err(err("timecode field out of range"));
                }
                return Ok(Self::Timecode(h, m, sec, fr, drop));
            }
            if drop {
                return Err(err("expected drop-frame timecode HH:MM:SS;FF"));
            }
            if fields.len() == 2 || fields.len() == 3 {
                let secs_field = fields[fields.len() - 1];
                let secs: f64 = secs_field
                    .parse()
                    .map_err(|_| err("seconds field must be a number"))?;
                let mut total = secs;
                let mut mult = 60.0;
                for f in fields[..fields.len() - 1].iter().rev() {
                    total += int(f)? as f64 * mult;
                    mult *= 60.0;
                }
                if !(0.0..60.0).contains(&secs) {
                    return Err(err("seconds out of range"));
                }
                return Ok(Self::Seconds(total));
            }
            return Err(err("expected HH:MM:SS:FF or [HH:]MM:SS.mmm"));
        }
        if let Some(n) = t.strip_suffix('f') {
            return n
                .parse::<i64>()
                .map(Self::Frames)
                .map_err(|_| err("frames must be an integer, like 450f"));
        }
        if let Some(n) = t.strip_suffix("ms") {
            let v: f64 = n
                .parse()
                .map_err(|_| err("milliseconds must be a number, like 250ms"))?;
            return Ok(Self::Seconds(v / 1000.0));
        }
        let mut total = 0.0;
        let mut num = String::new();
        let mut any = false;
        for c in t.chars() {
            match c {
                '0'..='9' | '.' => num.push(c),
                'h' | 'm' | 's' => {
                    let v: f64 = num
                        .parse()
                        .map_err(|_| err("expected a number before the unit"))?;
                    total += v * match c {
                        'h' => 3600.0,
                        'm' => 60.0,
                        _ => 1.0,
                    };
                    num.clear();
                    any = true;
                }
                _ => return Err(err("unknown unit; use f, s, ms, m, h or a timecode")),
            }
        }
        if !num.is_empty() {
            return Err(err(
                "number without a unit: write 30f for frames or 30s for seconds",
            ));
        }
        if !any {
            return Err(err("empty"));
        }
        Ok(Self::Seconds(total))
    }

    pub fn is_relative(&self) -> bool {
        matches!(self, Self::Relative(..))
    }
}

impl fmt::Display for TimeSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Frames(n) => write!(f, "{n}f"),
            Self::Seconds(s) => write!(f, "{s}s"),
            Self::Timecode(h, m, s, fr, drop) => {
                let sep = if *drop { ';' } else { ':' };
                write!(f, "{h:02}:{m:02}:{s:02}{sep}{fr:02}")
            }
            Self::Keyword(k) => f.write_str(match k {
                Keyword::Start => "start",
                Keyword::End => "end",
                Keyword::In => "in",
                Keyword::Out => "out",
                Keyword::Playhead => "playhead",
            }),
            Self::Relative(sign, inner) => {
                write!(f, "{}{inner}", if *sign < 0 { '-' } else { '+' })
            }
        }
    }
}

impl FromStr for TimeSpec {
    type Err = TimeError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl serde::Serialize for TimeSpec {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Frames(n) => s.serialize_i64(*n),
            other => s.collect_str(other),
        }
    }
}

impl<'de> serde::Deserialize<'de> for TimeSpec {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Int(i64),
            Str(String),
        }
        match Raw::deserialize(d)? {
            Raw::Int(n) => Ok(Self::Frames(n)),
            Raw::Str(s) => Self::parse(&s).map_err(serde::de::Error::custom),
        }
    }
}

impl schemars::JsonSchema for TimeSpec {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "TimeSpec".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "A time: an integer number of frames, or a string: `450f`, `15s`, `1m30s`, `250ms`, `HH:MM:SS:FF` (timecode; drop-frame in a project that counts so), `HH:MM:SS;FF` (drop-frame, at 29.97 and 59.94), `HH:MM:SS.mmm`, `start`, `end`, `in`, `out`, `playhead`, or a relative `+10f` / `-2s` where the command allows it.",
            "oneOf": [{"type": "integer"}, {"type": "string"}],
            "examples": [450, "450f", "15s", "1m30s", "00:01:00:00", "end", "+10f"]
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DurationSpec(pub TimeSpec);

impl DurationSpec {
    pub fn parse(s: &str) -> Result<Self, TimeError> {
        let t = s.trim();
        let (sign, rest) = if let Some(r) = t.strip_prefix('+') {
            (Some(1), r)
        } else if let Some(r) = t.strip_prefix(['-', '\u{2212}']) {
            (Some(-1), r)
        } else {
            (None, t)
        };
        let rest = rest.trim();
        let length = if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()) {
            TimeSpec::Frames(
                rest.parse()
                    .map_err(|_| TimeError::Parse(s.to_string(), "too many frames"))?,
            )
        } else {
            TimeSpec::parse_absolute(rest, s)?
        };
        Ok(Self(match sign {
            Some(k) => TimeSpec::Relative(k, Box::new(length)),
            None => length,
        }))
    }

    pub fn frames(&self, ctx: &TimeContext, current: i64) -> Result<i64, TimeError> {
        let d = ctx.duration(&self.0)?;
        Ok(if self.0.is_relative() { current + d } else { d })
    }
}

impl fmt::Display for DurationSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl serde::Serialize for DurationSpec {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de> serde::Deserialize<'de> for DurationSpec {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Int(i64),
            Str(String),
        }
        match Raw::deserialize(d)? {
            Raw::Int(n) => Ok(Self(TimeSpec::Frames(n))),
            Raw::Str(s) => Self::parse(&s).map_err(serde::de::Error::custom),
        }
    }
}

impl schemars::JsonSchema for DurationSpec {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "DurationSpec".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "A duration as the viewer's Duration field takes it: a number of frames, without a unit (`48`); a timecode or a time with a unit (`00:00:02:00`, `2s`, `450f`); or `+` or `-` and either, to change the duration by that much (`+12`, `-5`, `+1s`).",
            "oneOf": [{"type": "integer"}, {"type": "string"}],
            "examples": [48, "48", "00:00:02:00", "+12", "-5"]
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RangeSpec {
    pub start: TimeSpec,
    pub end: RangeEnd,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RangeEnd {
    At(TimeSpec),
    Length(TimeSpec),
}

impl RangeSpec {
    pub fn parse(s: &str) -> Result<Self, TimeError> {
        let (a, b) = s
            .split_once("..")
            .ok_or_else(|| TimeError::Parse(s.to_string(), "expected A..B or A..+D"))?;
        let start = TimeSpec::parse(a)?;
        let end = match b.trim().strip_prefix('+') {
            Some(d) => RangeEnd::Length(TimeSpec::parse(d)?),
            None => RangeEnd::At(TimeSpec::parse(b)?),
        };
        Ok(Self { start, end })
    }
}

impl fmt::Display for RangeSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.end {
            RangeEnd::At(b) => write!(f, "{}..{b}", self.start),
            RangeEnd::Length(d) => write!(f, "{}..+{d}", self.start),
        }
    }
}

impl FromStr for RangeSpec {
    type Err = TimeError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl serde::Serialize for RangeSpec {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for RangeSpec {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

impl schemars::JsonSchema for RangeSpec {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "RangeSpec".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "description": "A half-open range `A..B` (B excluded) or `A..+D` (D long), where A, B and D are TimeSpecs; e.g. `00:01:00:00..00:01:10:00`, `5s..+20s`, `in..out`.",
            "examples": ["10s..20s", "5s..+20s", "in..out", "450f..end"]
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TimeContext {
    pub fps: Rational,
    pub start_tc: i64,
    pub drop_frame: bool,
    pub end: i64,
    pub mark_in: Option<i64>,
    pub mark_out: Option<i64>,
    pub playhead: Option<i64>,
}

impl TimeContext {
    pub fn resolve(&self, spec: &TimeSpec) -> Result<i64, TimeError> {
        match spec {
            TimeSpec::Relative(..) => Err(TimeError::NoBase(spec.to_string())),
            other => self.resolve_inner(other),
        }
    }

    pub fn resolve_from(&self, spec: &TimeSpec, base: i64) -> Result<i64, TimeError> {
        match spec {
            TimeSpec::Relative(sign, inner) => Ok(base + sign * self.duration(inner)?),
            other => self.resolve_inner(other),
        }
    }

    pub fn duration(&self, spec: &TimeSpec) -> Result<i64, TimeError> {
        match spec {
            TimeSpec::Frames(n) => Ok(*n),
            TimeSpec::Seconds(s) => Ok(self.fps.secs_to_frames(*s)),
            TimeSpec::Timecode(..) => Ok(self.resolve_inner(spec)? + self.start_tc),
            TimeSpec::Relative(sign, inner) => Ok(sign * self.duration(inner)?),
            TimeSpec::Keyword(_) => Err(TimeError::Parse(
                spec.to_string(),
                "a keyword is not a length",
            )),
        }
    }

    fn resolve_inner(&self, spec: &TimeSpec) -> Result<i64, TimeError> {
        Ok(match spec {
            TimeSpec::Frames(n) => *n,
            TimeSpec::Seconds(s) => self.fps.secs_to_frames(*s),
            TimeSpec::Timecode(h, m, s, f, drop) => {
                let bad = |why| TimeError::Parse(spec.to_string(), why);
                if *f >= self.fps.timecode_base() {
                    return Err(bad("frame field exceeds the frame rate"));
                }
                if *drop && !self.fps.has_drop_frame() {
                    return Err(bad("drop-frame timecode is for 29.97 and 59.94 fps"));
                }
                timecode_frames(*h, *m, *s, *f, self.fps, *drop || self.drop_frame)
                    .ok_or_else(|| bad("drop-frame timecode skips that label"))?
                    - self.start_tc
            }
            TimeSpec::Keyword(Keyword::Start) => 0,
            TimeSpec::Keyword(Keyword::End) => self.end,
            TimeSpec::Keyword(Keyword::In) => {
                self.mark_in.ok_or(TimeError::Unset("timeline In"))?
            }
            TimeSpec::Keyword(Keyword::Out) => {
                self.mark_out.ok_or(TimeError::Unset("timeline Out"))?
            }
            TimeSpec::Keyword(Keyword::Playhead) => self
                .playhead
                .ok_or(TimeError::Unset("playhead (only available live)"))?,
            TimeSpec::Relative(..) => return Err(TimeError::NoBase(spec.to_string())),
        })
    }

    pub fn range(&self, r: &RangeSpec) -> Result<(i64, i64), TimeError> {
        let start = self.resolve(&r.start)?;
        let end = match &r.end {
            RangeEnd::At(b) => self.resolve_from(b, start)?,
            RangeEnd::Length(d) => start + self.duration(d)?,
        };
        Ok((start, end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(fps: &str) -> TimeContext {
        TimeContext {
            fps: fps.parse().unwrap(),
            start_tc: 0,
            drop_frame: false,
            end: 9000,
            mark_in: Some(100),
            mark_out: None,
            playhead: None,
        }
    }

    #[test]
    fn frame_to_sample_is_exact() {
        let ntsc = Rational::new(30000, 1001).unwrap();
        assert_eq!(ntsc.frame_to_sample(5, 48_000), 8008);
        assert_eq!(ntsc.frame_to_sample(30_000, 48_000), 48_048_000);
        let pal = Rational::new(25, 1).unwrap();
        assert_eq!(pal.frame_to_sample(1, 44_100), 1764);
    }

    #[test]
    fn a_rate_below_half_a_frame_still_has_a_timecode_base() {
        let slow = Rational::new(1, 3).unwrap();
        assert_eq!(slow.timecode_base(), 1);
        assert_eq!(format_timecode(0, slow, 0, false), "00:00:00:00");
    }

    #[test]
    fn media_timecodes() {
        let r = |n, d| Rational::new(n, d).unwrap();
        let tc = parse_media_timecode;
        assert_eq!(tc("01:00:00:00", r(25, 1)), Some(90_000));
        assert_eq!(tc("10:00:00:00", r(24, 1)), Some(864_000));
        assert_eq!(tc("00:37:01:00", r(24000, 1001)), Some(53_304));
        assert_eq!(tc("01:00:00:00", r(30000, 1001)), Some(108_000));
        assert_eq!(tc("01:00:00;00", r(30000, 1001)), Some(107_892));
        assert_eq!(tc("00:01:00;02", r(30000, 1001)), Some(1800));
        assert_eq!(tc("00:10:00;00", r(30000, 1001)), Some(17_982));
        assert_eq!(tc("00:01:00;04", r(60000, 1001)), Some(3600));
        assert_eq!(tc("00:00:01;05", r(25, 1)), Some(30));
        assert_eq!(tc("00:01:00;00", r(24000, 1001)), Some(1440));
        for bad in ["", "01:00:00", "01:00:00:25", "a:b:c:d", "00:61:00:00"] {
            assert_eq!(tc(bad, r(25, 1)), None, "{bad:?}");
        }
    }

    #[test]
    fn ranges() {
        let c = ctx("25");
        let r = |s: &str| c.range(&RangeSpec::parse(s).unwrap()).unwrap();
        assert_eq!(r("10s..20s"), (250, 500));
        assert_eq!(r("5s..+20s"), (125, 625));
        assert_eq!(r("in..+4f"), (100, 104));
        assert_eq!(r("450f..end"), (450, 9000));
        assert_eq!(r("100f..+00:00:01:00"), (100, 125));
        assert!(RangeSpec::parse("10s").is_err());
    }

    #[test]
    fn a_frame_number_is_digits() {
        assert_eq!(parse_frame_number("100"), Some(100));
    }

    #[test]
    fn a_timecode_is_not_a_frame_number() {
        assert_eq!(parse_frame_number("00:00:01:00"), None);
    }

    #[test]
    fn a_counter_is_the_frame_under_its_label() {
        let fps = Rational::new(24, 1).unwrap();
        assert_eq!(parse_counter("01:00:00:00", fps, false, 86400), Some(0));
    }

    #[test]
    fn a_skipped_drop_frame_label_is_not_a_counter() {
        let fps = Rational::new(30000, 1001).unwrap();
        assert_eq!(parse_counter("00:01:00;00", fps, true, 0), None);
    }
}
