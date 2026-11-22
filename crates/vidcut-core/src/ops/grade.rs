use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::model::{
    Clip, Corrector, Grade, GradeKey, GradeNode, GradeTool, GradeWindow, KeyInput, KeyMix,
    KeyOutput, Project, WindowKind,
};
use crate::preferences::ColorPrefs;
use crate::request::{
    CopiedNode, GradeAdd, GradeChange, GradeMixer, GradeNodeChange, GradePalette, PrinterColor,
};

use super::locate;
use super::picture::takes;

impl GradeChange {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn changed(&self, old: &Corrector) -> Corrector {
        let mut g = old.clone();
        let put = |to: &mut f64, v: Option<f64>| {
            if let Some(v) = v {
                *to = v;
            }
        };
        if let Some(v) = self.lift {
            g.lift = v;
        }
        if let Some(v) = self.gamma {
            g.gamma = v;
        }
        if let Some(v) = self.gain {
            g.gain = v;
        }
        if let Some(v) = self.offset {
            g.offset = v;
        }
        if let Some(v) = self.log_shadow {
            g.log_shadow = v;
        }
        if let Some(v) = self.log_midtone {
            g.log_midtone = v;
        }
        if let Some(v) = self.log_highlight {
            g.log_highlight = v;
        }
        put(&mut g.contrast, self.contrast);
        put(&mut g.pivot, self.pivot);
        put(&mut g.saturation, self.saturation);
        put(&mut g.hue, self.hue);
        put(&mut g.lum_mix, self.lum_mix);
        put(&mut g.temp, self.temp);
        put(&mut g.tint, self.tint);
        put(&mut g.color_boost, self.color_boost);
        put(&mut g.shadows, self.shadows);
        put(&mut g.highlights, self.highlights);
        put(&mut g.midtone_detail, self.midtone_detail);
        put(&mut g.low_range, self.low_range);
        put(&mut g.high_range, self.high_range);
        g
    }
}

impl GradeNodeChange {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    fn changes_key(&self) -> bool {
        self.key_gain.is_some() || self.key_offset.is_some() || self.key_invert.is_some()
    }

    fn key_changed(&self, k: KeyOutput) -> KeyOutput {
        KeyOutput {
            gain: self.key_gain.unwrap_or(k.gain),
            offset: self.key_offset.unwrap_or(k.offset),
            invert: self.key_invert.unwrap_or(k.invert),
        }
    }

    fn changes_key_input(&self) -> bool {
        let flags = [self.key_input_invert, self.key_input_mask];
        let values = [
            self.key_input_gain,
            self.key_input_offset,
            self.key_input_blur,
            self.key_input_blur_ratio,
        ];
        flags.iter().any(Option::is_some) || values.iter().any(Option::is_some)
    }

    fn key_input_changed(&self, i: KeyInput) -> KeyInput {
        KeyInput {
            invert: self.key_input_invert.unwrap_or(i.invert),
            mask: self.key_input_mask.unwrap_or(i.mask),
            gain: self.key_input_gain.unwrap_or(i.gain),
            offset: self.key_input_offset.unwrap_or(i.offset),
            blur: self.key_input_blur.unwrap_or(i.blur),
            blur_ratio: self.key_input_blur_ratio.unwrap_or(i.blur_ratio),
        }
    }

    fn changes_mix(&self) -> bool {
        let flags = [self.mix_invert, self.mix_matte];
        let values = [self.mix_gain, self.mix_offset];
        flags.iter().any(Option::is_some) || values.iter().any(Option::is_some)
    }

    fn mix_changed(&self, m: KeyMix) -> KeyMix {
        KeyMix {
            invert: self.mix_invert.unwrap_or(m.invert),
            matte: self.mix_matte.unwrap_or(m.matte),
            gain: self.mix_gain.unwrap_or(m.gain),
            offset: self.mix_offset.unwrap_or(m.offset),
        }
    }
}

fn within(v: f64, (low, high): (f64, f64), what: &str) -> Result<()> {
    match v.is_finite() && (low..=high).contains(&v) {
        true => Ok(()),
        false => Err(Error::invalid(format!(
            "{what} must be from {low} to {high}"
        ))),
    }
}

fn finite(values: &[f64], what: &str) -> Result<()> {
    match values.iter().all(|v| v.is_finite()) {
        true => Ok(()),
        false => Err(Error::invalid(format!("{what} must be a number"))),
    }
}

pub fn corrector_checked(g: &Corrector) -> Result<()> {
    finite(&g.lift, "lift")?;
    finite(&g.gamma, "gamma")?;
    finite(&g.gain, "gain")?;
    finite(&g.offset, "offset")?;
    finite(&g.log_shadow, "log_shadow")?;
    finite(&g.log_midtone, "log_midtone")?;
    finite(&g.log_highlight, "log_highlight")?;
    within(g.contrast, (0.0, 2.0), "contrast")?;
    within(g.pivot, (0.0, 1.0), "pivot")?;
    within(g.saturation, (0.0, 100.0), "saturation")?;
    within(g.hue, (0.0, 100.0), "hue")?;
    within(g.lum_mix, (0.0, 100.0), "lum_mix")?;
    within(g.temp, (-4000.0, 4000.0), "temp")?;
    within(g.tint, (-100.0, 100.0), "tint")?;
    within(g.color_boost, (-100.0, 100.0), "color_boost")?;
    within(g.shadows, (-100.0, 100.0), "shadows")?;
    within(g.highlights, (-100.0, 100.0), "highlights")?;
    within(g.midtone_detail, (-100.0, 100.0), "midtone_detail")?;
    if !(g.low_range > 0.0 && g.low_range < g.high_range && g.high_range < 1.0) {
        return Err(Error::invalid(
            "low_range must be above 0, below high_range, and high_range below 1",
        ));
    }
    Ok(())
}

