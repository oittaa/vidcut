use std::collections::HashSet;

use serde_json::{Value, json};

use super::locate;
use crate::error::{Error, Result};
use crate::generators::{Axis, CENTER, checked_center};
use crate::keyframes::{
    PictureValue, checked, checked_handle, ease_mark, ease_of, put_pair, reshape,
};
use crate::model::{
    Clip, ClipAudio, Ease, Effect, Generator, Gradient, Handle, Keyframe, LowerThirdLine, Picture,
    Project, Title, TitleKind,
};
use crate::request::{CopiedKey, EffectParam, KeyParam, KeyPicks, KeyValue};
use crate::validate::GAIN;

const ANCHOR_LINEAR: &str = "the anchor point's keyframes are linear: they take no ease or handles";
const SPEED_SET: &str =
    "a speed keyframe takes no value or handles: speed.set sets the speed of a section";
const SPEED_ADD: &str = "a new speed keyframe takes no value or ease: speed.set sets the speed \
     of a section, and keyframes.set its ease";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pick {
    pub frames: Option<Vec<i64>>,
    pub at: Option<Vec<i64>>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct KeyChange {
    pub value: Option<KeyValue>,
    pub by: Option<KeyValue>,
    pub ease: Option<Ease>,
    pub in_handle: Option<Option<Handle>>,
    pub out_handle: Option<Option<Handle>>,
}

fn keyed(a: &ClipAudio) -> bool {
    a.is_keyed()
}

pub fn check_keyed_gain(p: &Project, ids: &[String], at: Option<i64>) -> Result<()> {
    let clips: Vec<&Clip> = ids.iter().filter_map(|id| p.clip(id)).collect();
    let all_keyed = clips.iter().all(|c| c.audio.as_ref().is_some_and(keyed));
    if clips.is_empty() || !all_keyed {
        return Ok(());
    }
    match at {
        None => Err(Error::usage(format!(
            "{} has volume keyframes: give `at` for the keyframe",
            clips[0].id
        ))),
        Some(_) => Ok(()),
    }
}

pub fn put_value(keys: &mut Vec<Keyframe>, frame: i64, value: f64) {
    reshape(keys, |marked| {
        match marked.iter_mut().find(|(k, _)| k.frame == frame) {
            Some((k, _)) => k.value = value,
            None => marked.push((
                Keyframe {
                    frame,
                    value,
                    in_handle: None,
                    out_handle: None,
                },
                None,
            )),
        }
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Number {
    Volume,
    Center(Axis),
    Effect(EffectParam),
}

impl Number {
    fn of(param: KeyParam) -> Self {
        match param {
            KeyParam::CenterX => Self::Center(Axis::X),
            KeyParam::CenterY => Self::Center(Axis::Y),
            KeyParam::Effect(q) => Self::Effect(q),
            _ => Self::Volume,
        }
    }

    fn name(self) -> String {
        match self {
            Self::Volume => "volume".to_string(),
            Self::Center(Axis::X) => "center x".to_string(),
            Self::Center(Axis::Y) => "center y".to_string(),
            Self::Effect(q) => q.value.name().replace('_', " "),
        }
    }

    fn takes(self, c: &Clip) -> bool {
        match self {
            Self::Volume => c.audio_range().is_some(),
            Self::Center(_) => gradient(c).is_some(),
            Self::Effect(q) => effect_of(c, q).is_some_and(|e| e.takes(q.value)),
        }
    }

    fn missing(self) -> &'static str {
        match self {
            Self::Volume => "has no sound",
            Self::Center(_) => "is not a Four Color Gradient",
            Self::Effect(_) => "has no effect with that value",
        }
    }

    fn pair(self) -> bool {
        matches!(self, Self::Effect(q) if q.value.pair())
    }

    fn ys(self, c: &Clip) -> &[f64] {
        match self {
            Self::Effect(q) => effect_of(c, q).map_or(&[], |e| e.y_of(q.value)),
            _ => &[],
        }
    }

    fn point_at(self, c: &Clip, frame: f64) -> [f64; 2] {
        match self {
            Self::Effect(q) => effect_of(c, q).map_or([0.0; 2], |e| e.point_at(q.value, frame)),
            _ => [self.value_at(c, frame), 0.0],
        }
    }

    fn checked_point(self, [x, y]: [f64; 2]) -> Result<[f64; 2]> {
        let y = if self.pair() { self.checked(y)? } else { y };
        Ok([self.checked(x)?, y])
    }

    fn keys(self, c: &Clip) -> &[Keyframe] {
        match self {
            Self::Volume => c.audio.as_ref().map_or(&[], |a| a.gain_keys.as_slice()),
            Self::Center(axis) => gradient(c).map_or(&[], |g| g.keys(axis)),
            Self::Effect(q) => effect_of(c, q).map_or(&[], |e| e.keys_of(q.value)),
        }
    }

    fn channel_mut(self, c: &mut Clip) -> Option<Channel<'_>> {
        match self {
            Self::Volume => c.audio.as_mut().map(|a| (&mut a.gain_keys, None)),
            Self::Center(axis) => match &mut c.generator {
                Some(Generator::FourColorGradient(g)) => Some((g.keys_mut(axis), None)),
                _ => None,
            },
            Self::Effect(q) => c
                .picture
                .as_mut()?
                .effects
                .get_mut(q.effect)?
                .channel_mut(q.value),
        }
    }

    fn value_at(self, c: &Clip, frame: f64) -> f64 {
        match self {
            Self::Volume => c.audio.as_ref().map_or(0.0, |a| a.db_at(frame)),
            Self::Center(axis) => gradient(c).map_or(0.0, |g| g.center_at(axis, frame)),
            Self::Effect(q) => effect_of(c, q).map_or(0.0, |e| e.value_at(q.value, frame)),
        }
    }

    fn checked(self, value: f64) -> Result<f64> {
        match self {
            Self::Volume => checked(value),
            Self::Center(_) => checked_center(value),
            Self::Effect(_) if value.is_finite() => Ok(value),
            Self::Effect(_) => Err(Error::invalid("a keyframe's value is a finite number")),
        }
    }

    fn limited(self, c: &Clip, value: f64) -> f64 {
        let (low, high) = match self {
            Self::Volume => (*GAIN.start(), *GAIN.end()),
            Self::Center(_) => (*CENTER.start(), *CENTER.end()),
            Self::Effect(q) => effect_of(c, q)
                .and_then(|e| e.range_of(q.value))
                .unwrap_or((value, value)),
        };
        value.clamp(low, high)
    }
}

type Channel<'a> = (&'a mut Vec<Keyframe>, Option<&'a mut Vec<f64>>);

fn ys_after(keys: &[Keyframe], follow: &[(i64, f64)]) -> Vec<f64> {
    keys.iter()
        .map(|k| {
            follow
                .iter()
                .find(|(f, _)| *f == k.frame)
                .map_or(0.0, |(_, y)| *y)
        })
        .collect()
}

fn number_value(
    param: KeyParam,
    number: Number,
    value: Option<KeyValue>,
) -> Result<Option<[f64; 2]>> {
    match (number.pair(), value) {
        (_, None) => Ok(None),
        (true, Some(KeyValue::Pair(n))) => Ok(Some(n)),
        (false, Some(KeyValue::Scalar(n))) => Ok(Some([n, 0.0])),
        (true, Some(KeyValue::Scalar(_))) => {
            Err(Error::usage(format!("{} is x and y", param_name(param))))
        }
        (false, Some(KeyValue::Pair(_))) => Err(Error::usage(format!(
            "{} takes one number",
            param_name(param)
        ))),
    }
}

fn effect_of(c: &Clip, q: EffectParam) -> Option<&Effect> {
    c.picture.as_ref()?.effects.get(q.effect)
}

fn settled(p: &mut Project, ids: &[String], number: Number) -> Result<()> {
    let Number::Effect(q) = number else {
        return Ok(());
    };
    for id in ids {
        let (ti, ci) = locate(p, id)?;
        let picture = p.tracks[ti].clips[ci].picture.as_mut();
        if let Some(e) = picture.and_then(|x| x.effects.get_mut(q.effect)) {
            *e = super::picture::effect_checked(e)?;
        }
    }
    Ok(())
}

fn gradient(c: &Clip) -> Option<&Gradient> {
    match &c.generator {
        Some(Generator::FourColorGradient(g)) => Some(g),
        _ => None,
    }
}

fn number_mut<'a>(p: &'a mut Project, id: &str, number: Number) -> Result<(Channel<'a>, Clip)> {
    let (ti, ci) = locate(p, id)?;
    let c = &mut p.tracks[ti].clips[ci];
    let shape = c.clone();
    let channel = number
        .channel_mut(c)
        .ok_or_else(|| Error::invalid(format!("{id} {}", number.missing())))?;
    Ok((channel, shape))
}

pub fn add(
    p: &mut Project,
    ids: &[String],
    param: KeyParam,
    at: i64,
    value: Option<KeyValue>,
    ease: Option<Ease>,
) -> Result<Vec<(String, i64)>> {
    picture_only(p, ids, param)?;
    if param == KeyParam::Speed {
        if value.is_some() || ease.is_some() {
            return Err(Error::usage(SPEED_ADD));
        }
        return super::speed::add_keyframe(p, ids, at);
    }
    if let Some(v) = picture_param(param) {
        return add_picture(p, ids, v, at, value, ease);
    }
    let number = Number::of(param);
    let value = number_value(param, number, value)?;
    let added = add_number(p, ids, number, at, value, ease)?;
    settled(p, ids, number)?;
    Ok(added)
}

fn add_number(
    p: &mut Project,
    ids: &[String],
    number: Number,
    at: i64,
    value: Option<[f64; 2]>,
    ease: Option<Ease>,
) -> Result<Vec<(String, i64)>> {
    let value = value.map(|v| number.checked_point(v)).transpose()?;
    let takes = |c: &Clip| number.takes(c);
    let targets: Vec<String> = ids
        .iter()
        .filter(|id| p.clip(id).is_some_and(takes))
        .cloned()
        .collect();
    if targets.is_empty() {
        for id in ids {
            locate(p, id)?;
        }
        return Err(Error::invalid(format!(
            "no clip of {} {}",
            ids.join(", "),
            match number {
                Number::Volume => "has sound",
                Number::Center(_) => "is a Four Color Gradient",
                Number::Effect(_) => "has an effect with that value",
            }
        )));
    }
    let mut added = Vec::new();
    for id in &targets {
        let ((keys, y), c) = number_mut(p, id, number)?;
        let frame = c.content_at(at);
        let point = value.unwrap_or_else(|| number.point_at(&c, frame as f64));
        if let Some(y) = y {
            put_pair(keys, y, frame, point);
            if let Some(e) = ease {
                ease_mark(keys, frame, e);
            }
            added.push((id.clone(), frame));
            continue;
        }
        let v = point[0];
        reshape(keys, |marked| {
            let i = match marked.iter().position(|(k, _)| k.frame == frame) {
                Some(i) => {
                    marked[i].0.value = v;
                    i
                }
                None => {
                    marked.push((
                        Keyframe {
                            frame,
                            value: v,
                            in_handle: None,
                            out_handle: None,
                        },
                        None,
                    ));
                    marked.len() - 1
                }
            };
            if let Some(e) = ease {
                eased(&mut marked[i], e);
            }
        });
        added.push((id.clone(), frame));
    }
    Ok(added)
}

fn eased((k, mark): &mut (Keyframe, Option<Ease>), e: Ease) {
    *mark = Some(e);
    if e.is_linear() {
        (k.in_handle, k.out_handle) = (None, None);
    }
}

pub(super) fn picked_as(
    c: &Clip,
    keys: &[Keyframe],
    pick: &Pick,
    name: &str,
) -> Result<HashSet<i64>> {
    let mut out = HashSet::new();
    if pick.frames.is_none() && pick.at.is_none() {
        out.extend(keys.iter().map(|k| k.frame));
        return Ok(out);
    }
    for &f in pick.frames.iter().flatten() {
        if !keys.iter().any(|k| k.frame == f) {
            return Err(Error::not_found(format!(
                "{} has no {name} keyframe on source frame {f}",
                c.id
            )));
        }
        out.insert(f);
    }
    for &t in pick.at.iter().flatten() {
        let k = keys
            .iter()
            .find(|k| c.key_time(k.frame).round() as i64 == t)
            .ok_or_else(|| {
                Error::not_found(format!("{} has no {name} keyframe at frame {t}", c.id))
            })?;
        out.insert(k.frame);
    }
    Ok(out)
}

fn keyed_number<'a>(p: &'a mut Project, id: &str, number: Number) -> Result<(Channel<'a>, Clip)> {
    let (channel, c) = number_mut(p, id, number)?;
    if channel.0.is_empty() {
        return Err(Error::invalid(format!(
            "{id} has no {} keyframes",
            number.name()
        )));
    }
    Ok((channel, c))
}

