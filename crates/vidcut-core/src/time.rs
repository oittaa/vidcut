//! Frame-based time: project frame rates, timecode (drop-frame too), and the
//! `TimeSpec`/`RangeSpec` syntax shared by the CLI, JSON-RPC and the GUI.
//!
//! All timeline positions are integer frames at the project frame rate.
//!
//! **TimeSpec** strings (JSON integers are plain frames):
//!
//! | Form | Meaning |
//! |---|---|
//! | `450f` | frames |
//! | `15s`, `1.5s`, `250ms`, `1m30s`, `1h2m3.5s` | duration, rounded to the nearest frame |
//! | `00:01:02:03` | timecode `HH:MM:SS:FF`, relative to the project start timecode; drop-frame in a project that counts so (decision D47) |
//! | `00:01:02;03` | drop-frame timecode `HH:MM:SS;FF` (29.97 and 59.94 only) |
//! | `00:01:02.500` | clock time `HH:MM:SS.mmm` (or `MM:SS.mmm`) |
//! | `start`, `end`, `in`, `out`, `playhead` | timeline start/end, timeline marks, playhead (live only) |
//! | `+10f`, `-2s` | relative to a base given by the command (e.g. the playhead) |
//!
//! Bare numbers in strings are rejected: `30` could mean frames or seconds.
//!
//! **RangeSpec** strings are half-open: `A..B` (B excluded), `A..+D` (D long),
//! `in..out`.

use std::fmt;
use std::str::FromStr;

/// Errors from parsing or resolving times.
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

/// A positive rational number of frames per second, e.g. 30000/1001.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rational {
    num: i64,
    den: i64,
}

/// Frame rates vidcut snaps measured rates to.
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
    /// A positive rate `num/den`, reduced.
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

    /// Seconds spanned by `frames` frames.
    pub fn frames_to_secs(self, frames: i64) -> f64 {
        frames as f64 * self.den as f64 / self.num as f64
    }

    /// Nearest whole frame to `secs` seconds.
    pub fn secs_to_frames(self, secs: f64) -> i64 {
        (secs * self.num as f64 / self.den as f64).round() as i64
    }

    /// The rate as people write it: `30`, `23.976`, `29.97`, `59.94` (at
    /// most three decimals, no trailing zeros). It parses back to this rate
    /// for the standard ones; `to_string` gives the exact `num/den`.
    pub fn decimal(self) -> String {
        if self.den == 1 {
            return self.num.to_string();
        }
        let s = format!("{:.3}", self.as_f64());
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }

    /// Whether the rate is a whole number of frames a second.
    pub fn is_whole(self) -> bool {
        self.den == 1
    }

    /// Whole frames per second used for non-drop-frame timecode (30 for 29.97).
    pub fn timecode_base(self) -> i64 {
        (self.num + self.den / 2) / self.den
    }

    /// The frame numbers drop-frame timecode skips at the start of every
    /// minute but every tenth: 2 at 29.97, 4 at 59.94, none at other rates.
    pub fn dropped_per_minute(self) -> i64 {
        match (self.den, self.timecode_base()) {
            (1001, b @ (30 | 60)) => b / 15,
            _ => 0,
        }
    }

    /// Whether drop-frame timecode is defined at this rate: 29.97 and 59.94.
    pub fn has_drop_frame(self) -> bool {
        self.dropped_per_minute() > 0
    }

    /// The standard rate within 0.1% of `rate`, if any (e.g. 29.97002 → 30000/1001).
    pub fn snap_standard(rate: f64) -> Option<Self> {
        STANDARD_RATES
            .iter()
            .map(|&(n, d)| Self { num: n, den: d })
            .find(|r| ((r.as_f64() - rate) / r.as_f64()).abs() < 0.001)
    }

    /// Sample index (at `sample_rate`) where frame `frame` starts, rounded to
    /// the nearest sample; exact integer arithmetic.
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

    /// Accepts `30000/1001`, `25`, or a decimal like `29.97` (snapped to a
    /// standard rate, or kept exact as written).
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
        // Exact decimal: 12.5 -> 125/10.
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

/// Formats a timeline position as timecode `HH:MM:SS:FF`, offset by the
/// project start timecode (in frames). With `drop` at 29.97 and 59.94 it is
/// drop-frame, `HH:MM:SS;FF` (decision D47).
pub fn format_timecode(frames: i64, fps: Rational, start_tc: i64, drop: bool) -> String {
    let base = fps.timecode_base();
    let total = frames + start_tc;
    let (sign, t) = if total < 0 {
        ("-", -total)
    } else {
        ("", total)
    };
    let dropped = if drop { fps.dropped_per_minute() } else { 0 };
    // Drop-frame: the label counts back in the numbers skipped so far.
    let t = if dropped > 0 {
        let (per_ten, per_minute) = (base * 600 - dropped * 9, base * 60 - dropped);
        let (tens, rest) = (t / per_ten, t % per_ten);
        let minutes = if rest > dropped {
            (rest - dropped) / per_minute
        } else {
            0
        };
        t + dropped * (9 * tens + minutes)
    } else {
        t
    };
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

/// The frame count of timecode `h:m:s:f` at `fps`, counted drop-frame with
/// `drop` at 29.97 and 59.94; `None` for a field out of range or a label
/// drop-frame skips (`00:01:00;00`).
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

/// The frame count of a timecode a media file records (`HH:MM:SS:FF`) at
/// `fps`. A `;` (or `.`) before the frames marks drop-frame timecode, which
/// at 29.97 and 59.94 fps skips frame numbers 0 and 1 (0–3 at 59.94) at the
/// start of every minute but every tenth; at other rates it counts as
/// written.
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
    // Drop-frame is defined for 29.97 and 59.94 only.
    let dropped = if drop { fps.dropped_per_minute() } else { 0 };
    let minutes = h * 60 + m;
    Some(nominal - dropped * (minutes - minutes / 10))
}

