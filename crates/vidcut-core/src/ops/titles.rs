use crate::error::{Error, Result};
use crate::keyframes::{PictureValue, plain_channel, title_keys_problem};
use crate::looks::TitleLook;
use crate::model::{
    Clip, Generator, Keyframe, LowerThirdLine, Picture, PositionHandles, Project, Title,
    TitleAlign, TitleKind, TitleShadow, TitleStyle, Track, TrackKind,
};

use super::subtitles::{checked_background, checked_color, checked_font, checked_outline};
use super::{clear_range, fix_audio, is_main, locate, place, tracks, unlocked};

pub const DEFAULT_SECS: f64 = 5.0;

const TEXT_MOST: usize = 2000;
const ZOOM: (f64, f64) = (0.25, 4.0);

const PLACE: &str =
    "a title's position is two fractions of the picture, from -4 to 4 across and -3 to 5 down";
const ZOOMED: &str = "a title's zoom is from 1/4 to 4";
const TURNED: &str = "a title's rotation is degrees, from -100000 to 100000";
const FADED: &str = "a title's opacity is from 0 to 1";
const POSITION_HANDLED: &str =
    "a title's position keyframes take `ease`, not `in` or `out`: their handles are points";
const SCROLL_HANDLES: &str = "a scroll's position keyframes have no handles";
const LOOSE_HANDLES: &str = "a title's position handles need finite numbers and their keyframe";

fn across(v: f64) -> bool {
    (Title::ACROSS.0..=Title::ACROSS.1).contains(&v)
}

fn down(v: f64) -> bool {
    (Title::DOWN.0..=Title::DOWN.1).contains(&v)
}

fn zooms(z: f64) -> bool {
    z.is_finite() && (ZOOM.0..=ZOOM.1).contains(&z)
}

fn turns(r: f64) -> bool {
    r.abs() <= Picture::TURNS
}

fn fades(o: f64) -> bool {
    o.is_finite() && (0.0..=1.0).contains(&o)
}

fn all_in(keys: &[Keyframe], ok: fn(f64) -> bool, why: &str) -> Result<()> {
    if keys.iter().all(|k| ok(k.value)) {
        Ok(())
    } else {
        Err(Error::invalid(why))
    }
}

fn checked_keys(t: &Title) -> Result<()> {
    if let Some(why) = title_keys_problem(&t.as_picture()) {
        return Err(Error::invalid(why));
    }
    if t.kind == TitleKind::Scroll && (!t.zoom_keys.is_empty() || !t.rotation_keys.is_empty()) {
        return Err(Error::invalid("a scroll has no zoom or rotation"));
    }
    if t.position_keys
        .iter()
        .any(|k| k.in_handle.is_some() || k.out_handle.is_some())
    {
        return Err(Error::invalid(match t.kind {
            TitleKind::Scroll => SCROLL_HANDLES,
            TitleKind::Text | TitleKind::LowerThird => POSITION_HANDLED,
        }));
    }
    if t.kind == TitleKind::Scroll && !t.position_handles.is_empty() {
        return Err(Error::invalid(SCROLL_HANDLES));
    }
    let keyed = |h: &PositionHandles| t.position_keys.iter().any(|k| k.frame == h.frame);
    let whole = |h: &PositionHandles| {
        [h.in_handle, h.out_handle]
            .into_iter()
            .flatten()
            .flatten()
            .all(f64::is_finite)
    };
    if !t.position_handles.iter().all(|h| keyed(h) && whole(h)) {
        return Err(Error::invalid(LOOSE_HANDLES));
    }
    all_in(&t.position_keys, across, PLACE)?;
    if !t.position_y.iter().copied().all(down) {
        return Err(Error::invalid(PLACE));
    }
    all_in(&t.zoom_keys, zooms, ZOOMED)?;
    all_in(&t.rotation_keys, turns, TURNED)?;
    all_in(&t.opacity_keys, fades, FADED)
}