pub fn set(
    p: &mut Project,
    ids: &[String],
    param: KeyParam,
    pick: &Pick,
    change: &KeyChange,
) -> Result<()> {
    picture_only(p, ids, param)?;
    if param == KeyParam::Speed {
        let shaped = change.value.is_some()
            || change.by.is_some()
            || change.in_handle.is_some()
            || change.out_handle.is_some();
        return match change.ease.filter(|_| !shaped) {
            Some(e) => super::speed::ease_keyframes(p, ids, pick, e),
            None => Err(Error::usage(SPEED_SET)),
        };
    }
    if let Some(v) = picture_param(param) {
        return set_picture(p, ids, v, pick, change);
    }
    let number = Number::of(param);
    let value = number_value(param, number, change.value)?
        .map(|v| number.checked_point(v))
        .transpose()?;
    let by = number_value(param, number, change.by)?;
    if by.is_some_and(|b| !b[0].is_finite() || !b[1].is_finite()) {
        return Err(Error::invalid(match number {
            Number::Volume => "`by` needs a number of dB",
            Number::Center(_) | Number::Effect(_) => "`by` needs a number",
        }));
    }
    if value.is_some() && by.is_some() {
        return Err(Error::usage("give `value` or `by`, not both"));
    }
    let handles = change.in_handle.is_some() || change.out_handle.is_some();
    if change.ease.is_some() && handles {
        return Err(Error::usage("give `ease` or handles, not both"));
    }
    let in_handle = change
        .in_handle
        .map(|h| h.map(|h| checked_handle(h, true)).transpose())
        .transpose()?;
    let out_handle = change
        .out_handle
        .map(|h| h.map(|h| checked_handle(h, false)).transpose())
        .transpose()?;
    for id in ids {
        let ((keys, y), c) = keyed_number(p, id, number)?;
        let chosen = picked_as(&c, keys, pick, &number.name())?;
        if let Some(y) = y {
            for (_, n) in keys
                .iter()
                .zip(y.iter_mut())
                .filter(|(k, _)| chosen.contains(&k.frame))
            {
                if let Some(v) = value {
                    *n = v[1];
                }
                if let Some(by) = by {
                    *n = number.limited(&c, *n + by[1]);
                }
            }
        }
        reshape(keys, |marked| {
            for entry in marked.iter_mut().filter(|(k, _)| chosen.contains(&k.frame)) {
                if let Some(v) = value {
                    entry.0.value = v[0];
                }
                if let Some(by) = by {
                    entry.0.value = number.limited(&c, entry.0.value + by[0]);
                }
                if let Some(e) = change.ease {
                    eased(entry, e);
                }
                if let Some(h) = in_handle {
                    (entry.0.in_handle, entry.1) = (h, None);
                }
                if let Some(h) = out_handle {
                    (entry.0.out_handle, entry.1) = (h, None);
                }
            }
        });
    }
    settled(p, ids, number)
}