fn keys_checked(d: &GradeNode, k: u32) -> Result<()> {
    if d.keys.is_empty() {
        return Ok(());
    }
    if d.tool.is_mixer() {
        return Err(Error::invalid(format!(
            "node {k} is a mixer, which takes no keyframes"
        )));
    }
    if d.keys.windows(2).any(|w| w[0].frame >= w[1].frame) {
        return Err(Error::invalid(format!(
            "the keyframes of node {k} must be in the order of their frames, one on a frame"
        )));
    }
    let last = crate::grade_keys::LAST_PROFILE;
    for key in &d.keys {
        if key.start > last || key.end > last {
            return Err(Error::invalid(format!(
                "start and end must be from 0 to {last}"
            )));
        }
        corrector_checked(&key.values)?;
        key_checked(&key.key)?;
    }
    Ok(())
}

fn key_checked(k: &KeyOutput) -> Result<()> {
    within(k.gain, (0.0, 2.0), "key_gain")?;
    within(k.offset, (-1.0, 1.0), "key_offset")
}

fn key_input_checked(i: &KeyInput) -> Result<()> {
    within(i.gain, (0.0, 2.0), "key_input_gain")?;
    within(i.offset, (-1.0, 1.0), "key_input_offset")?;
    within(i.blur, (-1.0, 1.0), "key_input_blur")?;
    within(i.blur_ratio, (-1.0, 1.0), "key_input_blur_ratio")
}

fn links_checked(g: &Grade, n: &GradeNode, k: u32) -> Result<()> {
    let last = g.last();
    if let Some(v) = n.links().find(|v| *v > last || *v == k) {
        return Err(Error::invalid(format!("node {k} cannot take node {v}")));
    }
    let keyed = |v: u32| {
        (g.node(v)).is_some_and(|d| matches!(d.tool, GradeTool::Corrector(_)) || d.takes_keys())
    };
    let keys = n.inputs.iter().flatten().filter(|_| n.takes_keys());
    if let Some(v) = keys.copied().chain(n.key_from).find(|v| !keyed(*v)) {
        return Err(Error::invalid(format!(
            "node {k} cannot take the key of node {v}: a corrector or a key mixer gives a key"
        )));
    }
    let mut pictures = n.inputs.iter().flatten().filter(|_| !n.takes_keys());
    if let Some(v) = pictures.find(|v| g.node(**v).is_some_and(GradeNode::takes_keys)) {
        return Err(Error::invalid(format!(
            "key mixer {v} gives out no picture"
        )));
    }
    if n.tool.is_mixer() && (n.key_from.is_some() || !n.key_input.is_usual()) {
        return Err(Error::invalid(format!("mixer {k} has no key input")));
    }
    if let GradeTool::KeyMixer { mix } = &n.tool {
        if mix.len() != n.inputs.len() {
            return Err(Error::invalid(format!(
                "key mixer {k} has one mix for each input"
            )));
        }
        for m in mix {
            within(m.gain, (0.0, 2.0), "mix_gain")?;
            within(m.offset, (-1.0, 1.0), "mix_offset")?;
        }
    }
    key_input_checked(&n.key_input)
}

pub fn checked(g: &Grade) -> Result<()> {
    let last = g.last();
    if let Some(o) = g.output.filter(|o| *o > last) {
        return Err(no_node(o));
    }
    if let Some(o) = g
        .output
        .filter(|o| g.node(*o).is_some_and(GradeNode::takes_keys))
    {
        return Err(Error::invalid(format!(
            "key mixer {o} gives out no picture"
        )));
    }
    for (n, k) in g.nodes.iter().zip(1..) {
        links_checked(g, n, k)?;
        finite(&n.place, "place")?;
        match &n.tool {
            GradeTool::Corrector(c) => {
                if n.inputs.len() != 1 {
                    return Err(Error::invalid(format!("corrector {k} has one input")));
                }
                let kinds = n.windows.iter().map(|w| w.shape.kind());
                if !kinds.take(WindowKind::ALL.len()).eq(WindowKind::ALL) {
                    return Err(Error::invalid(format!(
                        "corrector {k} starts its windows with a linear, a circle, a polygon, a curve and a gradient window"
                    )));
                }
                corrector_checked(c)?;
            }
            _ if n.inputs.is_empty() => {
                return Err(Error::invalid(format!("mixer {k} needs an input")));
            }
            _ => {}
        }
        if n.tool.is_mixer() && !n.takes_keys() && !n.key.is_usual() {
            return Err(Error::invalid(format!("mixer {k} gives out no key")));
        }
        key_checked(&n.key)?;
        keys_checked(n, k)?;
        if n.tool.is_mixer() && !n.windows.is_empty() {
            return Err(Error::invalid(format!("mixer {k} has no windows")));
        }
        for w in &n.windows {
            super::grade_windows::checked(w)?;
        }
    }
    if crate::grade::order(g).is_none() {
        return Err(Error::invalid("the links of the grade make a loop"));
    }
    Ok(())
}

