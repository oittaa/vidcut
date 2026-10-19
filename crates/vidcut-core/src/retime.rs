use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::transcript::{self, Said};

pub const SPEECH: f64 = 0.5;
pub const SHORTEST_SPEECH: f64 = 0.25;
pub const SHORTEST_PAUSE: f64 = 0.1;
pub const MAX_OFFSET: f64 = 60.0;
pub const CLEAR: f64 = 1.25;
pub const FIT: f64 = 0.5;
pub const RUN: usize = 3;
pub const MIN_ANCHORS: usize = 10;
const STEP: f64 = 0.01;
const APART: f64 = 2.0;
const LOBE: f64 = 1.0;
const SLOWEST: f64 = 0.95;
const FASTEST: f64 = 1.05;
const DRIFT: f64 = 0.25;
const SECTION: usize = 5;
const PAIRED: usize = 1000;

const RATIOS: [(f64, &str); 6] = [
    (1000.0 / 1001.0, "23.976/24"),
    (1001.0 / 1000.0, "24/23.976"),
    (24.0 / 25.0, "24/25"),
    (25.0 / 24.0, "25/24"),
    (24000.0 / 1001.0 / 25.0, "23.976/25"),
    (25.0 * 1001.0 / 24000.0, "25/23.976"),
];

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct LinedUp {
    pub offset: f64,
    pub speed: f64,
    pub ratio: Option<&'static str>,
    pub peak: f64,
    pub rival: f64,
    pub rival_offset: f64,
    pub clarity: Option<f64>,
    pub clear: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unlined {
    Silent,
    Throughout,
}

pub type Lined = Result<LinedUp, Unlined>;

struct Ramps {
    lo: f64,
    second: Vec<f64>,
    level: f64,
}

impl Ramps {
    fn add(&mut self, at: f64, w: f64) {
        if at <= self.lo {
            self.second[0] += w;
            self.level += w * (self.lo - at);
            return;
        }
        let x = (at - self.lo) / STEP;
        let i = x.floor() as usize;
        if i >= self.second.len() {
            return;
        }
        let part = x - i as f64;
        self.second[i] += w * (1.0 - part);
        if let Some(next) = self.second.get_mut(i + 1) {
            *next += w * part;
        }
    }

    fn overlap(&mut self, (a, b): (f64, f64), (c, e): (f64, f64), w: f64) {
        self.add(a - e, w);
        self.add(a - c, -w);
        self.add(b - e, -w);
        self.add(b - c, w);
    }

    fn values(self) -> Vec<f64> {
        let mut out = Vec::with_capacity(self.second.len());
        let (mut v, mut slope) = (self.level, 0.0);
        for s in &self.second {
            out.push(v);
            slope += s;
            v += STEP * slope;
        }
        out
    }
}

struct Shows<'a> {
    speech: &'a [(f64, f64)],
    mean: f64,
    extent: (f64, f64),
    cues: &'a [(f64, f64)],
    max: f64,
    spread: f64,
}

impl Shows<'_> {
    fn steps(&self) -> usize {
        (2.0 * self.max / STEP).round() as usize + 1
    }

    fn shift(&self, k: usize) -> f64 {
        -self.max + k as f64 * STEP
    }

    fn curve(&self, speed: f64) -> Vec<f64> {
        let n = self.steps();
        let (lo, hi) = (self.shift(0), self.shift(n - 1));
        let mut r = Ramps {
            lo,
            second: vec![0.0; n],
            level: 0.0,
        };
        let mut shown = 0.0;
        for &(c, e) in self.cues {
            let (c, e) = (c * speed, e * speed);
            shown += e - c;
            let first = self.speech.partition_point(|s| s.1 <= c + lo);
            for &s in self.speech[first..].iter().take_while(|s| s.0 < e + hi) {
                r.overlap(s, (c, e), 1.0);
            }
            r.overlap(self.extent, (c, e), -self.mean);
        }
        let len = self.extent.1 - self.extent.0;
        let norm = self.spread * (shown - shown * shown / len).max(0.0).sqrt();
        let mut v = r.values();
        if norm > 0.0 {
            for x in &mut v {
                *x /= norm;
            }
        }
        v
    }

    fn peak(&self, speed: f64) -> f64 {
        self.curve(speed).into_iter().fold(f64::MIN, f64::max)
    }
}

fn span(cues: &[(f64, f64)]) -> f64 {
    let first = cues.iter().map(|c| c.0).fold(f64::MAX, f64::min);
    let last = cues.iter().map(|c| c.1).fold(f64::MIN, f64::max);
    (last - first).max(1.0)
}

