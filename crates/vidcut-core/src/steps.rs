use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::{Clip, Project, Track, Transition};
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
    pub by: Option<Client>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opened: Option<u64>,
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

    pub fn by_gui(&self) -> bool {
        self.by.as_ref().is_some_and(|c| c.kind == ClientKind::Gui)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Steps {
    pub line: Vec<Step>,
    pub done: usize,
}

impl Steps {
    pub fn undo(&self) -> Option<&Step> {
        self.done.checked_sub(1).and_then(|i| self.line.get(i))
    }

    pub fn redo(&self) -> Option<&Step> {
        self.line.get(self.done)
    }

    pub fn place(&mut self, step: Step) {
        self.line.truncate(self.done);
        self.line.push(step);
        self.done = self.line.len();
    }

    pub fn back(&mut self, n: usize) {
        self.done -= n.min(self.done);
    }

    pub fn forward(&mut self, n: usize) {
        self.done = (self.done + n).min(self.line.len());
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
    (clips, merged(spans))
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
