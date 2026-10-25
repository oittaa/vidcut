use serde_json::{Value, json};
use vidcut_core::model::Project;
use vidcut_core::ops::{self, Edge};

use super::{EditSel, Side, audio_edit_outgoing, ripples};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cut {
    Roll(String),
    Audio(String),
    Tail(String),
    Head(String),
}

impl Cut {
    pub fn of(p: &Project, e: &EditSel) -> Option<Self> {
        let t = p.track(&e.track.id())?;
        if e.side == Side::Audio {
            return audio_edit_outgoing(t, e.at).map(Self::Audio);
        }
        let out = t.clips.iter().find(|c| c.end() == e.at);
        let inc = t.clips.iter().find(|c| c.start == e.at);
        Some(match (e.side, out, inc) {
            (Side::Both, Some(o), Some(_)) => Self::Roll(o.id.clone()),
            (Side::Out | Side::Both, Some(o), _) => Self::Tail(o.id.clone()),
            (Side::In | Side::Both, _, Some(i)) => Self::Head(i.id.clone()),
            _ => return None,
        })
    }

    pub fn clip(&self) -> &str {
        match self {
            Self::Roll(id) | Self::Audio(id) | Self::Tail(id) | Self::Head(id) => id,
        }
    }

    pub fn rolls(&self) -> bool {
        matches!(self, Self::Roll(_) | Self::Audio(_))
    }

    pub fn apply(&self, p: &mut Project, by: i64) -> vidcut_core::Result<()> {
        match self {
            Self::Roll(id) => ops::roll(p, id, Edge::Tail, by),
            Self::Audio(id) => ops::audio_roll(p, id, Edge::Tail, by),
            Self::Tail(id) => ops::trim(p, id, Edge::Tail, by, None),
            Self::Head(id) => ops::trim(p, id, Edge::Head, by, None),
        }
    }

    pub fn command(&self, by: i64) -> Value {
        let (method, edge) = match self {
            Self::Roll(_) => ("edit.roll", "tail"),
            Self::Audio(_) => ("edit.audio_roll", "tail"),
            Self::Tail(_) => ("edit.trim", "tail"),
            Self::Head(_) => ("edit.trim", "head"),
        };
        json!({ "method": method, "params": { "clip": self.clip(), "edge": edge, "by": by } })
    }

    pub fn at(&self, p: &Project) -> Option<i64> {
        let c = p.clip(self.clip())?;
        Some(match self {
            Self::Audio(_) => c.end() + c.audio.as_ref().map_or(0, |a| a.tail),
            Self::Head(_) => c.start,
            Self::Roll(_) | Self::Tail(_) => c.end(),
        })
    }

    fn ripples(&self, p: &Project) -> bool {
        !self.rolls() && p.settings.ripple_on && ripples(p, self.clip())
    }
}

pub type Trim = (EditSel, Cut, i64);

#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    cuts: Vec<(EditSel, Cut, i64)>,
}

impl Plan {
    pub fn new(p: &Project, edits: &[EditSel], lead: EditSel) -> vidcut_core::Result<Self> {
        let resolved = |e: &EditSel| {
            Cut::of(p, e).ok_or_else(|| vidcut_core::Error::not_found("no clip at the edit"))
        };
        let first = resolved(&lead)?;
        let mut cuts = Vec::new();
        for e in edits {
            let c = resolved(e)?;
            if c.rolls() != first.rolls() {
                return Err(vidcut_core::Error::usage(
                    "a roll and a trim cannot move together",
                ));
            }
            let facing = matches!(
                (&first, &c),
                (Cut::Tail(_), Cut::Head(_)) | (Cut::Head(_), Cut::Tail(_))
            );
            cuts.push((*e, c, if facing { -1 } else { 1 }));
        }
        cuts.sort_by_key(|(e, ..)| (e.track.audio, e.track.n, e.at));
        Ok(Self { cuts })
    }

    pub fn each(&self, by: i64) -> Vec<Trim> {
        self.cuts
            .iter()
            .map(|(e, c, sign)| (*e, c.clone(), sign * by))
            .collect()
    }

    pub fn shared(&self, p: &Project, by: i64) -> Vec<Trim> {
        let rippling: Vec<bool> = self.cuts.iter().map(|(_, c, _)| c.ripples(p)).collect();
        self.cuts
            .iter()
            .enumerate()
            .map(|(i, (e, c, sign))| {
                if !rippling[i] {
                    return (*e, c.clone(), sign * by);
                }
                let mates =
                    |(j, (x, ..)): (usize, &(EditSel, Cut, i64))| rippling[j] && x.track == e.track;
                let n = self.cuts.iter().enumerate().filter(|&m| mates(m)).count() as i64;
                let k = self.cuts[..i]
                    .iter()
                    .enumerate()
                    .filter(|&m| mates(m))
                    .count() as i64;
                let extra = i64::from(k < (by % n).abs()) * by.signum();
                (*e, c.clone(), sign * (by / n + extra))
            })
            .collect()
    }

    pub fn clips(&self, p: &Project) -> Vec<String> {
        self.cuts
            .iter()
            .flat_map(|(_, c, _)| match c {
                Cut::Tail(id) | Cut::Head(id) => vec![id.clone()],
                Cut::Roll(id) => {
                    let end = p.clip(id).map(|x| x.end());
                    let incoming = p.find_clip(id).and_then(|(ti, _)| {
                        p.tracks[ti]
                            .clips
                            .iter()
                            .find(|x| Some(x.start) == end)
                            .map(|x| x.id.clone())
                    });
                    std::iter::once(id.clone()).chain(incoming).collect()
                }
                Cut::Audio(_) => Vec::new(),
            })
            .collect()
    }
}

pub fn applied(p: &Project, trims: &[Trim]) -> vidcut_core::Result<Project> {
    let mut q = p.clone();
    for (_, c, by) in trims {
        c.apply(&mut q, *by)?;
    }
    Ok(q)
}

pub fn request(trims: &[Trim]) -> Option<Value> {
    match trims {
        [] => None,
        [(_, c, by)] => Some(c.command(*by)),
        many => Some(json!({
            "method": "apply",
            "params": {
                "commands": many.iter().map(|(_, c, by)| c.command(*by)).collect::<Vec<_>>(),
                "label": format!("Trim {} edits", many.len()),
            },
        })),
    }
}

pub fn moved(trims: &[Trim], q: &Project) -> Vec<(EditSel, EditSel)> {
    trims
        .iter()
        .map(|(e, c, _)| {
            let at = c.at(q).unwrap_or(e.at);
            (*e, EditSel { at, ..*e })
        })
        .collect()
}