pub fn snap(speed: f64, span: f64) -> (f64, Option<&'static str>) {
    let near = |r: f64| (speed - r).abs() * span <= DRIFT;
    if near(1.0) {
        return (1.0, None);
    }
    RATIOS
        .iter()
        .filter(|(r, _)| near(*r))
        .min_by(|a, b| (speed - a.0).abs().total_cmp(&(speed - b.0).abs()))
        .map_or((speed, None), |&(r, name)| (r, Some(name)))
}

fn rival(curve: &[f64], best: usize) -> usize {
    let apart = (APART / STEP).round() as usize;
    let lobe = (LOBE / STEP).round() as usize;
    let far = |j: &usize| j.abs_diff(best) > apart;
    let local = |j: &usize| {
        let (a, b) = (j.saturating_sub(lobe), (j + lobe + 1).min(curve.len()));
        curve[a..b].iter().all(|&x| x <= curve[*j])
    };
    let highest =
        |js: &mut dyn Iterator<Item = usize>| js.max_by(|&a, &b| curve[a].total_cmp(&curve[b]));
    highest(&mut (0..curve.len()).filter(far).filter(local))
        .or_else(|| highest(&mut (0..curve.len()).filter(far)))
        .unwrap_or(best)
}

pub fn line_up(
    speech: &[(f64, f64)],
    extent: (f64, f64),
    cues: &[(f64, f64)],
    max_offset: f64,
) -> Lined {
    let len = extent.1 - extent.0;
    let spoken: f64 = speech
        .iter()
        .map(|&(a, b)| b.min(extent.1) - a.max(extent.0))
        .filter(|d| *d > 0.0)
        .sum();
    if cues.is_empty() || len <= 0.0 || spoken <= 0.0 {
        return Err(Unlined::Silent);
    }
    if spoken >= len {
        return Err(Unlined::Throughout);
    }
    let mean = spoken / len;
    let shows = Shows {
        speech,
        mean,
        extent,
        cues,
        max: max_offset.max(STEP),
        spread: (len * mean * (1.0 - mean)).sqrt(),
    };
    let span = span(cues);
    let coarse = (DRIFT / span).clamp(1e-5, 1e-3);
    let still = shows.peak(1.0);
    let (mut best, mut speed) = (still, 1.0);
    let steps = ((FASTEST - SLOWEST) / coarse).ceil() as usize;
    for k in 0..=steps {
        let s = SLOWEST + k as f64 * coarse;
        let p = shows.peak(s);
        if p > best {
            (best, speed) = (p, s);
        }
    }
    let around = speed;
    for k in -10..=10 {
        let s = around + f64::from(k) * coarse / 10.0;
        let p = shows.peak(s);
        if p > best {
            (best, speed) = (p, s);
        }
    }
    let (mut speed, mut ratio) = snap(speed, span);
    if speed != 1.0 && shows.peak(speed) <= still {
        (speed, ratio) = (1.0, None);
    }
    let curve = shows.curve(speed);
    let k = (0..curve.len())
        .max_by(|&a, &b| curve[a].total_cmp(&curve[b]))
        .unwrap_or(0);
    let j = rival(&curve, k);
    let (peak, other) = (curve[k], curve[j]);
    Ok(LinedUp {
        offset: shows.shift(k),
        speed,
        ratio,
        peak,
        rival: other,
        rival_offset: shows.shift(j),
        clarity: (other > 0.0).then(|| peak / other),
        clear: peak > 0.0 && (other <= 0.0 || peak >= CLEAR * other),
    })
}