pub fn shift(
    p: &mut Project,
    ids: &[String],
    param: KeyParam,
    pick: &Pick,
    by: Option<i64>,
    to: Option<i64>,
) -> Result<()> {
    picture_only(p, ids, param)?;
    if param == KeyParam::Speed {
        return Err(Error::invalid("speed keyframes don't move"));
    }
    if let Some(v) = picture_param(param) {
        return shift_picture(p, ids, v, pick, by, to);
    }
    let number = Number::of(param);
    shift_number(p, ids, number, pick, by, to)?;
    settled(p, ids, number)
}

fn shift_number(
    p: &mut Project,
    ids: &[String],
    number: Number,
    pick: &Pick,
    by: Option<i64>,
    to: Option<i64>,
) -> Result<()> {
    if by.is_some() == to.is_some() {
        return Err(Error::usage("give `by` or `to`"));
    }
    for id in ids {
        let ((keys, y), c) = keyed_number(p, id, number)?;
        let chosen = picked_as(&c, keys, pick, &number.name())?;
        if to.is_some() && chosen.len() != 1 {
            return Err(Error::usage(format!(
                "`to` moves one keyframe; {id} has {} chosen",
                chosen.len()
            )));
        }
        let moved = |f: i64| match (by, to) {
            (Some(k), _) => c.key_at(c.key_time(f) + k as f64).round() as i64,
            (_, Some(t)) => c.content_at(t),
            _ => f,
        };
        let mut taken: HashSet<i64> = keys
            .iter()
            .map(|k| k.frame)
            .filter(|f| !chosen.contains(f))
            .collect();
        for f in &chosen {
            if !taken.insert(moved(*f)) {
                return Err(Error::invalid(format!(
                    "{id} already has a {} keyframe on source frame {}",
                    number.name(),
                    moved(*f)
                )));
            }
        }
        let follow: Option<Vec<(i64, f64)>> = y.as_deref().map(|y| {
            keys.iter()
                .zip(y)
                .map(|(k, n)| match chosen.contains(&k.frame) {
                    true => (moved(k.frame), *n),
                    false => (k.frame, *n),
                })
                .collect()
        });
        reshape(keys, |marked| {
            for (k, _) in marked.iter_mut().filter(|(k, _)| chosen.contains(&k.frame)) {
                k.frame = moved(k.frame);
            }
        });
        if let (Some(y), Some(follow)) = (y, follow) {
            *y = ys_after(keys, &follow);
        }
    }
    Ok(())
}

pub fn remove(p: &mut Project, ids: &[String], param: KeyParam, pick: &Pick) -> Result<()> {
    picture_only(p, ids, param)?;
    if param == KeyParam::Speed {
        return super::speed::remove_keyframes(p, ids, pick);
    }
    if let Some(v) = picture_param(param) {
        return remove_picture(p, ids, v, pick);
    }
    let number = Number::of(param);
    remove_number(p, ids, number, pick)?;
    settled(p, ids, number)
}

fn remove_number(p: &mut Project, ids: &[String], number: Number, pick: &Pick) -> Result<()> {
    for id in ids {
        let ((keys, y), c) = keyed_number(p, id, number)?;
        let chosen = picked_as(&c, keys, pick, &number.name())?;
        let follow: Option<Vec<(i64, f64)>> = y.as_deref().map(|y| {
            keys.iter()
                .map(|k| k.frame)
                .zip(y.iter().copied())
                .collect()
        });
        reshape(keys, |marked| {
            marked.retain(|(k, _)| !chosen.contains(&k.frame));
        });
        if let (Some(y), Some(follow)) = (y, follow) {
            *y = ys_after(keys, &follow);
        }
    }
    Ok(())
}

