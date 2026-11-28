use crate::model::{Clip, Project, TrackKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toward {
    Previous,
    Next,
    Above,
    Below,
}

fn rows(p: &Project) -> Vec<usize> {
    let of = |kind: TrackKind| {
        p.tracks
            .iter()
            .enumerate()
            .filter(move |(_, t)| t.kind == kind)
            .map(|(i, _)| i)
    };
    of(TrackKind::Video).rev().chain(of(TrackKind::Audio)).collect()
}

fn distance(c: &Clip, at: i64) -> i64 {
    if at < c.start {
        c.start - at
    } else if at >= c.end() {
        at - c.end() + 1
    } else {
        0
    }
}

pub fn clip_beside(p: &Project, from: &str, toward: Toward) -> Option<String> {
    let (ti, ci) = p.find_clip(from)?;
    let clips = &p.tracks[ti].clips;
    let found = match toward {
        Toward::Previous => ci.checked_sub(1).map(|i| &clips[i]),
        Toward::Next => clips.get(ci + 1),
        Toward::Above | Toward::Below => {
            let rows = rows(p);
            let row = rows.iter().position(|&r| r == ti)?;
            let start = clips[ci].start;
            let order: Vec<usize> = match toward {
                Toward::Above => rows[..row].iter().rev().copied().collect(),
                _ => rows[row + 1..].to_vec(),
            };
            order
                .into_iter()
                .filter(|&r| !p.tracks[r].locked)
                .find_map(|r| p.tracks[r].clips.iter().min_by_key(|c| distance(c, start)))
        }
    };
    found.map(|c| c.id.clone())
}