pub fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch.is_alphanumeric() {
            word.extend(ch.to_lowercase());
        } else if matches!(ch, '\'' | '\u{2019}')
            && !word.is_empty()
            && chars.peek().is_some_and(|c| c.is_alphanumeric())
        {
            word.push('\'');
        } else if !word.is_empty() {
            out.push(std::mem::take(&mut word));
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct Spoken {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

pub fn spoken(said: &[Said]) -> Vec<Spoken> {
    let mut out: Vec<Spoken> = Vec::new();
    for w in said.iter().flat_map(transcript::words) {
        match out.last_mut() {
            Some(last) if w.joined => {
                last.text.push_str(&w.text);
                last.end = last.end.max(w.end);
            }
            _ => out.push(Spoken {
                start: w.start,
                end: w.end,
                text: w.text,
            }),
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchor {
    pub cue: usize,
    pub start: f64,
    pub end: Option<f64>,
}

fn longest(
    a: &[u32],
    at: &HashMap<u32, Vec<usize>>,
    (alo, ahi, blo, bhi): (usize, usize, usize, usize),
) -> (usize, usize, usize) {
    let (mut bi, mut bj, mut bk) = (alo, blo, 0);
    let mut prev: HashMap<usize, usize> = HashMap::new();
    for (i, t) in a.iter().enumerate().take(ahi).skip(alo) {
        let mut next = HashMap::new();
        if let Some(js) = at.get(t) {
            let from = js.partition_point(|&j| j < blo);
            for &j in js[from..].iter().take_while(|&&j| j < bhi) {
                let k = j
                    .checked_sub(1)
                    .and_then(|p| prev.get(&p))
                    .copied()
                    .unwrap_or(0)
                    + 1;
                next.insert(j, k);
                if k > bk {
                    (bi, bj, bk) = (i + 1 - k, j + 1 - k, k);
                }
            }
        }
        prev = next;
    }
    (bi, bj, bk)
}

fn blocks(a: &[u32], b: &[u32]) -> Vec<(usize, usize, usize)> {
    let mut at: HashMap<u32, Vec<usize>> = HashMap::new();
    for (j, &t) in b.iter().enumerate() {
        at.entry(t).or_default().push(j);
    }
    let mut out = Vec::new();
    let mut todo = vec![(0, a.len(), 0, b.len())];
    while let Some(range) = todo.pop() {
        let (alo, ahi, blo, bhi) = range;
        let (i, j, k) = longest(a, &at, range);
        if k == 0 {
            continue;
        }
        out.push((i, j, k));
        if alo < i && blo < j {
            todo.push((alo, i, blo, j));
        }
        if i + k < ahi && j + k < bhi {
            todo.push((i + k, ahi, j + k, bhi));
        }
    }
    out.sort_unstable();
    out
}

pub fn anchors(cues: &[String], words: &[Spoken]) -> Vec<Anchor> {
    let mut ids: HashMap<String, u32> = HashMap::new();
    let mut id = |t: String| {
        let n = ids.len() as u32;
        *ids.entry(t).or_insert(n)
    };
    let (mut a, mut of) = (Vec::new(), Vec::new());
    for (ci, text) in cues.iter().enumerate() {
        let toks = tokens(text);
        let last = toks.len().saturating_sub(1);
        for (k, t) in toks.into_iter().enumerate() {
            a.push(id(t));
            of.push((ci, k == 0, k == last));
        }
    }
    let (mut b, mut times) = (Vec::new(), Vec::new());
    for w in words {
        for t in tokens(&w.text) {
            b.push(id(t));
            times.push((w.start, w.end));
        }
    }
    let mut found: HashMap<usize, Anchor> = HashMap::new();
    for (i, j, k) in blocks(&a, &b) {
        if k < RUN {
            continue;
        }
        let mut begun = HashSet::new();
        for m in 0..k {
            let (cue, first, last) = of[i + m];
            let (start, end) = times[j + m];
            if first {
                begun.insert(cue);
                found.insert(
                    cue,
                    Anchor {
                        cue,
                        start,
                        end: None,
                    },
                );
            }
            if last
                && begun.contains(&cue)
                && let Some(x) = found.get_mut(&cue)
                && end > x.start
            {
                x.end = Some(end);
            }
        }
    }
    let mut out: Vec<Anchor> = found.into_values().collect();
    out.sort_by_key(|x| x.cue);
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Offset,
    Speed,
    Sections,
    Words,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Section {
    pub from: f64,
    pub offset: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Plan {
    pub kind: Kind,
    pub speed: f64,
    pub ratio: Option<&'static str>,
    pub sections: Vec<Section>,
    #[serde(skip)]
    pub snapped: Vec<Anchor>,
}

impl Plan {
    pub fn lined(l: &LinedUp, cues: &[(f64, f64)]) -> Self {
        Self {
            kind: if l.speed == 1.0 {
                Kind::Offset
            } else {
                Kind::Speed
            },
            speed: l.speed,
            ratio: l.ratio,
            sections: vec![Section {
                from: cues.first().map_or(0.0, |c| c.0),
                offset: l.offset,
            }],
            snapped: Vec::new(),
        }
    }

    fn section_at(&self, at: f64) -> usize {
        self.sections
            .partition_point(|s| s.from <= at)
            .saturating_sub(1)
    }

    fn offset_at(&self, at: f64) -> f64 {
        self.sections[self.section_at(at)].offset
    }

    pub fn counts(&self, cues: &[(f64, f64)]) -> Vec<usize> {
        let mut n = vec![0; self.sections.len()];
        for c in cues {
            n[self.section_at(c.0)] += 1;
        }
        n
    }

    pub fn map(&self, cue: usize, (start, end): (f64, f64)) -> (f64, f64) {
        let length = self.speed * (end - start);
        if let Ok(i) = self.snapped.binary_search_by_key(&cue, |a| a.cue) {
            let a = &self.snapped[i];
            return (a.start, a.end.unwrap_or(a.start + length));
        }
        let to = self.speed * start + self.offset_at(start);
        (to, to + length)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Errors {
    pub median: f64,
    pub p90: f64,
    pub within: f64,
}

fn quantile(sorted: &[f64], q: f64) -> f64 {
    let at = q * (sorted.len().saturating_sub(1)) as f64;
    let (lo, hi) = (at.floor() as usize, at.ceil() as usize);
    sorted[lo] + (sorted[hi] - sorted[lo]) * (at - lo as f64)
}

fn median(v: &[f64]) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    quantile(&s, 0.5)
}

pub fn errors(plan: &Plan, cues: &[(f64, f64)], anchors: &[Anchor]) -> Option<Errors> {
    let mut e: Vec<f64> = anchors
        .iter()
        .map(|a| (plan.map(a.cue, cues[a.cue]).0 - a.start).abs())
        .collect();
    if e.is_empty() {
        return None;
    }
    e.sort_by(f64::total_cmp);
    Some(Errors {
        median: quantile(&e, 0.5),
        p90: quantile(&e, 0.9),
        within: e.iter().filter(|&&x| x <= FIT).count() as f64 / e.len() as f64,
    })
}

fn theil_sen(x: &[f64], y: &[f64]) -> Option<f64> {
    let every = x.len().div_ceil(PAIRED).max(1);
    let pick: Vec<usize> = (0..x.len()).step_by(every).collect();
    let mut slopes = Vec::new();
    for (n, &i) in pick.iter().enumerate() {
        for &j in &pick[n + 1..] {
            let dx = x[j] - x[i];
            if dx > 0.0 {
                slopes.push((y[j] - y[i]) / dx);
            }
        }
    }
    (!slopes.is_empty()).then(|| median(&slopes))
}

fn sections(first: f64, at: &[f64], r: &[f64]) -> Vec<Section> {
    let n = r.len();
    let mut starts = vec![0];
    let mut current = median(&r[..SECTION.min(n)]);
    let mut i = 0;
    while i + SECTION <= n {
        let next = &r[i..i + SECTION];
        let m = median(next);
        if (m - current).abs() > FIT
            && next
                .iter()
                .all(|x| (x - current).abs() > FIT && (x - m).abs() <= FIT)
        {
            starts.push(i);
            current = m;
            i += SECTION;
        } else {
            i += 1;
        }
    }
    starts
        .iter()
        .enumerate()
        .map(|(k, &a)| {
            let b = starts.get(k + 1).copied().unwrap_or(n);
            Section {
                from: if k == 0 { first } else { at[a] },
                offset: median(&r[a..b]),
            }
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Level {
    #[serde(flatten)]
    pub plan: Plan,
    pub errors: Errors,
    pub fits: bool,
}

pub fn levels(cues: &[(f64, f64)], anchors: &[Anchor]) -> Vec<Level> {
    if anchors.is_empty() {
        return Vec::new();
    }
    let at: Vec<f64> = anchors.iter().map(|a| cues[a.cue].0).collect();
    let need: Vec<f64> = anchors.iter().zip(&at).map(|(a, c)| a.start - c).collect();
    let first = cues.first().map_or(0.0, |c| c.0);
    let one = |offset: f64| {
        vec![Section {
            from: first,
            offset,
        }]
    };
    let mut out = Vec::new();
    let mut push = |plan: Plan| {
        let errors = errors(&plan, cues, anchors).expect("anchors");
        out.push(Level {
            fits: errors.p90 <= FIT,
            plan,
            errors,
        });
    };
    let mut base = Plan {
        kind: Kind::Offset,
        speed: 1.0,
        ratio: None,
        sections: one(median(&need)),
        snapped: Vec::new(),
    };
    push(base.clone());
    if let Some(slope) = theil_sen(&at, &need) {
        let (speed, ratio) = snap(1.0 + slope, span(cues));
        if speed != 1.0 && (SLOWEST..=FASTEST).contains(&speed) {
            let rest: Vec<f64> = need
                .iter()
                .zip(&at)
                .map(|(n, c)| n - (speed - 1.0) * c)
                .collect();
            base = Plan {
                kind: Kind::Speed,
                speed,
                ratio,
                sections: one(median(&rest)),
                snapped: Vec::new(),
            };
            push(base.clone());
        }
    }
    let rest: Vec<f64> = need
        .iter()
        .zip(&at)
        .map(|(n, c)| n - (base.speed - 1.0) * c)
        .collect();
    let cut = sections(first, &at, &rest);
    if cut.len() > 1 {
        base = Plan {
            kind: Kind::Sections,
            sections: cut,
            ..base
        };
        push(base.clone());
    }
    push(Plan {
        kind: Kind::Words,
        snapped: anchors.to_vec(),
        ..base
    });
    out
}