pub fn list(p: &Project, ids: Option<&[String]>) -> Result<Value> {
    let mut clips = Vec::new();
    for t in &p.tracks {
        for c in &t.clips {
            let wanted = ids.is_none_or(|ids| ids.contains(&c.id));
            if !wanted {
                continue;
            }
            let mut numbers = vec![
                (json!("volume"), Number::Volume),
                (json!("center_x"), Number::Center(Axis::X)),
                (json!("center_y"), Number::Center(Axis::Y)),
            ];
            numbers.extend(effect_params(c).map(|q| (json!(q), Number::Effect(q))));
            for (param, number) in numbers {
                let held = number.keys(c);
                if held.is_empty() {
                    continue;
                }
                let ys = number.ys(c);
                let keys: Vec<Value> = held
                    .iter()
                    .enumerate()
                    .map(|(i, k)| {
                        let at = c.key_time(k.frame).round() as i64;
                        let value = match number.pair() {
                            true => json!([k.value, ys.get(i).copied().unwrap_or(0.0)]),
                            false => json!(k.value),
                        };
                        let mut v = json!({
                            "frame": k.frame,
                            "at": at,
                            "at_tc": p.settings.timecode(at),
                            "value": value,
                            "ease": ease_of(held, i),
                        });
                        if let Some(h) = k.in_handle {
                            v["in"] = json!(h);
                        }
                        if let Some(h) = k.out_handle {
                            v["out"] = json!(h);
                        }
                        v
                    })
                    .collect();
                clips.push(
                    json!({ "clip": c.id, "track": t.id, "param": param, "keyframes": keys }),
                );
            }
            if let Some(speed) = c.speed.as_ref().filter(|s| s.sections.len() > 1) {
                let keys: Vec<Value> = speed
                    .sections
                    .iter()
                    .skip(1)
                    .map(|s| {
                        let at = c.key_time(s.at).round() as i64;
                        let mut v = json!({
                            "frame": s.at,
                            "at": at,
                            "at_tc": p.settings.timecode(at),
                            "value": s.speed,
                        });
                        if s.ease.is_some() {
                            v["ease"] = json!(Ease::EaseInOut);
                        }
                        v
                    })
                    .collect();
                clips.push(
                    json!({ "clip": c.id, "track": t.id, "param": "speed", "keyframes": keys }),
                );
            }
            if let Some(picture) = &held_picture(c) {
                let rows = listed_values(p, c, &t.id, picture);
                let apart = c.title.as_ref().is_some_and(|t| t.upper.is_some());
                clips.extend(if apart {
                    with_line(rows, "lower")
                } else {
                    rows
                });
            }
            if let Some(title) = c.title.as_ref().filter(|t| t.upper.is_some()) {
                let picture = title.upper_view().as_picture();
                let rows = with_line(listed_values(p, c, &t.id, &picture), "upper");
                clips.extend(rows.into_iter().filter(|r| r["param"] != "opacity"));
            }
        }
    }
    for t in &p.subtitles {
        for q in &t.cues {
            if ids.is_none_or(|ids| ids.contains(&q.id))
                && let Some(picture) = &q.picture
            {
                clips.extend(listed_values(p, &q.as_clip(), &t.id, picture));
            }
        }
    }
    if let Some(ids) = ids {
        for id in ids {
            holder(p, id)?;
        }
    }
    Ok(json!({ "clips": clips }))
}

fn with_line(mut rows: Vec<Value>, line: &str) -> Vec<Value> {
    for row in rows.iter_mut().filter(|r| r["param"] != "opacity") {
        row["param"] = json!(format!("{line}_{}", row["param"].as_str().unwrap_or("")));
    }
    rows
}

pub fn on_line<T>(
    p: &mut Project,
    ids: &[String],
    param: KeyParam,
    work: impl FnOnce(&mut Project, KeyParam) -> Result<T>,
) -> Result<T> {
    let (value, line) = param.on_line();
    super::titles::on_line(p, ids, line, |p| work(p, value))
}

fn of_line(c: &Clip, line: Option<LowerThirdLine>) -> Clip {
    let mut c = c.clone();
    if line == Some(LowerThirdLine::Upper)
        && let Some(t) = c.title.as_mut()
    {
        t.swap_lines();
    }
    c
}

fn listed_values(p: &Project, c: &Clip, track: &str, picture: &Picture) -> Vec<Value> {
    PictureValue::ALL
        .into_iter()
        .filter(|v| picture.has_keys(*v))
        .map(|v| {
            json!({
                "clip": c.id,
                "track": track,
                "param": v.name(),
                "keyframes": listed(p, c, picture, v),
            })
        })
        .collect()
}

fn holder(p: &Project, id: &str) -> Result<Clip> {
    p.key_holder(id)
        .ok_or_else(|| Error::not_found(format!("no clip {id}")))
}

fn picture_only(p: &Project, ids: &[String], param: KeyParam) -> Result<()> {
    match (
        picture_param(param),
        ids.iter().find(|id| p.cue(id).is_some()),
    ) {
        (None, Some(id)) => Err(Error::invalid(format!(
            "{id} is a subtitle, which has no {} keyframes",
            param_name(param)
        ))),
        _ => Ok(()),
    }
}

fn keys_of(c: &Clip, param: KeyParam) -> &[Keyframe] {
    match (param, picture_param(param), &c.title) {
        (KeyParam::Speed, _, _) => &[],
        (_, Some(v), Some(t)) => t.keys_of(v),
        (_, Some(v), None) => c.picture.as_ref().map_or(&[], |p| p.keys_of(v)),
        (_, None, _) => Number::of(param).keys(c),
    }
}

fn param_name(param: KeyParam) -> String {
    match param {
        KeyParam::Speed => "speed".to_string(),
        KeyParam::CenterX => "center_x".to_string(),
        KeyParam::CenterY => "center_y".to_string(),
        KeyParam::Effect(q) => q.value.name(),
        _ => picture_param(param)
            .map_or("volume", PictureValue::name)
            .to_string(),
    }
}

fn effect_params(c: &Clip) -> impl Iterator<Item = EffectParam> + '_ {
    c.picture
        .iter()
        .flat_map(|x| x.effects.iter().enumerate())
        .flat_map(|(effect, e)| {
            e.keys.iter().map(move |k| EffectParam {
                effect,
                value: k.value,
            })
        })
}

pub type ChosenKeys = (KeyParam, Vec<i64>);

fn listed_params(c: &Clip) -> Vec<KeyParam> {
    let apart = c.title.as_ref().is_some_and(|t| t.upper.is_some());
    let plain = KeyParam::ALL.into_iter().filter(move |&param| {
        if param.on_line().1.is_some() {
            return apart;
        }
        !apart || param.for_line(LowerThirdLine::Lower) == param
    });
    plain
        .chain(effect_params(c).map(KeyParam::Effect))
        .collect()
}

