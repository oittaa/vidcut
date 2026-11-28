use serde::Serialize;

use super::{linked, locate, transition};
use crate::error::{Error, Result};
use crate::model::{Project, SoundFade, USUAL_X_LEVEL, USUAL_X_POINT};
use crate::request::{BatchCrossFade, BatchFade, CrossFadeKind, FadeEnd};

pub fn to_playhead(p: &mut Project, ids: &[String], fade: FadeEnd, at: i64) -> Result<Vec<String>> {
    let mut done = Vec::new();
    for id in linked::sounds(p, ids) {
        let (ti, ci) = locate(p, &id)?;
        let c = &mut p.tracks[ti].clips[ci];
        if !(c.start < at && at < c.end()) {
            continue;
        }
        let frames = match fade {
            FadeEnd::In => at - c.start,
            FadeEnd::Out => c.end() - at,
        } as f64;
        let duration = c.duration;
        let Some(a) = c.audio.as_mut() else {
            continue;
        };
        let end = match fade {
            FadeEnd::In => &mut a.fade_in,
            FadeEnd::Out => &mut a.fade_out,
        };
        *end = Some(match *end {
            Some(f) => SoundFade { frames, ..f },
            None => SoundFade::linear(frames),
        });
        crate::fades::fitted(&id, a, duration)?;
        done.push(id);
    }
    if done.is_empty() {
        return Err(Error::invalid(
            "no sound clip of these clips is under that time",
        ));
    }
    Ok(done)
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Batched {
    pub faded: Vec<String>,
    pub cross_faded: Vec<String>,
    pub skipped: Vec<Skipped>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Skipped {
    pub clip: String,
    pub fade: FadeEnd,
    pub why: &'static str,
}

fn checked(f: Option<BatchFade>) -> Result<Option<SoundFade>> {
    let Some(f) = f else {
        return Ok(None);
    };
    if f.frames < 1 {
        return Err(Error::invalid(format!(
            "a fade of {} frames is less than 1",
            f.frames
        )));
    }
    let fade = SoundFade {
        frames: f.frames as f64,
        x_level: f.x_level.unwrap_or(USUAL_X_LEVEL),
        x_point: f.x_point.unwrap_or(USUAL_X_POINT),
    };
    match crate::fades::problem(&fade) {
        Some(why) => Err(Error::invalid(why)),
        None => Ok(Some(fade)),
    }
}

pub fn batch(
    p: &mut Project,
    ids: &[String],
    fade_in: Option<BatchFade>,
    fade_out: Option<BatchFade>,
    cross: Option<BatchCrossFade>,
) -> Result<Batched> {
    let made = [checked(fade_in)?, checked(fade_out)?];
    let overwrite = [fade_in, fade_out].map(|f| f.is_some_and(|f| f.overwrite));
    if let Some(x) = cross.filter(|x| x.frames < 1) {
        return Err(Error::invalid(format!(
            "a cross fade of {} frames is less than 1",
            x.frames
        )));
    }
    let heard = linked::sounds(p, ids);
    for id in &heard {
        locate(p, id)?;
    }
    let mut out = Batched::default();
    let mut joined: Vec<(String, FadeEnd)> = Vec::new();
    if let Some(x) = cross {
        let edits: Vec<(usize, i64, String, String)> = p
            .tracks
            .iter()
            .enumerate()
            .flat_map(|(ti, t)| {
                t.clips
                    .windows(2)
                    .filter(|w| w[0].end() == w[1].start)
                    .filter(|w| heard.contains(&w[0].id) && heard.contains(&w[1].id))
                    .map(move |w| (ti, w[1].start, w[0].id.clone(), w[1].id.clone()))
            })
            .collect();
        for (ti, e, from, to) in edits {
            joined.push((from.clone(), FadeEnd::Out));
            joined.push((to, FadeEnd::In));
            let why = if transition::has_transition(p, ti, e) && !x.overwrite {
                Some("the edit has a transition")
            } else {
                let gain = x.kind == CrossFadeKind::EqualGain;
                match transition::batch_cross_fade(p, ti, e, x.frames, gain) {
                    Some(m) => {
                        out.cross_faded.push(m.id);
                        None
                    }
                    None => Some("the clips have too few frames past the edit"),
                }
            };
            if let Some(why) = why {
                out.skipped.push(Skipped {
                    clip: from,
                    fade: FadeEnd::Out,
                    why,
                });
            }
        }
    }
    for id in &heard {
        for (k, end) in [FadeEnd::In, FadeEnd::Out].into_iter().enumerate() {
            let Some(fade) = made[k] else {
                continue;
            };
            if joined.contains(&(id.clone(), end)) {
                continue;
            }
            let (ti, ci) = locate(p, id)?;
            let c = &mut p.tracks[ti].clips[ci];
            let duration = c.duration;
            let Some(a) = c.audio.as_mut() else {
                continue;
            };
            let mut next = a.clone();
            let slot = match end {
                FadeEnd::In => &mut next.fade_in,
                FadeEnd::Out => &mut next.fade_out,
            };
            let why = if slot.is_some() && !overwrite[k] {
                Some("the clip has a fade there")
            } else {
                *slot = Some(fade);
                (!next.fades_fit(duration)).then_some("the fades are longer than the clip")
            };
            match why {
                Some(why) => out.skipped.push(Skipped {
                    clip: id.clone(),
                    fade: end,
                    why,
                }),
                None => {
                    *a = next;
                    if !out.faded.contains(id) {
                        out.faded.push(id.clone());
                    }
                }
            }
        }
    }
    Ok(out)
}
