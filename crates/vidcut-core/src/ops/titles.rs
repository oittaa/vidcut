use crate::error::{Error, Result};
use crate::model::{
    Clip, Project, Title, TitleAlign, TitleKind, TitleShadow, TitleStyle, Track, TrackKind,
};

use super::subtitles::{checked_background, checked_color, checked_font, checked_outline};
use super::{clear_range, fix_audio, is_main, locate, place, tracks, unlocked};

pub const DEFAULT_SECS: f64 = 5.0;

const TEXT_MOST: usize = 2000;

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
        && !at.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v))
    {
        return Err(Error::invalid(
            "a title's position is two fractions of the picture, from 0 to 1",
        ));
    }
    let zoom = match t.zoom {
        Some(z) if z.is_finite() && (0.25..=4.0).contains(&z) => (z != 1.0).then_some(z),
        Some(_) => return Err(Error::invalid("a title's zoom is from 1/4 to 4")),
        None => None,
    };
    let rotation = match t.rotation {
        Some(r) if r.is_finite() && r.abs() <= 3600.0 => (r != 0.0).then_some(r),
        Some(_) => {
            return Err(Error::invalid(
                "a title's rotation is degrees, from -3600 to 3600",
            ));
        }
        None => None,
    };
    let opacity = match t.opacity {
        Some(o) if o.is_finite() && (0.0..=1.0).contains(&o) => (o != 1.0).then_some(o),
        Some(_) => return Err(Error::invalid("a title's opacity is from 0 to 1")),
        None => None,
    };
    let s = &t.style;
    let style = TitleStyle {
        font: checked_font(s.font.as_deref(), "title")?,
        size: super::subtitles::checked_size(s.size, "title")?,
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
        zoom,
        rotation,
        opacity,
    })
}

fn checked_shadow(s: &TitleShadow) -> Result<TitleShadow> {
    let opacity = match s.opacity {
        Some(o) if o.is_finite() && (0.0..=1.0).contains(&o) => Some(o),
        Some(_) => return Err(Error::invalid("a shadow's opacity is from 0 to 1")),
        None => None,
    };
    let offset = match s.offset {
        Some(at) if at.iter().all(|v| v.is_finite() && v.abs() <= 0.5) => Some(at),
        Some(_) => {
            return Err(Error::invalid(
                "a shadow's offset is two fractions of the picture, each from -1/2 to 1/2",
            ));
        }
        None => None,
    };
    let blur = match s.blur {
        Some(b) if b.is_finite() && (0.0..=100.0).contains(&b) => Some(b),
        Some(_) => return Err(Error::invalid("a shadow's blur is from 0 to 100")),
        None => None,
    };
    Ok(TitleShadow {
        color: checked_color(s.color.as_deref(), "shadow")?,
        opacity,
        offset,
        blur,
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
    if start < 0 {
        return Err(Error::invalid("a title can't start before 0"));
    }
    if duration < 1 {
        return Err(Error::invalid("a title lasts at least one frame"));
    }
    let title = checked(title)?;
    let end = start + duration;
    let ti = match track {
        Some(id) => {
            let ti = tracks::resolve(p, id)?;
            if p.tracks[ti].kind != TrackKind::Video {
                return Err(Error::invalid(format!(
                    "a title goes on a video track, not {id}"
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
            generator: None,
            title: Some(title),
            compound: None,
            speed: None,
            color: None,
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
}

pub fn set(p: &mut Project, id: &str, change: &TitleChange) -> Result<()> {
    let (ti, ci) = locate(p, id)?;
    let Some(old) = p.tracks[ti].clips[ci].title.as_ref() else {
        return Err(Error::invalid(format!("{id} is not a title")));
    };
    let mut t = old.clone();
    if let Some(k) = change.kind {
        if k == TitleKind::Text {
            t.align = None;
        }
        t.kind = k;
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
    let t = checked(&t)?;
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