pub fn chosen(p: &Project, id: &str, picks: Option<&[KeyPicks]>) -> Result<Vec<ChosenKeys>> {
    let whole = &holder(p, id)?;
    let wanted: Vec<(KeyParam, Option<Vec<i64>>)> = match picks {
        Some(picks) => picks
            .iter()
            .map(|k| (k.param, k.frames.as_ref().map(|f| f.to_vec())))
            .collect(),
        None => listed_params(whole)
            .into_iter()
            .filter(|&param| {
                let (value, line) = param.on_line();
                !keys_of(&of_line(whole, line), value).is_empty()
            })
            .map(|param| (param, None))
            .collect(),
    };
    let mut out: Vec<ChosenKeys> = Vec::new();
    for (param, frames) in wanted {
        if param == KeyParam::Speed {
            return Err(Error::invalid("speed keyframes don't copy"));
        }
        let (value, line) = param.on_line();
        let c = &of_line(whole, line);
        let keys = keys_of(c, value);
        if keys.is_empty() {
            return Err(Error::invalid(format!(
                "{id} has no {} keyframes",
                param_name(value).replace('_', " ")
            )));
        }
        let pick = Pick { frames, at: None };
        let mut taken: Vec<i64> = match picture_param(value) {
            Some(v) => picked_name(c, keys, &pick, v)?,
            None => picked_as(c, keys, &pick, &Number::of(value).name())?,
        }
        .into_iter()
        .collect();
        taken.sort_unstable();
        match out.iter_mut().find(|(q, _)| *q == param) {
            Some((_, have)) => {
                have.extend(taken);
                have.sort_unstable();
                have.dedup();
            }
            None => out.push((param, taken)),
        }
    }
    if out.is_empty() {
        return Err(Error::invalid(format!("{id} has no keyframes")));
    }
    Ok(out)
}

pub fn copy(p: &Project, id: &str, picks: &[ChosenKeys]) -> Result<Vec<CopiedKey>> {
    let whole = &holder(p, id)?;
    let mut rows = Vec::new();
    for (param, frames) in picks {
        let (plain, line) = param.on_line();
        let c = &of_line(whole, line);
        let held = held_picture(c);
        let keys = keys_of(c, plain);
        for (i, k) in keys
            .iter()
            .enumerate()
            .filter(|(_, k)| frames.contains(&k.frame))
        {
            let value = match (picture_param(plain), &held) {
                (Some(v), Some(picture)) if v.pair() => {
                    KeyValue::Pair(picture.key_value(v, k.frame).unwrap_or([k.value, 0.0]))
                }
                (None, _) if Number::of(plain).pair() => KeyValue::Pair([
                    k.value,
                    Number::of(plain).ys(c).get(i).copied().unwrap_or(0.0),
                ]),
                _ => KeyValue::Scalar(k.value),
            };
            let at = c.key_time(k.frame).round() as i64;
            let bent = c
                .title
                .as_ref()
                .filter(|_| plain == KeyParam::Position)
                .and_then(|t| t.handles_at(k.frame));
            rows.push((
                at,
                CopiedKey {
                    param: *param,
                    kind: match plain {
                        KeyParam::Effect(q) => effect_of(c, q).map(Effect::kind),
                        _ => None,
                    },
                    offset: 0,
                    value,
                    ease: ease_of(keys, i).filter(|_| bent.is_none()),
                    in_handle: k.in_handle,
                    out_handle: k.out_handle,
                    in_point: bent.and_then(|h| h.in_handle),
                    out_point: bent.and_then(|h| h.out_handle),
                },
            ));
        }
    }
    let first = rows.iter().map(|(at, _)| *at).min().unwrap_or(0);
    rows.sort_by_key(|(at, k)| (*at, k.param));
    Ok(rows
        .into_iter()
        .map(|(at, k)| CopiedKey {
            offset: at - first,
            ..k
        })
        .collect())
}

fn pastes_on_its_kind(c: &Clip, id: &str, k: &CopiedKey) -> Result<()> {
    let (KeyParam::Effect(q), Some(kind)) = (k.param.on_line().0, k.kind) else {
        return Ok(());
    };
    match effect_of(c, q) {
        Some(e) if e.kind() != kind => Err(Error::invalid(format!(
            "{id}'s effect {} is not the kind of the copied keyframe",
            q.effect
        ))),
        _ => Ok(()),
    }
}

pub fn paste(p: &mut Project, ids: &[String], at: i64, keys: &[CopiedKey]) -> Result<Vec<Value>> {
    let Some(first) = keys.iter().map(|k| k.offset).min() else {
        return Err(Error::usage("keyframes.paste needs keyframes"));
    };
    let mut starts = Vec::new();
    for id in ids {
        starts.push(holder(p, id)?.start);
    }
    let Some(earliest) = starts.iter().copied().min() else {
        return Err(Error::usage("keyframes.paste needs clips"));
    };
    for (id, k) in ids.iter().flat_map(|id| keys.iter().map(move |k| (id, k))) {
        pastes_on_its_kind(&holder(p, id)?, id, k)?;
    }
    let mut out = Vec::new();
    for (id, start) in ids.iter().zip(starts) {
        let one = std::slice::from_ref(id);
        let mut placed = Vec::new();
        let thirds = title_mut(p, id).is_some_and(|t| t.kind == TitleKind::LowerThird);
        for k in keys {
            let t = start + at - earliest + k.offset - first;
            let param = if thirds { k.param } else { k.param.on_line().0 };
            let added = on_line(p, one, param, |p, param| {
                add(p, one, param, t, Some(k.value), None)
            })?;
            for (_, frame) in added {
                placed.push((*k, frame));
            }
        }
        for (k, frame) in placed {
            let pick = Pick {
                frames: Some(vec![frame]),
                at: None,
            };
            let change = match k.ease {
                Some(e) => KeyChange {
                    ease: Some(e),
                    ..KeyChange::default()
                },
                None => KeyChange {
                    in_handle: Some(k.in_handle),
                    out_handle: Some(k.out_handle),
                    ..KeyChange::default()
                },
            };
            let param = if thirds { k.param } else { k.param.on_line().0 };
            on_line(p, one, param, |p, param| {
                set(p, one, param, &pick, &change)?;
                if param == KeyParam::Position
                    && let Some(t) = title_mut(p, id).filter(|t| !t.moves())
                {
                    t.put_handles(frame, k.in_point, k.out_point);
                }
                Ok(())
            })?;
            out.push(json!({ "clip": id, "param": k.param, "value": k.value, "frame": frame }));
        }
    }
    Ok(out)
}

fn picture_param(param: KeyParam) -> Option<PictureValue> {
    match param {
        KeyParam::Volume | KeyParam::CenterX | KeyParam::CenterY => None,
        KeyParam::Position | KeyParam::UpperPosition | KeyParam::LowerPosition => {
            Some(PictureValue::Position)
        }
        KeyParam::Zoom | KeyParam::UpperZoom | KeyParam::LowerZoom => Some(PictureValue::Zoom),
        KeyParam::Rotation | KeyParam::UpperRotation | KeyParam::LowerRotation => {
            Some(PictureValue::Rotation)
        }
        KeyParam::Anchor => Some(PictureValue::Anchor),
        KeyParam::Pitch => Some(PictureValue::Pitch),
        KeyParam::Yaw => Some(PictureValue::Yaw),
        KeyParam::CropLeft => Some(PictureValue::CropLeft),
        KeyParam::CropRight => Some(PictureValue::CropRight),
        KeyParam::CropTop => Some(PictureValue::CropTop),
        KeyParam::CropBottom => Some(PictureValue::CropBottom),
        KeyParam::CropSoftness => Some(PictureValue::CropSoftness),
        KeyParam::Opacity => Some(PictureValue::Opacity),
        KeyParam::Speed | KeyParam::Effect(_) => None,
    }
}

