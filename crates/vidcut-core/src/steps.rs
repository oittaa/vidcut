use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::{Clip, Cue, Project, SubtitleTrack, Track, Transition};
use crate::request::{Client, ClientKind};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<String>,
    pub from: i64,
    pub to: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Client>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opened: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restored: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clips: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spans: Vec<Span>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<String>,
}

impl Step {
    pub fn labeled(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            ..Self::default()
        }
    }
}

pub trait Node: Clone {
    fn id(&self) -> Option<u64>;
    fn placed(&self) -> Option<u64>;
}

impl Node for Step {
    fn id(&self) -> Option<u64> {
        self.id
    }

    fn placed(&self) -> Option<u64> {
        self.restored.or(self.time)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mark {
    pub step: u64,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Branch<T> {
    pub after: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced: Option<u64>,
    pub steps: Vec<T>,
    #[serde(default = "Vec::new", skip_serializing_if = "Vec::is_empty")]
    pub branches: Vec<Branch<T>>,
}

impl<T: Node> Branch<T> {
    pub fn id(&self) -> Option<u64> {
        self.steps.first().and_then(Node::id)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tree<T> {
    pub line: Vec<T>,
    pub done: usize,
    pub branches: Vec<Branch<T>>,
    pub marks: Vec<Mark>,
    restoring: Option<(usize, usize, Vec<Branch<T>>)>,
}

impl<T> Default for Tree<T> {
    fn default() -> Self {
        Self {
            line: Vec::new(),
            done: 0,
            branches: Vec::new(),
            marks: Vec::new(),
            restoring: None,
        }
    }
}

pub type Steps = Tree<Step>;

impl<T: Node> Tree<T> {
    pub fn undo(&self) -> Option<&T> {
        self.done.checked_sub(1).and_then(|i| self.line.get(i))
    }

    pub fn redo(&self) -> Option<&T> {
        self.line.get(self.done)
    }

    pub fn place(&mut self, step: T) {
        self.place_at(self.done + 1, step);
    }

    pub fn place_at(&mut self, at: usize, step: T) {
        let pos = at.saturating_sub(1).min(self.line.len());
        if let Some(id) = step.id()
            && let Some(i) = self
                .branches
                .iter()
                .position(|b| b.after == pos && b.id() == Some(id))
        {
            let b = self.branches.remove(i);
            self.restoring = Some((b.steps.len(), pos, b.branches));
        }
        if self.line.len() > pos {
            let tail = self.line.split_off(pos);
            let (inner, outer): (Vec<_>, Vec<_>) = std::mem::take(&mut self.branches)
                .into_iter()
                .partition(|b| b.after > pos);
            self.branches = outer;
            self.branches.push(Branch {
                after: pos,
                replaced: step.placed(),
                steps: tail,
                branches: inner
                    .into_iter()
                    .map(|mut b| {
                        b.after -= pos;
                        b
                    })
                    .collect(),
            });
        }
        self.line.push(step);
        self.done = self.line.len();
        if let Some((left, _, _)) = &mut self.restoring {
            *left = left.saturating_sub(1);
            if *left == 0
                && let Some((_, start, nested)) = self.restoring.take()
            {
                self.branches.extend(nested.into_iter().map(|mut b| {
                    b.after += start;
                    b
                }));
            }
        }
    }

    pub fn back(&mut self, n: usize) {
        self.done -= n.min(self.done);
    }

    pub fn forward(&mut self, n: usize) {
        self.done = (self.done + n).min(self.line.len());
    }

    pub fn mark(&mut self, mark: Mark) {
        self.marks.retain(|m| m.step != mark.step);
        self.marks.push(mark);
    }

    pub fn unmark(&mut self, step: u64) {
        self.marks.retain(|m| m.step != step);
    }

    pub fn branch(&self, id: u64) -> Option<&Branch<T>> {
        self.branches.iter().find(|b| b.id() == Some(id))
    }

    pub fn chain(&self, id: u64) -> Option<Vec<&Branch<T>>> {
        chain(&self.branches, id)
    }

    pub fn map<U>(&self, f: &impl Fn(&T) -> U) -> Tree<U> {
        fn branch<T, U>(b: &Branch<T>, f: &impl Fn(&T) -> U) -> Branch<U> {
            Branch {
                after: b.after,
                replaced: b.replaced,
                steps: b.steps.iter().map(f).collect(),
                branches: b.branches.iter().map(|x| branch(x, f)).collect(),
            }
        }
        Tree {
            line: self.line.iter().map(f).collect(),
            done: self.done,
            branches: self.branches.iter().map(|b| branch(b, f)).collect(),
            marks: self.marks.clone(),
            restoring: None,
        }
    }
}

pub fn chain<T: Node>(branches: &[Branch<T>], id: u64) -> Option<Vec<&Branch<T>>> {
    for b in branches {
        if b.id() == Some(id) {
            return Some(vec![b]);
        }
        if let Some(mut inner) = chain(&b.branches, id) {
            inner.insert(0, b);
            return Some(inner);
        }
    }
    None
}

impl Steps {
    pub fn named(&self, id: u64) -> Option<&str> {
        self.marks
            .iter()
            .find(|m| m.step == id)
            .map(|m| m.name.as_str())
    }

    pub fn position(&self, id: u64) -> Option<usize> {
        if id == 0 {
            return Some(0);
        }
        self.line
            .iter()
            .position(|s| s.id == Some(id))
            .map(|i| i + 1)
    }
}

pub const KINDS: [&str; 14] = [
    "Edits in",
    "Splits",
    "Trims",
    "Deletes",
    "Moves",
    "Dissolves",
    "Markers and marks",
    "Media and bins",
    "Tracks and clips",
    "Settings",
    "Reverts",
    "Batches",
    "Outside changes",
    "Other",
];

pub fn category(kind: &str) -> &'static str {
    match kind {
        "edit.append"
        | "edit.insert"
        | "edit.overwrite"
        | "edit.smart_insert"
        | "edit.ripple_overwrite"
        | "edit.place_on_top"
        | "edit.paste"
        | "edit.replace" => "Edits in",
        "edit.split" | "edit.join" | "edit.detect_scene_cuts" => "Splits",
        "edit.trim"
        | "edit.roll"
        | "edit.slip"
        | "edit.slide"
        | "edit.audio_roll"
        | "edit.trim_to_playhead"
        | "edit.change_duration"
        | "edit.change_speed" => "Trims",
        "edit.delete"
        | "edit.delete_range"
        | "edit.close_gap"
        | "edit.ripple_delete_silence"
        | "edit.cut"
        | "edit.ripple_delete_words" => "Deletes",
        "edit.move" => "Moves",
        "edit.dissolve" | "edit.remove_transition" => "Dissolves",
        k if k.starts_with("markers.") || k.starts_with("marks.") || k.contains(".marks.") => {
            "Markers and marks"
        }
        k if k.starts_with("media.")
            || k.starts_with("bins.")
            || k.starts_with("proxy.")
            || k == "timeline.import" =>
        {
            "Media and bins"
        }
        k if k.starts_with("track.") || k.starts_with("clip.") || k == "edit.decompose" => {
            "Tracks and clips"
        }
        "project.set" => "Settings",
        "history.revert" => "Reverts",
        "apply" => "Batches",
        "file" => "Outside changes",
        _ => "Other",
    }
}

#[derive(Debug, Clone, Default)]
pub struct StepsCache {
    pub(crate) path: std::path::PathBuf,
    pub(crate) end: u64,
    pub(crate) mark: Vec<u8>,
    pub(crate) steps: Steps,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub from: usize,
    pub to: usize,
    pub by: Client,
}

pub fn groups(line: &[Step]) -> Vec<Group> {
    let mut out: Vec<Group> = Vec::new();
    let mut opened = None;
    for (i, s) in line.iter().enumerate() {
        let session = s.opened.is_some() && s.opened != opened;
        if s.opened.is_some() {
            opened = s.opened;
        }
        let Some(by) = s.by.as_ref().filter(|c| c.kind != ClientKind::Gui) else {
            continue;
        };
        match out.last_mut() {
            Some(g) if g.to == i && g.by == *by && !session => g.to = i + 1,
            _ => out.push(Group {
                from: i,
                to: i + 1,
                by: by.clone(),
            }),
        }
    }
    out.retain(|g| g.to - g.from > 1);
    out
}

pub fn places(before: &Project, after: &Project) -> (Vec<String>, Vec<Span>) {
    let mut clips = Vec::new();
    let mut spans: Vec<Span> = Vec::new();
    let mut shifted = Vec::new();
    let tracks: Vec<&str> = after
        .tracks
        .iter()
        .chain(&before.tracks)
        .map(|t| t.id.as_str())
        .collect();
    let mut seen = Vec::new();
    for id in tracks {
        if seen.contains(&id) {
            continue;
        }
        seen.push(id);
        let (ta, tb) = (before.track(id), after.track(id));
        if ta == tb {
            continue;
        }
        let ((ca, xa), (cb, xb)) = (contents(ta), contents(tb));
        let on = |from: i64, to: i64| Span {
            track: Some(id.to_string()),
            from,
            to,
        };
        let was: HashMap<&str, &Clip> = ca.iter().map(|c| (c.id.as_str(), c)).collect();
        let is: HashMap<&str, &Clip> = cb.iter().map(|c| (c.id.as_str(), c)).collect();
        for c in ca {
            match is.get(c.id.as_str()) {
                None => spans.push(on(c.start, c.end())),
                Some(n) if *n == c => {}
                Some(n) if only_moved(c, n) => {
                    shifted.push((n.id.clone(), on(c.start, c.end())));
                    shifted.push((n.id.clone(), on(n.start, n.end())));
                }
                Some(n) => {
                    clips.push(n.id.clone());
                    spans.push(on(c.start.min(n.start), c.end().max(n.end())));
                }
            }
        }
        for c in cb {
            if !was.contains_key(c.id.as_str()) {
                clips.push(c.id.clone());
                spans.push(on(c.start, c.end()));
            }
        }
        let around = |clips: &[Clip], x: &Transition| {
            cut(clips, x).map(|at| on(at - x.before, at + x.after))
        };
        let old: HashMap<&str, &Transition> = xa.iter().map(|x| (x.id.as_str(), x)).collect();
        for x in xb {
            if old.get(x.id.as_str()) != Some(&x) {
                spans.extend(around(cb, x));
                if let Some(o) = old.get(x.id.as_str()) {
                    spans.extend(around(ca, o));
                }
            }
        }
        for x in xa {
            if !xb.iter().any(|y| y.id == x.id) {
                spans.extend(around(ca, x));
            }
        }
    }
    if spans.is_empty() && clips.is_empty() {
        for (id, span) in shifted {
            if !clips.contains(&id) {
                clips.push(id);
            }
            spans.push(span);
        }
    }
    let marked = |p: &Project| -> BTreeMap<String, (i64, i64)> {
        p.markers
            .iter()
            .map(|k| (k.id.clone(), (k.at, k.at + k.duration.max(1))))
            .collect()
    };
    let (ka, kb) = (marked(before), marked(after));
    for (id, span) in ka.iter().chain(&kb) {
        let a = ka.get(id);
        let b = kb.get(id);
        if a != b {
            spans.push(Span {
                track: None,
                from: span.0,
                to: span.1,
            });
        }
    }
    let (qa, qb) = (cues(before), cues(after));
    let mut hits: Vec<(&str, &Cue)> = Vec::new();
    for (id, (track, c)) in &qa {
        match qb.get(id) {
            Some(now) if now == &(*track, *c) => {}
            Some((t, n)) => {
                hits.push((track, c));
                hits.push((t, n));
            }
            None => hits.push((track, c)),
        }
    }
    for (id, (track, c)) in &qb {
        if !qa.contains_key(id) {
            hits.push((track, c));
        }
    }
    let mut touched: BTreeMap<&str, (i64, i64)> = BTreeMap::new();
    for (track, c) in hits {
        let e = touched.entry(track).or_insert((c.start, c.end()));
        *e = (e.0.min(c.start), e.1.max(c.end()));
    }
    spans.extend(touched.into_iter().map(|(track, (from, to))| Span {
        track: Some(track.to_string()),
        from,
        to,
    }));
    (clips, merged(spans))
}

fn cues(p: &Project) -> HashMap<&str, (&str, &Cue)> {
    p.subtitles
        .iter()
        .flat_map(|t| t.cues.iter().map(|c| (c.id.as_str(), (t.id.as_str(), c))))
        .collect()
}

pub fn union(steps: &[Step]) -> (Vec<String>, Vec<Span>) {
    let mut clips: Vec<String> = Vec::new();
    for s in steps {
        for c in &s.clips {
            if !clips.contains(c) {
                clips.push(c.clone());
            }
        }
    }
    let spans = merged(steps.iter().flat_map(|s| s.spans.clone()).collect());
    (clips, spans)
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Since {
    pub clips: Counts,
    pub subtitles: Counts,
    pub dissolves: Counts,
    pub fades: Counts,
    pub markers: Counts,
    pub media: Counts,
    pub tracks: Counts,
    pub bins: Counts,
    pub marks: bool,
    pub settings: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SinceStep {
    pub since: Since,
    pub redo: bool,
    pub version: Option<u64>,
}

pub fn since_step(then: &Project, now: &Project, step: usize, done: usize) -> SinceStep {
    let redo = step > done;
    SinceStep {
        since: match redo {
            true => since(now, then),
            false => since(then, now),
        },
        redo,
        version: None,
    }
}

pub fn since_version(then: &Project, now: &Project, id: u64) -> SinceStep {
    SinceStep {
        since: since(now, then),
        redo: false,
        version: Some(id),
    }
}

impl SinceStep {
    pub fn text(&self) -> String {
        let lead = match (self.version, self.redo) {
            (Some(_), _) => "Restoring to here",
            (None, true) => "Redoing to here",
            (None, false) => "Since this step",
        };
        format!("{lead}: {}", self.since.text())
    }

    pub fn json(&self, step: usize) -> Value {
        let mut v = self.since.json(step);
        v["redo"] = json!(self.redo);
        if let Some(id) = self.version {
            v["branch"] = json!(id);
        }
        v
    }
}

pub fn every_step(steps: &Steps) -> Vec<&Step> {
    fn walk<'a>(branches: &'a [Branch<Step>], out: &mut Vec<&'a Step>) {
        for b in branches {
            out.extend(&b.steps);
            walk(&b.branches, out);
        }
    }
    let mut out: Vec<&Step> = steps.line.iter().collect();
    walk(&steps.branches, &mut out);
    out
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Counts {
    pub added: usize,
    pub deleted: usize,
    pub moved: usize,
    pub trimmed: usize,
    pub changed: usize,
}

impl Counts {
    fn words(&self, one: &str, many: &str) -> Vec<String> {
        let mut out = Vec::new();
        for (n, verb) in [
            (self.added, "added"),
            (self.deleted, "deleted"),
            (self.moved, "moved"),
            (self.trimmed, "trimmed"),
            (self.changed, "changed"),
        ] {
            if n == 0 {
                continue;
            }
            out.push(match out.is_empty() {
                true => format!("{n} {} {verb}", if n == 1 { one } else { many }),
                false => format!("{n} {verb}"),
            });
        }
        out
    }

    fn plus(self, o: Counts) -> Counts {
        Counts {
            added: self.added + o.added,
            deleted: self.deleted + o.deleted,
            moved: self.moved + o.moved,
            trimmed: self.trimmed + o.trimmed,
            changed: self.changed + o.changed,
        }
    }

    fn of<'a, T: PartialEq + 'a>(
        before: impl IntoIterator<Item = (&'a str, &'a T)>,
        after: impl IntoIterator<Item = (&'a str, &'a T)>,
    ) -> Self {
        let a: HashMap<&str, &T> = before.into_iter().collect();
        let b: HashMap<&str, &T> = after.into_iter().collect();
        Self {
            added: b.keys().filter(|k| !a.contains_key(*k)).count(),
            deleted: a.keys().filter(|k| !b.contains_key(*k)).count(),
            changed: a
                .iter()
                .filter(|(k, x)| b.get(*k).is_some_and(|y| y != *x))
                .count(),
            ..Self::default()
        }
    }
}

impl Since {
    pub fn text(&self) -> String {
        let mut parts = Vec::new();
        parts.extend(self.clips.words("clip", "clips"));
        parts.extend(self.subtitles.words("subtitle", "subtitles"));
        parts.extend(self.dissolves.words("dissolve", "dissolves"));
        parts.extend(self.fades.words("cross fade", "cross fades"));
        parts.extend(self.markers.words("marker", "markers"));
        parts.extend(self.media.words("media file", "media files"));
        parts.extend(self.tracks.words("track", "tracks"));
        parts.extend(self.bins.words("bin", "bins"));
        if self.marks {
            parts.push("In and Out marks changed".into());
        }
        if self.settings {
            parts.push("settings changed".into());
        }

        if parts.is_empty() {
            "nothing".into()
        } else {
            parts.join(", ")
        }
    }

    pub fn json(&self, step: usize) -> Value {
        let mut v = serde_json::to_value(self).unwrap_or_default();
        v["step"] = json!(step);
        v["text"] = json!(self.text());
        v
    }
}

pub fn since(before: &Project, after: &Project) -> Since {
    let clips = |p: &Project| -> HashMap<String, (String, Clip)> {
        p.tracks
            .iter()
            .flat_map(|t| {
                t.clips
                    .iter()
                    .map(|c| (c.id.clone(), (t.id.clone(), c.clone())))
            })
            .collect()
    };
    let (a, b) = (clips(before), clips(after));
    let mut c = Counts {
        added: b.keys().filter(|k| !a.contains_key(*k)).count(),
        deleted: a.keys().filter(|k| !b.contains_key(*k)).count(),
        ..Counts::default()
    };
    for (id, (ta, x)) in &a {
        let Some((tb, y)) = b.get(id) else { continue };
        if ta == tb && x == y {
            continue;
        }
        if x.duration != y.duration || x.source_in != y.source_in {
            c.trimmed += 1;
        } else if only_moved(x, y) || (ta != tb && x == y) {
            c.moved += 1;
        } else {
            c.changed += 1;
        }
    }
    let (qa, qb) = (cues(before), cues(after));
    let mut q = Counts {
        added: qb.keys().filter(|k| !qa.contains_key(*k)).count(),
        deleted: qa.keys().filter(|k| !qb.contains_key(*k)).count(),
        ..Counts::default()
    };
    for (id, (ta, x)) in &qa {
        let Some((tb, y)) = qb.get(id) else { continue };
        if ta == tb && x == y {
            continue;
        }
        if x.text != y.text || x.place != y.place {
            q.changed += 1;
        } else if x.duration != y.duration {
            q.trimmed += 1;
        } else {
            q.moved += 1;
        }
    }
    Since {
        clips: c,
        subtitles: q,
        dissolves: Counts::of(transitions(before, false), transitions(after, false)),
        fades: Counts::of(transitions(before, true), transitions(after, true)),
        markers: Counts::of(
            before.markers.iter().map(|k| (k.id.as_str(), k)),
            after.markers.iter().map(|k| (k.id.as_str(), k)),
        ),
        media: Counts::of(
            before.media.iter().map(|m| (m.id.as_str(), m)),
            after.media.iter().map(|m| (m.id.as_str(), m)),
        ),
        tracks: Counts::of(
            bare(before).iter().map(|t| (t.id.as_str(), t)),
            bare(after).iter().map(|t| (t.id.as_str(), t)),
        )
        .plus(Counts::of(
            bare_subtitles(before).iter().map(|t| (t.id.as_str(), t)),
            bare_subtitles(after).iter().map(|t| (t.id.as_str(), t)),
        )),
        bins: Counts::of(
            before.bins.iter().map(|b| (b.as_str(), b)),
            after.bins.iter().map(|b| (b.as_str(), b)),
        ),
        marks: before.marks != after.marks || before.tape_marks != after.tape_marks,
        settings: before.settings != after.settings,
    }
}

fn bare(p: &Project) -> Vec<Track> {
    p.tracks
        .iter()
        .map(|t| Track {
            clips: Vec::new(),
            transitions: Vec::new(),
            ..t.clone()
        })
        .collect()
}

fn bare_subtitles(p: &Project) -> Vec<SubtitleTrack> {
    p.subtitles
        .iter()
        .map(|t| SubtitleTrack {
            cues: Vec::new(),
            ..t.clone()
        })
        .collect()
}

fn transitions(p: &Project, sound: bool) -> impl Iterator<Item = (&str, &Transition)> {
    p.tracks
        .iter()
        .flat_map(|t| t.transitions.iter())
        .filter(move |x| x.sound == sound)
        .map(|x| (x.id.as_str(), x))
}

pub fn listing(steps: &Steps, from: Option<usize>, limit: Option<usize>, p: &Project) -> Value {
    let n = steps.line.len();
    let limit = limit.unwrap_or(100);
    let first = from.unwrap_or(n.saturating_sub(limit) + 1).max(1);
    let listed: Vec<Value> = steps
        .line
        .iter()
        .enumerate()
        .skip(first - 1)
        .take(limit)
        .map(|(i, s)| {
            let mut v = serde_json::to_value(s).unwrap_or_default();
            v["step"] = json!(i + 1);
            v["done"] = json!(i < steps.done);
            if let Some(spans) = v.get_mut("spans").and_then(Value::as_array_mut) {
                for span in spans {
                    for key in ["from", "to"] {
                        if let Some(f) = span[key].as_i64() {
                            span[format!("{key}_tc")] = json!(p.settings.timecode(f));
                        }
                    }
                }
            }
            v
        })
        .collect();
    let label = |s: &Step| s.label.clone();
    json!({
        "at": steps.done,
        "undo_steps": steps.done,
        "redo_steps": n - steps.done,
        "undo": steps.line[..steps.done].iter().rev().take(100).map(label).collect::<Vec<_>>(),
        "redo": steps.line[steps.done..].iter().take(100).map(label).collect::<Vec<_>>(),
        "steps": listed,
        "groups": groups(&steps.line)
            .iter()
            .map(|g| json!({ "from": g.from + 1, "to": g.to, "by": g.by }))
            .collect::<Vec<_>>(),
        "branches": steps.branches.iter().map(branch_json).collect::<Vec<_>>(),
        "marks": steps
            .marks
            .iter()
            .filter_map(|m| {
                let at = steps.position(m.step);
                (at.is_some() || steps.branches.iter().any(|b| holds(b, m.step))).then(|| {
                    json!({ "step": at, "id": m.step, "name": m.name, "time": m.time })
                })
            })
            .collect::<Vec<_>>(),
    })
}

fn holds(b: &Branch<Step>, id: u64) -> bool {
    b.steps.iter().any(|s| s.id == Some(id)) || b.branches.iter().any(|x| holds(x, id))
}

fn branch_json(b: &Branch<Step>) -> Value {
    json!({
        "id": b.id(),
        "after": b.after,
        "replaced": b.replaced,
        "steps": b.steps,
        "branches": b.branches.iter().map(branch_json).collect::<Vec<_>>(),
    })
}

fn contents(t: Option<&Track>) -> (&[Clip], &[Transition]) {
    match t {
        Some(t) => (&t.clips, &t.transitions),
        None => (&[], &[]),
    }
}

fn only_moved(a: &Clip, b: &Clip) -> bool {
    a.start != b.start
        && *a
            == Clip {
                start: a.start,
                ..b.clone()
            }
}

fn cut(clips: &[Clip], x: &Transition) -> Option<i64> {
    let clip = |id: &Option<String>| {
        id.as_ref()
            .and_then(|id| clips.iter().find(|c| &c.id == id))
    };
    clip(&x.from)
        .map(Clip::end)
        .or_else(|| clip(&x.to).map(|c| c.start))
}

fn merged(mut spans: Vec<Span>) -> Vec<Span> {
    spans.sort_by(|a, b| (&a.track, a.from, a.to).cmp(&(&b.track, b.from, b.to)));
    let mut out: Vec<Span> = Vec::new();
    for s in spans {
        match out.last_mut() {
            Some(l) if l.track == s.track && s.from <= l.to => l.to = l.to.max(s.to),
            _ => out.push(s),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(kind: ClientKind, name: Option<&str>, opened: Option<u64>) -> Step {
        Step {
            by: Some(Client::new(kind, name.map(String::from))),
            opened,
            ..Step::labeled("x")
        }
    }

    fn made(id: u64, time: u64) -> Step {
        Step {
            id: Some(id),
            time: Some(time),
            ..Step::labeled("x")
        }
    }

    #[test]
    fn a_restore_says_when_it_replaced_the_steps_it_replaced() {
        let mut tree = Steps::default();
        tree.place(made(1, 10));
        tree.place(made(2, 20));
        tree.back(1);
        tree.place(made(3, 30));
        tree.back(1);
        tree.place(Step {
            restored: Some(40),
            ..made(2, 20)
        });
        assert_eq!(tree.branches[0].replaced, Some(40));
    }

    #[test]
    fn an_agents_edits_in_a_row_are_one_group() {
        let line = [
            step(ClientKind::Gui, None, Some(1)),
            step(ClientKind::Mcp, Some("agent"), Some(1)),
            step(ClientKind::Mcp, Some("agent"), Some(1)),
            step(ClientKind::Mcp, Some("agent"), Some(1)),
            step(ClientKind::Gui, None, Some(1)),
        ];
        let g = groups(&line);
        assert_eq!(
            g.iter().map(|g| (g.from, g.to)).collect::<Vec<_>>(),
            [(1, 4)]
        );
    }

    #[test]
    fn the_guis_own_steps_stay_apart() {
        let line = [
            step(ClientKind::Gui, None, Some(1)),
            step(ClientKind::Gui, None, Some(1)),
        ];
        assert_eq!(groups(&line), []);
    }

    #[test]
    fn a_new_session_starts_a_new_group() {
        let line = [
            step(ClientKind::Cli, None, Some(1)),
            step(ClientKind::Cli, None, Some(1)),
            step(ClientKind::Cli, None, Some(2)),
            step(ClientKind::Cli, None, Some(2)),
        ];
        let g = groups(&line);
        assert_eq!(
            g.iter().map(|g| (g.from, g.to)).collect::<Vec<_>>(),
            [(0, 2), (2, 4)]
        );
    }

    #[test]
    fn a_roll_is_a_trim() {
        assert_eq!(category("edit.roll"), "Trims");
    }

    #[test]
    fn a_source_mark_is_a_mark() {
        assert_eq!(category("media.marks.set"), "Markers and marks");
    }

    #[test]
    fn touching_spans_on_a_track_merge() {
        let on = |from, to| Span {
            track: Some("V1".into()),
            from,
            to,
        };
        assert_eq!(
            merged(vec![on(30, 40), on(0, 10), on(10, 20)]),
            [on(0, 20), on(30, 40)]
        );
    }
}