impl PrinterColor {
    pub fn channels(self) -> [f64; 3] {
        match self {
            Self::Red => [1.0, 0.0, 0.0],
            Self::Green => [0.0, 1.0, 0.0],
            Self::Blue => [0.0, 0.0, 1.0],
            Self::Cyan => [0.0, 1.0, 1.0],
            Self::Magenta => [1.0, 0.0, 1.0],
            Self::Yellow => [1.0, 1.0, 0.0],
            Self::Master => [1.0; 3],
        }
    }
}

pub fn printed(
    offset: [f64; 3],
    (color, points): (PrinterColor, f64),
    lights: &ColorPrefs,
) -> [f64; 3] {
    let on = color.channels();
    std::array::from_fn(|i| {
        offset[i] + points * on[i] * lights.printer_step[i] * lights.printer_density[i]
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GradeOf {
    Timeline,
    Clip(String),
}

fn grades(p: &Project, ti: usize, c: &Clip) -> Result<()> {
    takes(p, ti, c)?;
    if c.generator.is_some() {
        return Err(Error::invalid(format!(
            "{} is a generator, which takes no grade",
            c.id
        )));
    }
    Ok(())
}

fn no_timeline() -> Error {
    Error::invalid("the project has no timeline: make one with timeline.new")
}

pub fn frame_of(p: &Project, of: &GradeOf, at: i64) -> Result<i64> {
    match of {
        GradeOf::Timeline => Ok(at),
        GradeOf::Clip(id) => {
            let (ti, ci) = locate(p, id)?;
            Ok(p.tracks[ti].clips[ci].content_at(at))
        }
    }
}

pub fn stored(p: &Project, of: &GradeOf) -> Result<Grade> {
    let grade = match of {
        GradeOf::Timeline => {
            if !p.compound_path.is_empty() {
                return Err(Error::invalid("a compound clip has no timeline grade"));
            }
            p.timeline().ok_or_else(no_timeline)?.grade.as_ref()
        }
        GradeOf::Clip(id) => {
            let (ti, ci) = locate(p, id)?;
            let c = &p.tracks[ti].clips[ci];
            grades(p, ti, c)?;
            c.picture.as_ref().and_then(|pic| pic.grade.as_ref())
        }
    };
    Ok(grade
        .cloned()
        .unwrap_or_else(|| Grade::usual(p.frame_size())))
}

fn store(p: &mut Project, of: &GradeOf, grade: &Grade) -> Result<()> {
    checked(grade)?;
    let grade = (!grade.is_usual(p.frame_size())).then(|| grade.clone());
    match of {
        GradeOf::Timeline => {
            if !p.compound_path.is_empty() {
                return Err(Error::invalid("a compound clip has no timeline grade"));
            }
            p.timeline_mut().ok_or_else(no_timeline)?.grade = grade;
        }
        GradeOf::Clip(id) => {
            let (ti, ci) = locate(p, id)?;
            let c = &mut p.tracks[ti].clips[ci];
            let mut picture = c.picture.take().unwrap_or_default();
            picture.grade = grade;
            c.picture = (!picture.is_usual()).then_some(picture);
        }
    }
    Ok(())
}

pub(super) fn edited(
    p: &mut Project,
    of: &GradeOf,
    edit: impl FnOnce(&mut Grade) -> Result<()>,
) -> Result<()> {
    let mut g = stored(p, of)?;
    let marked = |g: &mut Grade| {
        for d in &mut g.nodes {
            d.animated = d.is_animated();
        }
    };
    marked(&mut g);
    edit(&mut g)?;
    marked(&mut g);
    store(p, of, &g)
}

pub(super) fn no_node(n: u32) -> Error {
    Error::not_found(format!("the grade has no node {n}"))
}

pub(super) fn node(g: &mut Grade, n: u32) -> Result<&mut GradeNode> {
    g.node_mut(n).ok_or_else(|| no_node(n))
}

pub(super) fn locked(n: u32) -> Error {
    Error::invalid(format!("node {n} is locked"))
}

pub(super) fn unlocked(g: &mut Grade, n: u32) -> Result<&mut GradeNode> {
    let d = node(g, n)?;
    match d.locked {
        true => Err(locked(n)),
        false => Ok(d),
    }
}

fn changed_node(g: &mut Grade, n: u32, frame: Option<i64>) -> Result<(&mut GradeNode, i64)> {
    let d = unlocked(g, n)?;
    if d.tool.is_mixer() {
        return Err(Error::invalid(format!("node {n} is a mixer")));
    }
    let frame = match frame {
        Some(f) => f,
        None if d.keys.is_empty() => i64::MIN,
        None => {
            return Err(Error::usage(format!(
                "node {n} has keyframes: give `at` for the keyframe"
            )));
        }
    };
    Ok((d, frame))
}

fn changed(
    g: &mut Grade,
    n: u32,
    frame: Option<i64>,
    change: impl FnOnce(&Corrector) -> Corrector,
) -> Result<()> {
    let (d, frame) = changed_node(g, n, frame)?;
    d.change_at(frame, change).ok_or_else(|| no_node(n))
}

fn key_changed(
    g: &mut Grade,
    n: u32,
    frame: Option<i64>,
    change: impl FnOnce(KeyOutput) -> KeyOutput,
) -> Result<()> {
    if g.node(n).is_some_and(GradeNode::takes_keys) {
        let d = unlocked(g, n)?;
        d.key = change(d.key);
        return Ok(());
    }
    let (d, frame) = changed_node(g, n, frame)?;
    d.change_key_output_at(frame, change)
        .ok_or_else(|| no_node(n))
}

fn key_input_changed(
    g: &mut Grade,
    n: u32,
    change: impl FnOnce(KeyInput) -> KeyInput,
) -> Result<()> {
    let d = unlocked(g, n)?;
    values(d, n)?;
    d.key_input = change(d.key_input);
    Ok(())
}

fn mix_changed(g: &mut Grade, n: u32, change: &GradeNodeChange) -> Result<()> {
    let d = unlocked(g, n)?;
    let GradeTool::KeyMixer { mix } = &mut d.tool else {
        return Err(Error::invalid(format!("node {n} is not a key mixer")));
    };
    let i =
        (change.input).ok_or_else(|| Error::usage("give `input`, an input of the key mixer"))?;
    let m = (mix.get_mut(i as usize))
        .ok_or_else(|| Error::not_found(format!("key mixer {n} has no input {i}")))?;
    *m = change.mix_changed(*m);
    Ok(())
}

const NEEDS: &str = "grade.set needs a value to change";

pub fn set(
    p: &mut Project,
    of: &[GradeOf],
    n: u32,
    (change, node_change): (&GradeChange, &GradeNodeChange),
    at: Option<i64>,
) -> Result<()> {
    if change.is_empty() && node_change.is_empty() {
        return Err(Error::usage(NEEDS));
    }
    for one in of {
        let frame = at.map(|t| frame_of(p, one, t)).transpose()?;
        edited(p, one, |g| {
            if !change.is_empty() {
                changed(g, n, frame, |c| change.changed(c))?;
            }
            if node_change.changes_key() {
                key_changed(g, n, frame, |k| node_change.key_changed(k))?;
            }
            if node_change.changes_key_input() {
                key_input_changed(g, n, |i| node_change.key_input_changed(i))?;
            }
            match (node_change.changes_mix(), node_change.input) {
                (true, _) => mix_changed(g, n, node_change)?,
                (false, Some(_)) => return Err(Error::usage("`input` needs a `mix_` value")),
                (false, None) => {}
            }
            node_changed(g, n, node_change)
        })?;
    }
    Ok(())
}

fn node_changed(g: &mut Grade, n: u32, change: &GradeNodeChange) -> Result<()> {
    let d = match change.mixer.is_some() || change.mode.is_some() {
        true => unlocked(g, n)?,
        false => node(g, n)?,
    };
    if let Some(locked) = change.locked {
        d.locked = locked;
    }
    if let Some(at) = change.place {
        d.place = at;
    }
    if let Some(on) = change.on {
        d.on = on;
    }
    if let Some(label) = &change.label {
        d.label = label.clone();
    }
    if let Some(to) = change.mixer {
        d.tool = match (&d.tool, to) {
            (GradeTool::Corrector(_), _) => {
                return Err(Error::invalid(format!("node {n} is a corrector")));
            }
            (GradeTool::KeyMixer { .. }, _) => {
                return Err(Error::invalid(format!("node {n} is a key mixer")));
            }
            (GradeTool::LayerMixer { .. }, GradeMixer::LayerMixer) => d.tool.clone(),
            (_, GradeMixer::LayerMixer) => GradeTool::LayerMixer {
                mode: Default::default(),
            },
            (_, GradeMixer::ParallelMixer) => GradeTool::ParallelMixer,
        };
    }
    if let Some(mode) = change.mode {
        match &mut d.tool {
            GradeTool::LayerMixer { mode: m } => *m = mode,
            _ => {
                return Err(Error::invalid(format!(
                    "node {n} is not a layer mixer, which takes a mode"
                )));
            }
        }
    }
    Ok(())
}

pub fn clear(
    p: &mut Project,
    of: &[GradeOf],
    (n, keep_nodes, palette): (Option<u32>, bool, Option<GradePalette>),
    at: Option<i64>,
) -> Result<()> {
    if palette.is_some() && n.is_none() {
        return Err(Error::usage("`palette` needs `node`"));
    }
    let size = p.frame_size();
    for one in of {
        let frame = at.map(|t| frame_of(p, one, t)).transpose()?;
        edited(p, one, |g| match (n, keep_nodes) {
            (Some(n), _) => cleared(g, n, frame, (palette, size)),
            (None, true) => {
                let graded: Vec<u32> = (g.nodes.iter().zip(1..))
                    .filter(|(d, _)| (!d.tool.is_mixer() || d.takes_keys()) && !d.locked)
                    .map(|(_, k)| k)
                    .collect();
                for k in graded {
                    cleared(g, k, frame, (None, size))?;
                }
                Ok(())
            }
            (None, false) => {
                *g = Grade::usual(size);
                Ok(())
            }
        })?;
    }
    Ok(())
}

fn cleared(
    g: &mut Grade,
    n: u32,
    frame: Option<i64>,
    (palette, size): (Option<GradePalette>, [f64; 2]),
) -> Result<()> {
    let resets = |one: GradePalette| palette.is_none_or(|p| p == one);
    if g.node(n).is_some_and(GradeNode::takes_keys) && resets(GradePalette::Key) {
        let d = unlocked(g, n)?;
        if let GradeTool::KeyMixer { mix } = &mut d.tool {
            mix.fill(KeyMix::default());
        }
        d.key = KeyOutput::default();
        return Ok(());
    }
    if resets(GradePalette::Primaries) {
        changed(g, n, frame, |_| Corrector::default())?;
    }
    if resets(GradePalette::Key) {
        key_changed(g, n, frame, |_| KeyOutput::default())?;
        let kept = palette.is_some();
        key_input_changed(g, n, |i| KeyInput {
            invert: kept && i.invert,
            ..KeyInput::default()
        })?;
    }
    if resets(GradePalette::Window) {
        let d = unlocked(g, n)?;
        if d.tool.is_mixer() {
            return Err(Error::invalid(format!("node {n} is a mixer")));
        }
        for w in &mut d.windows {
            *w = GradeWindow {
                on: false,
                name: std::mem::take(&mut w.name),
                ..GradeWindow::new(w.shape.kind(), size)
            };
        }
    }
    Ok(())
}

pub fn printer(
    p: &mut Project,
    of: &[GradeOf],
    (n, at): (u32, Option<i64>),
    point: (PrinterColor, f64),
    lights: &ColorPrefs,
) -> Result<()> {
    if !point.1.is_finite() {
        return Err(Error::invalid("points must be a number"));
    }
    for one in of {
        let frame = at.map(|t| frame_of(p, one, t)).transpose()?;
        edited(p, one, |g| {
            changed(g, n, frame, |c| Corrector {
                offset: printed(c.offset, point, lights),
                ..c.clone()
            })
        })?;
    }
    Ok(())
}

pub fn get(p: &Project, of: &GradeOf, at: Option<i64>) -> Result<Value> {
    let g = stored(p, of)?;
    let g = match at {
        Some(t) => g.at(frame_of(p, of, t)? as f64).into_owned(),
        None => g,
    };
    serde_json::to_value(g).map_err(|e| Error::invalid(e.to_string()))
}

fn takers(g: &Grade, n: u32) -> Vec<(u32, usize)> {
    g.nodes
        .iter()
        .zip(1..)
        .filter(|(d, _)| !d.takes_keys())
        .flat_map(|(d, k)| {
            d.inputs
                .iter()
                .enumerate()
                .filter(move |(_, v)| **v == Some(n))
                .map(move |(i, _)| (k, i))
        })
        .collect()
}

fn retake(g: &mut Grade, from: u32, to: Option<u32>, except: u32) {
    for (d, k) in g.nodes.iter_mut().zip(1..) {
        if k == except || d.takes_keys() {
            continue;
        }
        for v in &mut d.inputs {
            if *v == Some(from) {
                *v = to;
            }
        }
    }
    if g.output == Some(from) {
        g.output = to;
    }
}

fn shifted(g: &mut Grade, from_x: f64) {
    for d in &mut g.nodes {
        if d.place[0] >= from_x {
            d.place[0] += 1.0;
        }
    }
}

fn free_below(g: &Grade, [x, y]: [f64; 2]) -> [f64; 2] {
    let mut below = y + 1.0;
    while g.nodes.iter().any(|d| d.place == [x, below]) {
        below += 1.0;
    }
    [x, below]
}

fn input_of(g: &Grade, n: u32) -> Result<Option<u32>> {
    match g.node(n) {
        Some(d) if !d.tool.is_mixer() => Ok(d.inputs[0]),
        Some(_) => Err(Error::invalid(format!(
            "node {n} is a mixer: give a corrector"
        ))),
        None => Err(no_node(n)),
    }
}

fn mixer_of(kind: GradeAdd) -> GradeTool {
    match kind {
        GradeAdd::Layer => GradeTool::LayerMixer {
            mode: Default::default(),
        },
        _ => GradeTool::ParallelMixer,
    }
}

fn beside(g: &mut Grade, n: u32, kind: GradeAdd, frame: [f64; 2]) -> Result<u32> {
    let input = input_of(g, n)?;
    let at = g.node(n).map_or([0.0; 2], |d| d.place);
    let added = g.last() + 1;
    let takers = takers(g, n);
    let mixes = |m: u32| g.node(m).is_some_and(|d| d.tool.is_mixer());
    let mixer = match (takers.as_slice(), g.output == Some(n)) {
        ([(m, _)], false) => Some(*m).filter(|m| mixes(*m)),
        _ => None,
    };
    match mixer {
        Some(m) => {
            let place = free_below(g, at);
            g.nodes.push(GradeNode::corrector(input, place, frame));
            node(g, m)?.inputs.push(Some(added));
        }
        None => {
            shifted(g, at[0] + 1.0);
            let place = free_below(g, at);
            g.nodes.push(GradeNode::corrector(input, place, frame));
            retake(g, n, Some(added + 1), 0);
            let inputs = vec![Some(n), Some(added)];
            let place = [at[0] + 1.0, at[1]];
            g.nodes
                .push(GradeNode::mixer(mixer_of(kind), inputs, place));
        }
    }
    Ok(added)
}

pub fn add(
    p: &mut Project,
    of: &GradeOf,
    kind: GradeAdd,
    n: Option<u32>,
    place: Option<[f64; 2]>,
) -> Result<Value> {
    let frame = p.frame_size();
    let mut made = 0;
    edited(p, of, |g| {
        let added = g.last() + 1;
        let around = |g: &Grade| -> Result<(u32, [f64; 2])> {
            let n = n.ok_or_else(|| Error::usage("give `node`, the node it goes beside"))?;
            let d = g.node(n).ok_or_else(|| no_node(n))?;
            if d.takes_keys() {
                return Err(Error::invalid(format!(
                    "key mixer {n} gives out no picture"
                )));
            }
            Ok((n, d.place))
        };
        let below = g.nodes.iter().map(|d| d.place[1] + 1.0).fold(0.0, f64::max);
        made = match kind {
            GradeAdd::Serial => {
                let (n, [x, y]) = around(g)?;
                shifted(g, x + 1.0);
                g.nodes
                    .push(GradeNode::corrector(Some(n), [x + 1.0, y], frame));
                retake(g, n, Some(added), added);
                added
            }
            GradeAdd::Before => {
                let (n, [x, y]) = around(g)?;
                let input = node(g, n)?.inputs.first().copied().flatten();
                shifted(g, x);
                g.nodes.push(GradeNode::corrector(input, [x, y], frame));
                if let Some(first) = node(g, n)?.inputs.first_mut() {
                    *first = Some(added);
                }
                added
            }
            GradeAdd::Parallel | GradeAdd::Layer => {
                let (n, _) = around(g)?;
                beside(g, n, kind, frame)?
            }
            GradeAdd::Append => {
                let at = match g.output.and_then(|o| g.node(o)) {
                    Some(d) => [d.place[0] + 1.0, d.place[1]],
                    None => {
                        let right = g.nodes.iter().map(|d| d.place[0] + 1.0).fold(0.0, f64::max);
                        [right, 0.0]
                    }
                };
                g.nodes.push(GradeNode::corrector(g.output, at, frame));
                g.output = Some(added);
                added
            }
            GradeAdd::Corrector => {
                let at = place.unwrap_or([0.0, below]);
                g.nodes.push(GradeNode::corrector(None, at, frame));
                added
            }
            GradeAdd::Outside => {
                let (n, [x, y]) = around(g)?;
                values(node(g, n)?, n)?;
                shifted(g, x + 1.0);
                let mut outside = GradeNode::corrector(Some(n), [x + 1.0, y], frame);
                outside.key_from = Some(n);
                outside.key_input.invert = true;
                g.nodes.push(outside);
                retake(g, n, Some(added), added);
                added
            }
            GradeAdd::KeyMixer => {
                let at = place.unwrap_or([0.0, below]);
                let mix = vec![KeyMix::default(); 2];
                let tool = GradeTool::KeyMixer { mix };
                g.nodes.push(GradeNode::mixer(tool, vec![None; 2], at));
                added
            }
        };
        Ok(())
    })?;
    Ok(json!({ "node": made }))
}

fn renumbered(g: &mut Grade, gone: u32) {
    let after = |v: &mut Option<u32>| {
        if let Some(k) = v
            && *k > gone
        {
            *k -= 1;
        }
    };
    for d in &mut g.nodes {
        d.inputs.iter_mut().for_each(after);
        after(&mut d.key_from);
    }
    after(&mut g.output);
}

pub fn remove(p: &mut Project, of: &GradeOf, n: u32, extract: bool) -> Result<()> {
    edited(p, of, |g| {
        if !extract {
            return taken_out(g, n).map(drop);
        }
        let d = g.node(n).ok_or_else(|| no_node(n))?;
        let input = match &d.tool {
            GradeTool::Corrector(_) => d.inputs[0],
            _ => {
                return Err(Error::invalid(format!(
                    "node {n} is a mixer: only a corrector comes out"
                )));
            }
        };
        retake(g, n, input, n);
        let d = node(g, n)?;
        d.inputs = vec![None];
        d.key_from = None;
        unkeyed(g, n);
        Ok(())
    })
}

pub fn taken_out(g: &mut Grade, n: u32) -> Result<Vec<u32>> {
    let d = g.node(n).ok_or_else(|| no_node(n))?;
    let (input, corrector) = match &d.tool {
        GradeTool::Corrector(_) => (d.inputs[0], true),
        _ => (None, false),
    };
    let mut fed = Vec::new();
    unkeyed(g, n);
    for (d, k) in g.nodes.iter_mut().zip(1..) {
        let mixer = d.tool.is_mixer();
        let to = if mixer { None } else { input };
        for v in d.inputs.iter_mut().filter(|v| **v == Some(n) && k != n) {
            *v = to;
            if mixer && corrector {
                fed.push(k);
            }
        }
    }
    if g.output == Some(n) {
        g.output = input;
    }
    let mut gone = vec![n];
    for m in fed {
        if let Some(v) = lone_input(g, m) {
            retake(g, m, Some(v), m);
            gone.push(m);
        }
    }
    gone.sort_unstable_by(|a, b| b.cmp(a));
    gone.dedup();
    for &k in &gone {
        g.nodes.remove(k as usize - 1);
        renumbered(g, k);
    }
    Ok(gone)
}

fn unkeyed(g: &mut Grade, n: u32) {
    for d in &mut g.nodes {
        if d.key_from == Some(n) {
            d.key_from = None;
        }
        if d.takes_keys() {
            for v in d.inputs.iter_mut().filter(|v| **v == Some(n)) {
                *v = None;
            }
        }
    }
}

fn lone_input(g: &Grade, m: u32) -> Option<u32> {
    let mut linked = g.node(m)?.inputs.iter().flatten();
    match (linked.next(), linked.next()) {
        (Some(&v), None) => Some(v),
        _ => None,
    }
}

fn start_of(p: &Project, of: &GradeOf) -> Result<i64> {
    match of {
        GradeOf::Timeline => Ok(0),
        GradeOf::Clip(id) => {
            let (ti, ci) = locate(p, id)?;
            let c = &p.tracks[ti].clips[ci];
            Ok(c.content_at(c.start))
        }
    }
}

fn values(d: &GradeNode, n: u32) -> Result<&Corrector> {
    match &d.tool {
        GradeTool::Corrector(c) => Ok(c),
        _ => Err(Error::invalid(format!("node {n} is a mixer"))),
    }
}

fn shifted_keys(keys: &[GradeKey], by: i64) -> Vec<GradeKey> {
    keys.iter()
        .map(|k| GradeKey {
            frame: k.frame + by,
            ..k.clone()
        })
        .collect()
}

pub fn copy(p: &Project, of: &GradeOf, n: u32) -> Result<CopiedNode> {
    let g = stored(p, of)?;
    let d = g.node(n).ok_or_else(|| no_node(n))?;
    Ok(CopiedNode {
        values: values(d, n)?.clone(),
        keys: shifted_keys(&d.keys, -start_of(p, of)?),
        label: d.label.clone(),
        on: d.on,
        animated: d.is_animated(),
        key: d.key,
        key_input: d.key_input,
        windows: d.windows.clone(),
    })
}

pub fn paste(p: &mut Project, of: &[GradeOf], n: u32, copied: &CopiedNode) -> Result<()> {
    for one in of {
        let start = start_of(p, one)?;
        edited(p, one, |g| {
            let d = unlocked(g, n)?;
            values(d, n)?;
            d.tool = GradeTool::Corrector(Box::new(copied.values.clone()));
            d.keys = shifted_keys(&copied.keys, start);
            d.label = copied.label.clone();
            d.on = copied.on;
            d.animated = copied.animated;
            d.key = copied.key;
            d.key_input = copied.key_input;
            d.windows.clone_from(&copied.windows);
            Ok(())
        })?;
    }
    Ok(())
}

fn spread(g: &Grade, k: u32, spans: &mut [Option<f64>]) -> f64 {
    if let Some(s) = spans[k as usize] {
        return s;
    }
    let Some(d) = g.node(k) else {
        return 1.0;
    };
    let mut each = d.inputs.iter().map(|v| match v {
        Some(v) if *v > 0 => spread(g, *v, spans),
        _ => 1.0,
    });
    let s = match d.tool.is_mixer() {
        true => each.sum::<f64>().max(1.0),
        false => each.next().unwrap_or(1.0),
    };
    spans[k as usize] = Some(s);
    s
}

fn filled(g: &mut Grade, [from, to]: [[f64; 2]; 2]) {
    for axis in 0..2 {
        let (low, high) = (g.nodes.iter().map(|d| d.place[axis]))
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), v| {
                (l.min(v), h.max(v))
            });
        let span = high - low;
        let step = match span > 0.0 {
            true => ((to[axis] - from[axis]) / span).max(1.0),
            false => 1.0,
        };
        let middle = (from[axis] + to[axis]) / 2.0;
        for d in &mut g.nodes {
            d.place[axis] = middle + (d.place[axis] - (low + high) / 2.0) * step;
        }
    }
}

pub fn cleanup(p: &mut Project, of: &GradeOf, area: Option<[[f64; 2]; 2]>) -> Result<()> {
    if let Some([from, to]) = area {
        finite(&[from, to].concat(), "area")?;
        if to[0] < from[0] || to[1] < from[1] {
            return Err(Error::invalid(
                "area goes from its top left to its bottom right",
            ));
        }
    }
    edited(p, of, |g| {
        let order = crate::grade::order(g)
            .ok_or_else(|| Error::invalid("the links of the grade make a loop"))?;
        let count = g.nodes.len() + 1;
        let mut depth = vec![0.0; count];
        for &k in &order {
            let ins = node(g, k)?
                .inputs
                .iter()
                .flatten()
                .map(|v| depth[*v as usize]);
            depth[k as usize] = ins.fold(0.0, f64::max) + 1.0;
        }
        let mut spans = vec![None; count];
        let mut wants: Vec<Vec<f64>> = vec![Vec::new(); count];
        if let Some(o) = g.output.filter(|o| *o > 0) {
            wants[o as usize].push(0.0);
        }
        let mut rows = vec![None; count];
        for &k in order.iter().rev() {
            let w = &wants[k as usize];
            if w.is_empty() {
                continue;
            }
            let row = w.iter().sum::<f64>() / w.len() as f64;
            rows[k as usize] = Some(row);
            let inputs = node(g, k)?.inputs.clone();
            let mixer = node(g, k)?.tool.is_mixer();
            let sizes: Vec<f64> = (inputs.iter())
                .map(|v| match v {
                    Some(v) if *v > 0 && mixer => spread(g, *v, &mut spans),
                    _ => 1.0,
                })
                .collect();
            let mut at = row - sizes.iter().sum::<f64>() / 2.0;
            for (v, size) in inputs.iter().zip(&sizes) {
                if let Some(v) = v.filter(|v| *v > 0) {
                    wants[v as usize].push(if mixer { at + size / 2.0 } else { row });
                }
                at += size;
            }
        }
        let below = rows
            .iter()
            .flatten()
            .fold(0.0, |a: f64, b| a.max(*b))
            .floor()
            + 1.0;
        let mut loose = 0.0;
        for (d, k) in g.nodes.iter_mut().zip(1..) {
            d.place = match rows[k] {
                Some(row) => [depth[k] - 1.0, row],
                None => {
                    loose += 1.0;
                    [loose - 1.0, below]
                }
            };
        }
        if let Some(area) = area {
            filled(g, area);
        }
        Ok(())
    })
}

pub fn swap(p: &mut Project, of: &GradeOf, n: u32, with: u32) -> Result<()> {
    edited(p, of, |g| {
        let a = unlocked(g, n)?.clone();
        let b = unlocked(g, with)?.clone();
        values(&a, n)?;
        values(&b, with)?;
        for (k, from) in [(n, b), (with, a)] {
            let d = node(g, k)?;
            d.tool = from.tool;
            d.keys = from.keys;
            d.label = from.label;
            d.on = from.on;
            d.animated = from.animated;
            d.key = from.key;
            d.key_input = from.key_input;
            d.windows = from.windows;
        }
        Ok(())
    })
}

fn linked(
    inputs: &mut Vec<Option<u32>>,
    (n, input): (u32, Option<u32>),
    from: Option<u32>,
) -> Result<bool> {
    let count = inputs.len();
    match input.map(|i| i as usize) {
        Some(i) if i < count => {
            inputs[i] = from;
            Ok(false)
        }
        None => {
            inputs.push(from);
            Ok(true)
        }
        Some(i) if i == count => {
            inputs.push(from);
            Ok(true)
        }
        Some(i) => Err(Error::invalid(format!(
            "mixer {n} has {count} inputs: input {i} is past the next one"
        ))),
    }
}

pub fn connect(
    p: &mut Project,
    of: &GradeOf,
    (n, input): (u32, Option<u32>),
    from: Option<u32>,
    key: bool,
) -> Result<()> {
    edited(p, of, |g| {
        if let Some(f) = from.filter(|f| *f > g.last()) {
            return Err(no_node(f));
        }
        if n == 0 {
            if key {
                return Err(Error::invalid("the output takes a picture, not a key"));
            }
            if input.is_some_and(|i| i > 0) {
                return Err(Error::invalid("the output has one input"));
            }
            g.output = from;
            return Ok(());
        }
        if from == Some(n) {
            return Err(Error::invalid(format!("node {n} cannot take itself")));
        }
        let d = node(g, n)?;
        match (&mut d.tool, key) {
            (GradeTool::KeyMixer { mix }, true) => {
                if linked(&mut d.inputs, (n, input), from)? {
                    mix.push(KeyMix::default());
                }
            }
            (GradeTool::KeyMixer { .. }, false) => {
                return Err(Error::invalid(format!(
                    "key mixer {n} takes keys: give `key`"
                )));
            }
            (GradeTool::Corrector(_), true) => match input {
                None | Some(0) => d.key_from = from,
                Some(_) => {
                    return Err(Error::invalid(format!("corrector {n} has one key input")));
                }
            },
            (_, true) => {
                return Err(Error::invalid(format!("mixer {n} takes no key")));
            }
            (GradeTool::Corrector(_), false) => match input {
                None | Some(0) => d.inputs[0] = from,
                Some(_) => {
                    return Err(Error::invalid(format!("corrector {n} has one input")));
                }
            },
            (_, false) => {
                linked(&mut d.inputs, (n, input), from)?;
            }
        }
        if crate::grade::order(g).is_none() {
            return Err(Error::invalid("that link makes a loop"));
        }
        Ok(())
    })
}