fn numbers(v: PictureValue, value: KeyValue) -> Result<[f64; 2]> {
    match (v.pair(), value) {
        (true, KeyValue::Pair(n)) => Ok(n),
        (true, KeyValue::Scalar(n)) if v == PictureValue::Zoom => Ok([n, n]),
        (false, KeyValue::Scalar(n)) => Ok([n, 0.0]),
        (true, KeyValue::Scalar(_)) => Err(Error::usage(format!("{} is x and y", v.name()))),
        (false, KeyValue::Pair(_)) => Err(Error::usage(format!("{} is one number", v.name()))),
    }
}

fn holds(c: &Clip) -> bool {
    super::picture::has_picture(c) || c.title.is_some()
}

fn held_picture(c: &Clip) -> Option<Picture> {
    c.title
        .as_ref()
        .map(crate::model::Title::as_picture)
        .or_else(|| c.picture.clone())
}

fn owned_picture(p: &Project, id: &str, v: PictureValue) -> Result<(Picture, Clip)> {
    if let Some(q) = p.cue(id) {
        if v == PictureValue::Anchor {
            return Err(Error::invalid(format!(
                "{id} is a subtitle, which has no anchor keyframes"
            )));
        }
        return Ok((q.picture.clone().unwrap_or_default(), q.as_clip()));
    }
    let (ti, ci) = locate(p, id)?;
    let c = &p.tracks[ti].clips[ci];
    if let Some(t) = &c.title {
        if !t.takes_keys(v) {
            return Err(Error::invalid(format!(
                "{id} is a {}, which has no {} keyframes",
                if t.moves() { "scroll" } else { "title" },
                v.name().replace('_', " ")
            )));
        }
        return Ok((t.as_picture(), c.clone()));
    }
    if !super::picture::has_picture(c) {
        return Err(Error::invalid(format!("{id} has no picture")));
    }
    Ok((
        c.picture.clone().unwrap_or_default(),
        Clip {
            picture: None,
            ..c.clone()
        },
    ))
}

fn title_mut<'a>(p: &'a mut Project, id: &str) -> Option<&'a mut Title> {
    let (ti, ci) = p.find_clip(id)?;
    p.tracks[ti].clips[ci].title.as_mut()
}

fn bends(c: &Clip, v: PictureValue) -> bool {
    v == PictureValue::Position && c.title.as_ref().is_some_and(|t| !t.moves())
}

fn ease_path(p: &mut Project, id: &str, frames: impl IntoIterator<Item = i64>, ease: Ease) {
    if let Some(t) = title_mut(p, id) {
        for frame in frames {
            t.ease_position(frame, ease);
        }
    }
}

fn save(p: &mut Project, id: &str, picture: &Picture) -> Result<()> {
    if let Some((ti, ci)) = p.find_cue(id) {
        super::subtitles::unlocked(p, ti)?;
        p.subtitles[ti].cues[ci].picture = super::picture::checked(picture)?;
        return Ok(());
    }
    let (ti, ci) = locate(p, id)?;
    let c = &mut p.tracks[ti].clips[ci];
    let Some(mut t) = c.title.clone() else {
        c.picture = super::picture::checked(picture)?;
        return Ok(());
    };
    if picture
        .zoom_keys
        .iter()
        .zip(&picture.zoom_y)
        .any(|(k, y)| k.value != *y)
    {
        return Err(Error::invalid("a title's zoom is one number"));
    }
    t.take_picture(picture);
    c.title = Some(super::titles::checked(&t)?);
    Ok(())
}

fn add_picture(
    p: &mut Project,
    ids: &[String],
    v: PictureValue,
    at: i64,
    value: Option<KeyValue>,
    ease: Option<Ease>,
) -> Result<Vec<(String, i64)>> {
    if v == PictureValue::Anchor && ease.is_some_and(|e| !e.is_linear()) {
        return Err(Error::invalid(ANCHOR_LINEAR));
    }
    let value = value.map(|n| numbers(v, n)).transpose()?;
    let targets: Vec<String> = ids
        .iter()
        .filter(|id| p.cue(id).is_some() || p.clip(id).is_some_and(holds))
        .cloned()
        .collect();
    if targets.is_empty() {
        for id in ids {
            locate(p, id)?;
        }
        return Err(Error::invalid(format!(
            "no clip of {} has a picture",
            ids.join(", ")
        )));
    }
    let mut added = Vec::new();
    for id in &targets {
        let (mut picture, c) = owned_picture(p, id, v)?;
        let frame = c.content_at(at);
        let n = value.unwrap_or_else(|| match (v, &c.title) {
            (PictureValue::Position, Some(t)) => t.place_at(frame as f64),
            _ => picture.channel_at(v, frame as f64),
        });
        picture.put_channel(v, frame, n);
        let path = bends(&c, v);
        if let Some(e) = ease.filter(|_| !path) {
            picture.mark_ease(v, frame, e);
        }
        save(p, id, &picture)?;
        if let Some(e) = ease.filter(|_| path) {
            ease_path(p, id, [frame], e);
        }
        added.push((id.clone(), frame));
    }
    Ok(added)
}