/// Named positions a [`TimeSpec`] can refer to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keyword {
    Start,
    End,
    In,
    Out,
    Playhead,
}

/// A parsed time; resolve it to frames with [`TimeContext::resolve`].
#[derive(Debug, Clone, PartialEq)]
pub enum TimeSpec {
    Frames(i64),
    Seconds(f64),
    /// Timecode fields (h, m, s, f), and whether written drop-frame (`;`).
    Timecode(i64, i64, i64, i64, bool),
    Keyword(Keyword),
    /// `+`/`-` offset relative to a command-specific base.
    Relative(i64, Box<TimeSpec>),
}

impl TimeSpec {
    /// Parses the string form (see the module docs).
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
            // A `;` marks drop-frame timecode: `01:00:00;00`, `01;00;00;00`.
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
            // Clock time: [HH:]MM:SS(.fff)
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
        // Composite duration: 1h2m3.5s, 1m30s, 15s, 2m.
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

    /// Whether this is a `+`/`-` relative time.
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

/// A half-open range of timeline frames `[start, end)`.
#[derive(Debug, Clone, PartialEq)]
pub struct RangeSpec {
    pub start: TimeSpec,
    pub end: RangeEnd,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RangeEnd {
    /// `A..B`: ends before B.
    At(TimeSpec),
    /// `A..+D`: D long.
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

/// Everything needed to turn a [`TimeSpec`] into frames.
#[derive(Debug, Clone, Copy)]
pub struct TimeContext {
    pub fps: Rational,
    /// Project start timecode in frames (timecodes are relative to it).
    pub start_tc: i64,
    /// Timecodes count drop-frame (the project's setting, decision D47;
    /// at 29.97 and 59.94 only).
    pub drop_frame: bool,
    /// End of the timeline (exclusive), for `end`.
    pub end: i64,
    pub mark_in: Option<i64>,
    pub mark_out: Option<i64>,
    pub playhead: Option<i64>,
}

impl TimeContext {
    /// Resolves an absolute time to a frame. Relative times need
    /// [`resolve_from`](Self::resolve_from).
    pub fn resolve(&self, spec: &TimeSpec) -> Result<i64, TimeError> {
        match spec {
            TimeSpec::Relative(..) => Err(TimeError::NoBase(spec.to_string())),
            other => self.resolve_inner(other),
        }
    }

    /// Resolves `spec`, treating relative times as offsets from `base`.
    pub fn resolve_from(&self, spec: &TimeSpec, base: i64) -> Result<i64, TimeError> {
        match spec {
            TimeSpec::Relative(sign, inner) => Ok(base + sign * self.duration(inner)?),
            other => self.resolve_inner(other),
        }
    }

    /// A length in frames (e.g. the `D` of `A..+D`, or a trim amount).
    pub fn duration(&self, spec: &TimeSpec) -> Result<i64, TimeError> {
        match spec {
            TimeSpec::Frames(n) => Ok(*n),
            TimeSpec::Seconds(s) => Ok(self.fps.secs_to_frames(*s)),
            TimeSpec::Timecode(..) => {
                // A timecode as a length counts from zero, not from start_tc.
                Ok(self.resolve_inner(spec)? + self.start_tc)
            }
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

    /// Resolves a range to `(start, end)` frames, `end` exclusive.
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
        // 1001 samples-per-frame cycle at 48 kHz: 1601.6 samples per frame.
        assert_eq!(ntsc.frame_to_sample(5, 48_000), 8008);
        assert_eq!(ntsc.frame_to_sample(30_000, 48_000), 48_048_000);
        let pal = Rational::new(25, 1).unwrap();
        assert_eq!(pal.frame_to_sample(1, 44_100), 1764);
    }

    #[test]
    fn media_timecodes() {
        let r = |n, d| Rational::new(n, d).unwrap();
        let tc = parse_media_timecode;
        assert_eq!(tc("01:00:00:00", r(25, 1)), Some(90_000));
        assert_eq!(tc("10:00:00:00", r(24, 1)), Some(864_000));
        // 23.976 counts 24 frames a second, like 24.
        assert_eq!(tc("00:37:01:00", r(24000, 1001)), Some(53_304));
        // Non-drop 29.97: an hour of timecode is 108000 frames...
        assert_eq!(tc("01:00:00:00", r(30000, 1001)), Some(108_000));
        // ...drop-frame skips 2 numbers a minute but every tenth: 107892.
        assert_eq!(tc("01:00:00;00", r(30000, 1001)), Some(107_892));
        assert_eq!(tc("00:01:00;02", r(30000, 1001)), Some(1800));
        assert_eq!(tc("00:10:00;00", r(30000, 1001)), Some(17_982));
        assert_eq!(tc("00:01:00;04", r(60000, 1001)), Some(3600));
        // Drop-frame means nothing at other rates.
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
}