pub fn checked(t: &Title) -> Result<Title> {
    let text = t.text.replace("\r\n", "\n").replace('\r', "\n");
    if text.chars().count() > TEXT_MOST {
        return Err(Error::invalid(format!(
            "a title's text is at most {TEXT_MOST} characters"
        )));
    }
    if text
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(Error::invalid(
            "a title's text holds no control characters but line breaks and tabs",
        ));
    }
    if t.kind == TitleKind::LowerThird && text.lines().count() > 2 {
        return Err(Error::invalid("a lower third has two lines at the most"));
    }
    if t.kind == TitleKind::Text && t.align.is_some() {
        return Err(Error::invalid(
            "align is a lower third's or a scroll's: left, middle or right",
        ));
    }
    if t.kind == TitleKind::Scroll
        && (t.zoom.is_some_and(|z| z != 1.0) || t.rotation.is_some_and(|r| r != 0.0))
    {
        return Err(Error::invalid("a scroll has no zoom or rotation"));
    }
    if let Some(at) = t.position
        && !Title::places(at)
    {
        return Err(Error::invalid(PLACE));
    }
    let zoom = match t.zoom {
        Some(z) if zooms(z) => (z != 1.0).then_some(z),
        Some(_) => return Err(Error::invalid(ZOOMED)),
        None => None,
    };
    let rotation = match t.rotation {
        Some(r) if turns(r) => (r != 0.0).then_some(r),
        Some(_) => return Err(Error::invalid(TURNED)),
        None => None,
    };
    let opacity = match t.opacity {
        Some(o) if fades(o) => (o != 1.0).then_some(o),
        Some(_) => return Err(Error::invalid(FADED)),
        None => None,
    };
    checked_keys(t)?;
    let upper = match &t.upper {
        Some(_) if t.kind != TitleKind::LowerThird => {
            return Err(Error::invalid("only a lower third has lines apart"));
        }
        Some(_) => {
            let mut high = t.clone();
            high.swap_lines();
            high.upper = None;
            Some(checked(&high)?.line())
        }
        None => None,
    };
    let s = &t.style;
    let style = TitleStyle {
        font: checked_font(s.font.as_deref(), "title")?,
        size: super::subtitles::checked_size(s.size, "title", Title::MOST_SIZE)?,
        color: checked_color(s.color.as_deref(), "title")?,
        outline: checked_outline(s.outline.as_ref())?,
        background: checked_background(s.background.as_ref())?,
        shadow: s.shadow.as_ref().map(checked_shadow).transpose()?,
    };
    Ok(Title {
        kind: t.kind,
        align: t.align,
        text,
        style,
        position: t.position,
        position_keys: t.position_keys.clone(),
        position_y: t.position_y.clone(),
        position_handles: t.position_handles.clone(),
        zoom,
        zoom_keys: t.zoom_keys.clone(),
        rotation,
        rotation_keys: t.rotation_keys.clone(),
        opacity,
        opacity_keys: t.opacity_keys.clone(),
        upper,
    })
}

pub(crate) fn checked_shadow(s: &TitleShadow) -> Result<TitleShadow> {
    let opacity = match s.opacity {
        Some(o) if o.is_finite() && (0.0..=1.0).contains(&o) => Some(o),
        Some(_) => return Err(Error::invalid("a shadow's opacity is from 0 to 1")),
        None => None,
    };
    let offset = match s.offset {
        Some(at) if at.iter().all(|v| v.abs() <= Picture::REACH) => Some(at),
        Some(_) => {
            return Err(Error::invalid(
                "a shadow's offset is two fractions of the picture, each from -4 to 4",
            ));
        }
        None => None,
    };
    let blur = match s.blur {
        Some(b) if (1.0..=100.0).contains(&b) => Some(b),
        Some(_) => return Err(Error::invalid("a shadow's blur is from 1 to 100")),
        None => None,
    };
    Ok(TitleShadow {
        color: checked_color(s.color.as_deref(), "shadow")?,
        opacity,
        offset,
        blur,
        off: s.off.filter(|&o| o),
    })
}

pub fn default_length(p: &Project) -> i64 {
    p.settings.fps.secs_to_frames(DEFAULT_SECS).max(1)
}

pub fn add(
    p: &mut Project,
    track: Option<&str>,
    start: i64,
    duration: i64,
    title: &Title,
) -> Result<(String, String)> {
    let title = checked(title)?;
    add_block(p, "title", track, (start, duration), Some(title), None)
}