fn set_picture(
    p: &mut Project,
    ids: &[String],
    v: PictureValue,
    pick: &Pick,
    change: &KeyChange,
) -> Result<()> {
    let value = change.value.map(|n| numbers(v, n)).transpose()?;
    let by = change.by.map(|n| numbers(v, n)).transpose()?;
    if by
        .as_ref()
        .is_some_and(|b| b.iter().any(|n| !n.is_finite()))
    {
        return Err(Error::invalid("`by` needs finite numbers"));
    }
    if value.is_some() && by.is_some() {
        return Err(Error::usage("give `value` or `by`, not both"));
    }
    let handles = change.in_handle.is_some() || change.out_handle.is_some();
    if change.ease.is_some() && handles {
        return Err(Error::usage("give `ease` or handles, not both"));
    }
    let curved = change.ease.is_some_and(|e| !e.is_linear())
        || change.in_handle.flatten().is_some()
        || change.out_handle.flatten().is_some();
    if v == PictureValue::Anchor && curved {
        return Err(Error::invalid(ANCHOR_LINEAR));
    }
    let in_handle = change
        .in_handle
        .map(|h| h.map(|h| checked_handle(h, true)).transpose())
        .transpose()?;
    let out_handle = change
        .out_handle
        .map(|h| h.map(|h| checked_handle(h, false)).transpose())
        .transpose()?;
    for id in ids {
        let (mut picture, c) = owned_picture(p, id, v)?;
        if !picture.has_keys(v) {
            return Err(Error::invalid(format!(
                "{id} has no {} keyframes",
                v.name().replace('_', " ")
            )));
        }
        let chosen = picked_name(&c, picture.keys_of(v), pick, v)?;
        let path = bends(&c, v);
        for frame in &chosen {
            let mut next =
                value.unwrap_or_else(|| picture.key_value(v, *frame).unwrap_or([0.0, 0.0]));
            if let Some(by) = by {
                next = [next[0] + by[0], next[1] + by[1]];
            }
            if value.is_some() || by.is_some() {
                picture.put_channel(v, *frame, next);
            }
            if let Some(e) = change.ease.filter(|_| !path) {
                picture.mark_ease(v, *frame, e);
            }
            if let Some(h) = in_handle {
                picture.set_handle(v, *frame, true, h);
            }
            if let Some(h) = out_handle {
                picture.set_handle(v, *frame, false, h);
            }
        }
        save(p, id, &picture)?;
        if let Some(e) = change.ease.filter(|_| path) {
            ease_path(p, id, chosen, e);
        }
    }
    Ok(())
}

fn shift_picture(
    p: &mut Project,
    ids: &[String],
    v: PictureValue,
    pick: &Pick,
    by: Option<i64>,
    to: Option<i64>,
) -> Result<()> {
    if by.is_some() == to.is_some() {
        return Err(Error::usage("give `by` or `to`"));
    }
    for id in ids {
        let (mut picture, c) = owned_picture(p, id, v)?;
        if !picture.has_keys(v) {
            return Err(Error::invalid(format!(
                "{id} has no {} keyframes",
                v.name().replace('_', " ")
            )));
        }
        let chosen = picked_name(&c, picture.keys_of(v), pick, v)?;
        if to.is_some() && chosen.len() != 1 {
            return Err(Error::usage(format!(
                "`to` moves one keyframe; {id} has {} chosen",
                chosen.len()
            )));
        }
        let moved = |f: i64| match (by, to) {
            (Some(k), _) => c.key_at(c.key_time(f) + k as f64).round() as i64,
            (_, Some(t)) => c.content_at(t),
            _ => f,
        };
        let mut taken: HashSet<i64> = picture
            .keys_of(v)
            .iter()
            .map(|k| k.frame)
            .filter(|f| !chosen.contains(f))
            .collect();
        for f in &chosen {
            if !taken.insert(moved(*f)) {
                return Err(Error::invalid(format!(
                    "{id} already has a {} keyframe on source frame {}",
                    v.name().replace('_', " "),
                    moved(*f)
                )));
            }
        }
        picture.move_chosen(v, &chosen, moved);
        if v == PictureValue::Position
            && let Some(t) = title_mut(p, id)
        {
            t.move_handles(|f| if chosen.contains(&f) { moved(f) } else { f });
        }
        save(p, id, &picture)?;
    }
    Ok(())
}

fn remove_picture(p: &mut Project, ids: &[String], v: PictureValue, pick: &Pick) -> Result<()> {
    for id in ids {
        let (mut picture, c) = owned_picture(p, id, v)?;
        if !picture.has_keys(v) {
            return Err(Error::invalid(format!(
                "{id} has no {} keyframes",
                v.name().replace('_', " ")
            )));
        }
        let chosen = picked_name(&c, picture.keys_of(v), pick, v)?;
        picture.drop_chosen(v, &chosen);
        save(p, id, &picture)?;
    }
    Ok(())
}

fn picked_name(c: &Clip, keys: &[Keyframe], pick: &Pick, v: PictureValue) -> Result<HashSet<i64>> {
    picked_as(c, keys, pick, &v.name().replace('_', " "))
}

fn listed(p: &Project, c: &Clip, picture: &Picture, v: PictureValue) -> Vec<Value> {
    picture
        .keys_of(v)
        .iter()
        .enumerate()
        .map(|(i, k)| {
            let at = c.key_time(k.frame).round() as i64;
            let value = picture.key_value(v, k.frame).unwrap_or([k.value, 0.0]);
            let mut row = json!({
                "frame": k.frame,
                "at": at,
                "at_tc": p.settings.timecode(at),
                "value": if v.pair() { json!(value) } else { json!(value[0]) },
                "ease": ease_of(picture.keys_of(v), i),
            });
            if let Some(h) = k.in_handle {
                row["in"] = json!(h);
            }
            if let Some(h) = k.out_handle {
                row["out"] = json!(h);
            }
            let path = c.title.as_ref().filter(|_| v == PictureValue::Position);
            if let Some(t) = path
                && let Some(h) = t.handles_at(k.frame)
            {
                row["ease"] = json!(t.position_ease(k.frame));
                row["in"] = json!(h.in_handle);
                row["out"] = json!(h.out_handle);
            }
            row
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::keyframes::PictureValue;
    use crate::model::{Effect, EffectValue};
    use crate::test_support::Fixture;
    use serde_json::json;

    fn req(method: &str, params: serde_json::Value) -> serde_json::Value {
        json!({ "method": method, "params": params })
    }

    #[test]
    fn linear_zoom_through_keyframes_add_is_a_quarter_at_frame_15() {
        let mut f = Fixture::new(&[(10.0, true)]);
        f.append("m1", 0, 100);
        f.ok(req(
            "keyframes.add",
            json!({"clips": "c1", "param": "zoom", "at": 0, "value": 0.1}),
        ));
        f.ok(req(
            "keyframes.add",
            json!({"clips": "c1", "param": "zoom", "at": 30, "value": 0.4}),
        ));
        let picture = f
            .editor
            .project()
            .clip("c1")
            .and_then(|c| c.picture.clone())
            .unwrap_or_default();
        assert_eq!(picture.scale_at(15.0), [0.25, 0.25]);
        assert!(picture.has_keys(PictureValue::Zoom));
    }

    fn blurred() -> Fixture {
        let mut f = Fixture::new(&[(10.0, true)]);
        f.append("m1", 0, 100);
        f.ok(req(
            "effects.add",
            json!({"clips": "c1", "kind": "gaussian_blur"}),
        ));
        f
    }

    fn strength() -> serde_json::Value {
        json!({"effect": 0, "value": "strength"})
    }

    fn effect(f: &Fixture) -> Effect {
        f.editor
            .project()
            .clip("c1")
            .and_then(|c| c.picture.clone())
            .map(|p| p.effects[0].clone())
            .expect("an effect")
    }

    #[test]
    fn keyframes_add_keys_an_effect_value() {
        let mut f = blurred();
        f.ok(req(
            "keyframes.add",
            json!({"clips": "c1", "param": strength(), "at": 10, "value": 0.7}),
        ));
        assert_eq!(effect(&f).value_at(EffectValue::Strength, 10.0), 0.7);
    }

    #[test]
    fn a_keyframe_out_of_range_is_refused() {
        let mut f = blurred();
        let got = f.err(req(
            "keyframes.add",
            json!({"clips": "c1", "param": strength(), "at": 10, "value": 7.0}),
        ));
        assert!(got.message.contains("from 0 to 1"), "{}", got.message);
    }

    #[test]
    fn keyframes_add_refuses_a_value_of_another_kind() {
        let mut f = blurred();
        let got = f.err(req(
            "keyframes.add",
            json!({"clips": "c1", "param": {"effect": 0, "value": "gamma"}, "at": 10}),
        ));
        assert!(got.message.contains("that value"), "{}", got.message);
    }

    #[test]
    fn removing_the_last_key_leaves_no_list() {
        let mut f = blurred();
        f.ok(req(
            "keyframes.add",
            json!({"clips": "c1", "param": strength(), "at": 10}),
        ));
        f.ok(req(
            "keyframes.remove",
            json!({"clips": "c1", "param": strength()}),
        ));
        assert_eq!(effect(&f).keys, vec![]);
    }

    #[test]
    fn effects_set_at_puts_a_key_and_keeps_the_plain_value() {
        let mut f = blurred();
        f.ok(req(
            "keyframes.add",
            json!({"clips": "c1", "param": strength(), "at": 10, "value": 0.2}),
        ));
        f.ok(req(
            "effects.set",
            json!({"clip": "c1", "effect": 0, "at": 20, "strength": 0.9}),
        ));
        let e = effect(&f);
        assert_eq!(
            (
                e.value_at(EffectValue::Strength, 20.0),
                e.plain(EffectValue::Strength)
            ),
            (0.9, 0.4)
        );
    }

    #[test]
    fn effects_set_of_a_keyed_value_needs_at() {
        let mut f = blurred();
        f.ok(req(
            "keyframes.add",
            json!({"clips": "c1", "param": strength(), "at": 10}),
        ));
        let got = f.err(req(
            "effects.set",
            json!({"clip": "c1", "effect": 0, "strength": 0.9}),
        ));
        assert!(got.message.contains("give `at`"), "{}", got.message);
    }

    #[test]
    fn effects_key_keys_each_value_of_the_effect() {
        let mut f = blurred();
        f.ok(req(
            "effects.key",
            json!({"clip": "c1", "effect": 0, "at": 10}),
        ));
        assert_eq!(effect(&f).keys.len(), 11);
    }

    #[test]
    fn effects_set_keys_only_the_number_that_changes() {
        let mut f = blurred();
        f.ok(req(
            "effects.key",
            json!({"clip": "c1", "effect": 0, "at": 0}),
        ));
        f.ok(req(
            "effects.set",
            json!({"clip": "c1", "effect": 0, "at": 10, "horizontal_channels": [0.5, 1.0, 1.0, 1.0]}),
        ));
        assert_eq!(effect(&f).keys_of(EffectValue::HorizontalGreen).len(), 1);
    }

    #[test]
    fn a_retime_keeps_an_effect_key_at_its_place_in_the_clip() {
        let mut f = blurred();
        f.ok(req(
            "keyframes.add",
            json!({"clips": "c1", "param": strength(), "at": 10}),
        ));
        let old = f.editor.project().clip("c1").expect("a clip").clone();
        let mut new = old.clone();
        new.source_in = 30;
        let mut e = effect(&f);
        e.retime_keys(&old, &new);
        assert_eq!(e.keys_of(EffectValue::Strength)[0].frame, 40);
    }

    #[test]
    fn keyframes_paste_refuses_an_effect_of_another_kind() {
        let mut f = blurred();
        f.ok(req(
            "keyframes.add",
            json!({"clips": "c1", "param": {"effect": 0, "value": "blend"}, "at": 10}),
        ));
        let copied = f.ok(req("keyframes.copy", json!({"clip": "c1"}))).result;
        f.ok(req("effects.add", json!({"clips": "c1", "kind": "glow"})));
        f.ok(req(
            "effects.move",
            json!({"clip": "c1", "effect": 1, "to": 0}),
        ));
        let got = f.err(req(
            "keyframes.paste",
            json!({"clips": "c1", "at": 40, "keyframes": copied["keyframes"]}),
        ));
        assert!(got.message.contains("kind"), "{}", got.message);
    }

    #[test]
    fn effects_key_on_a_keyed_frame_removes_its_keys() {
        let mut f = blurred();
        let key = json!({"clip": "c1", "effect": 0, "at": 10});
        f.ok(req("effects.key", key.clone()));
        f.ok(req("effects.key", key));
        assert_eq!(effect(&f).keys, vec![]);
    }

    #[test]
    fn effects_key_with_an_ease_needs_a_key_there() {
        let mut f = blurred();
        let got = f.err(req(
            "effects.key",
            json!({"clip": "c1", "effect": 0, "at": 10, "ease": "ease_in"}),
        ));
        assert!(got.message.contains("no keyframes"), "{}", got.message);
    }

    #[test]
    fn a_list_names_the_effect_and_its_value() {
        let mut f = blurred();
        f.ok(req(
            "keyframes.add",
            json!({"clips": "c1", "param": strength(), "at": 10}),
        ));
        let got = f.ok(req("keyframes.list", json!({"clips": "c1"}))).result;
        assert_eq!(got["clips"][0]["param"], strength());
    }

    #[test]
    fn a_copied_effect_key_pastes_later() {
        let mut f = blurred();
        f.ok(req(
            "keyframes.add",
            json!({"clips": "c1", "param": strength(), "at": 10, "value": 0.7}),
        ));
        let copied = f.ok(req("keyframes.copy", json!({"clip": "c1"}))).result;
        f.ok(req(
            "keyframes.paste",
            json!({"clips": "c1", "at": 40, "keyframes": copied["keyframes"]}),
        ));
        assert_eq!(effect(&f).value_at(EffectValue::Strength, 40.0), 0.7);
    }

    #[test]
    fn a_reset_removes_the_keys() {
        let mut f = blurred();
        f.ok(req(
            "keyframes.add",
            json!({"clips": "c1", "param": strength(), "at": 10}),
        ));
        f.ok(req("effects.reset", json!({"clip": "c1", "effect": 0})));
        assert_eq!(effect(&f).keys, vec![]);
    }
}