pub(super) fn add_block(
    p: &mut Project,
    what: &str,
    track: Option<&str>,
    (start, duration): (i64, i64),
    title: Option<Title>,
    generator: Option<Generator>,
) -> Result<(String, String)> {
    if start < 0 {
        return Err(Error::invalid(format!("a {what} can't start before 0")));
    }
    if duration < 1 {
        return Err(Error::invalid(format!("a {what} lasts at least one frame")));
    }
    let end = start + duration;
    let ti = match track {
        Some(id) => {
            let ti = tracks::resolve(p, id)?;
            if p.tracks[ti].kind != TrackKind::Video {
                return Err(Error::invalid(format!(
                    "a {what} goes on a video track, not {id}"
                )));
            }
            ti
        }
        None => {
            let busy = |t: &Track| t.clips.iter().any(|c| c.start < end && start < c.end());
            let video = p.tracks_of(TrackKind::Video);
            let top = video.iter().rposition(|&i| busy(&p.tracks[i]));
            tracks::nth(p, TrackKind::Video, top.map_or(0, |k| k + 1))
        }
    };
    unlocked(p, ti)?;
    let start = if is_main(p, ti) && p.settings.ripple_on {
        start.min(p.tracks[ti].end())
    } else {
        start
    };
    clear_range(p, ti, start, start + duration);
    let id = p.alloc_clip_id();
    place(
        p,
        ti,
        Clip {
            id: id.clone(),
            media: String::new(),
            start,
            duration,
            source_in: 0,
            video: true,
            audio: None,
            enabled: true,
            name: None,
            effects: None,
            generator,
            title,
            picture: None,
            compound: None,
            speed: None,
            retime: crate::model::ClipRetime::Project,
            color: None,
            render_cache: crate::model::ClipCache::Auto,
        },
    );
    fix_audio(p, ti);
    Ok((id, p.tracks[ti].id.clone()))
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TitleChange {
    pub text: Option<String>,
    pub kind: Option<TitleKind>,
    pub align: Option<TitleAlign>,
    pub style: Option<TitleStyle>,
    pub position: Option<Option<[f64; 2]>>,
    pub zoom: Option<f64>,
    pub rotation: Option<f64>,
    pub opacity: Option<f64>,
    pub look: Option<TitleLook>,
    pub line: Option<LowerThirdLine>,
}

pub fn changed(old: &Title, change: &TitleChange) -> Title {
    let mut t = match &change.look {
        Some(l) => Title {
            position_keys: old.position_keys.clone(),
            position_y: old.position_y.clone(),
            position_handles: old.position_handles.clone(),
            zoom_keys: old.zoom_keys.clone(),
            rotation_keys: old.rotation_keys.clone(),
            opacity_keys: old.opacity_keys.clone(),
            upper: old.upper.clone(),
            ..l.on(&old.text)
        },
        None => old.clone(),
    };
    if let Some(k) = change.kind {
        if k == TitleKind::Text {
            t.align = None;
        }
        t.kind = k;
    }
    if t.kind != TitleKind::LowerThird {
        t.upper = None;
    }
    if t.kind == TitleKind::Scroll {
        t.position_handles.clear();
        t.zoom_keys.clear();
        t.rotation_keys.clear();
    }
    if let Some(a) = change.align {
        t.align = Some(a);
    }
    if let Some(text) = &change.text {
        t.text.clone_from(text);
    }
    if let Some(s) = &change.style {
        t.style = s.clone();
    }
    if let Some(at) = change.position {
        t.position = at;
    }
    if let Some(z) = change.zoom {
        t.zoom = Some(z);
    }
    if let Some(r) = change.rotation {
        t.rotation = Some(r);
    }
    if let Some(o) = change.opacity {
        t.opacity = Some(o);
    }
    t
}

impl TitleChange {
    fn names(&self, v: PictureValue) -> bool {
        match v {
            PictureValue::Position => self.position.is_some(),
            PictureValue::Zoom => self.zoom.is_some(),
            PictureValue::Rotation => self.rotation.is_some(),
            PictureValue::Opacity => self.opacity.is_some(),
            _ => false,
        }
    }
}

fn line_of<'a>(p: &'a mut Project, id: &str) -> Result<&'a mut Title> {
    let not_one = || Error::invalid(format!("{id} is not a lower third"));
    let (ti, ci) = p.find_clip(id).ok_or_else(not_one)?;
    p.tracks[ti].clips[ci]
        .title
        .as_mut()
        .filter(|t| t.kind == TitleKind::LowerThird)
        .ok_or_else(not_one)
}

pub fn on_line<T>(
    p: &mut Project,
    ids: &[String],
    line: Option<LowerThirdLine>,
    work: impl FnOnce(&mut Project) -> Result<T>,
) -> Result<T> {
    let Some(line) = line else {
        return work(p);
    };
    for id in ids {
        line_of(p, id)?.part_lines();
    }
    let swap = |p: &mut Project| -> Result<()> {
        if line == LowerThirdLine::Upper {
            for id in ids {
                line_of(p, id)?.swap_lines();
            }
        }
        Ok(())
    };
    swap(p)?;
    let out = work(p);
    swap(p)?;
    out
}

pub fn set(p: &mut Project, id: &str, change: &TitleChange, at: Option<i64>) -> Result<()> {
    if change.line.is_some() && (change.kind.is_some() || change.look.is_some()) {
        return Err(Error::invalid("`line` does not go with `kind` or `look`"));
    }
    let one = [id.to_string()];
    on_line(p, &one, change.line, |p| set_values(p, id, change, at))
}

fn set_values(p: &mut Project, id: &str, change: &TitleChange, at: Option<i64>) -> Result<()> {
    let (ti, ci) = locate(p, id)?;
    let c = &p.tracks[ti].clips[ci];
    let Some(old) = c.title.as_ref() else {
        return Err(Error::invalid(format!("{id} is not a title")));
    };
    let frame = at.map(|t| c.content_at(t));
    let mut next = changed(old, change);
    if next.is_keyed() {
        let mut picture = next.as_picture();
        for v in PictureValue::ALL {
            if change.names(v) && !next.keys_of(v).is_empty() {
                let Some(f) = frame else {
                    return Err(Error::invalid(
                        "a value with keyframes needs `at` or a playhead",
                    ));
                };
                let now = plain_channel(&picture, v);
                picture.put_channel(v, f, now);
            }
        }
        next.take_picture(&picture);
    }
    let t = checked(&next)?;
    p.tracks[ti].clips[ci].title = Some(t);
    Ok(())
}

pub fn position(raw: &[f64]) -> Result<Option<[f64; 2]>> {
    match raw {
        [] => Ok(None),
        [x, y] => Ok(Some([*x, *y])),
        _ => Err(Error::usage(
            "a title's position is two fractions, x and y, or none for the usual place",
        )),
    }
}
