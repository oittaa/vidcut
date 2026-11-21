use crate::model::{
    Clip, Cue, Effects, Generator, Marker, Marks, Project, Track, TrackClip, TrackCue, Transition,
};
use crate::time::{Rational, rescale};

#[derive(Clone, Copy)]
pub struct Rates {
    pub from: Rational,
    pub to: Rational,
}

impl Rates {
    pub fn new(from: Rational, to: Rational) -> Option<Self> {
        (from != to).then_some(Self { from, to })
    }

    pub fn frame(self, f: i64) -> i64 {
        rescale(f, self.from, self.to)
    }

    fn ratio(self) -> f64 {
        self.to.as_f64() / self.from.as_f64()
    }
}

pub fn clips(p: &Project, list: &mut [TrackClip], r: Rates) {
    for c in list {
        clip(p, &mut c.clip, r);
    }
}

pub fn track(p: &Project, t: &mut Track, r: Rates) {
    for c in &mut t.clips {
        clip(p, c, r);
    }
    t.clips.retain(|c| c.duration > 0);
    t.transitions.retain(|x| {
        [&x.from, &x.to]
            .into_iter()
            .flatten()
            .all(|id| t.clips.iter().any(|c| &c.id == id))
    });
    for x in &mut t.transitions {
        transition(x, r);
    }
    t.transitions.retain(|x| x.before > 0 || x.after > 0);
}

pub fn cues(list: &mut [TrackCue], r: Rates) {
    for c in list.iter_mut().map(|c| &mut c.cue) {
        cue(c, r);
    }
}

pub fn cue(c: &mut Cue, r: Rates) {
    let (start, end) = (r.frame(c.start), r.frame(c.start + c.duration));
    (c.start, c.duration) = (start, (end - start).max(1));
    for w in &mut c.words {
        w.at = w.at.map(|a| r.frame(a));
    }
    if let Some(picture) = c.picture.as_mut() {
        picture.scale_times(&|f| r.frame(f), &|_| r.ratio());
    }
}

pub fn marks(m: &mut Marks, r: Rates) {
    m.mark_in = m.mark_in.map(|f| r.frame(f));
    m.mark_out = m.mark_out.map(|f| r.frame(f));
}

pub fn marker(k: &mut Marker, r: Rates) {
    (k.at, k.duration) = (r.frame(k.at), r.frame(k.duration).max(1));
}

fn transition(x: &mut Transition, r: Rates) {
    (x.before, x.after) = (r.frame(x.before), r.frame(x.after));
    crate::otio::transitions::scale_curve(x.otio.make_mut(), |f| r.frame(f), r.ratio());
}

fn effects(e: &mut Effects, r: Rates) {
    let ratio = r.ratio();
    e.from = r.frame(e.from);
    e.retime(|x| (x * ratio).round());
    e.scale_handles(ratio);
}

pub fn clip(p: &Project, c: &mut Clip, r: Rates) {
    let ratio = r.ratio();
    let conv = |f: i64| r.frame(f);
    let (was_start, was_end) = (c.start, c.end());
    let (start, end) = (conv(was_start), conv(was_end));
    if let Some(a) = c.audio.as_mut() {
        (a.head, a.tail) = (
            conv(was_start + a.head) - start,
            conv(was_end + a.tail) - end,
        );
        a.gain_keys = crate::keyframes::scaled(&a.gain_keys, conv, ratio);
        a.pan_keys = crate::keyframes::scaled(&a.pan_keys, conv, ratio);
        if let Some(e) = a.effects.as_mut() {
            effects(e, r);
        }
    }
    let exact = (c.source_in as f64 + c.phase) * ratio;
    let per = (p.media(&c.media))
        .map(|m| r.to.over(m.rate(p.settings.fps)))
        .filter(|per| per.den() != 1);
    match per {
        Some(per) => {
            let units = (exact * per.den() as f64).round() as i64;
            c.source_in = units.div_euclid(per.den());
            c.phase = units.rem_euclid(per.den()) as f64 / per.den() as f64;
        }
        None => (c.source_in, c.phase) = (exact.round() as i64, 0.0),
    }
    if let Some(e) = c.effects.as_mut() {
        effects(e, r);
    }
    if let Some(picture) = c.picture.as_mut() {
        picture.scale_times(&conv, &|_| ratio);
    }
    if let Some(title) = c.title.as_mut().filter(|t| t.is_keyed()) {
        title.each_line(|line| {
            let mut picture = line.as_picture();
            picture.scale_times(&conv, &|_| ratio);
            line.move_handles(conv);
            line.take_picture(&picture);
        });
    }
    if let Some(Generator::FourColorGradient(g)) = c.generator.as_mut() {
        for axis in [crate::generators::Axis::X, crate::generators::Axis::Y] {
            let keys = g.keys_mut(axis);
            *keys = crate::keyframes::scaled(keys, conv, ratio);
        }
    }
    if let Some(s) = c.speed.as_mut() {
        s.skip = conv(s.skip);
        let sections: Vec<_> = (s.sections.iter())
            .map(|x| crate::model::Section {
                at: conv(x.at),
                from: x.from * ratio,
                ease: x.ease.map(|[a, b]| [a * ratio, b * ratio]),
                ..*x
            })
            .collect();
        s.sections = sections.into();
    }
    (c.start, c.duration) = (start, end - start);
    if let Some(k) = p.clip_compound(c) {
        fit_compound(c, rescale(k.frames, k.fps, r.to));
    }
}

fn fit_compound(c: &mut Clip, limit: i64) {
    if c.duration <= 0 {
        return;
    }
    if limit <= 0 {
        c.duration = 0;
        return;
    }
    loop {
        let (lo, hi) = c.shown();
        if lo < 0 {
            c.source_in -= lo;
            continue;
        }
        if hi <= limit {
            break;
        }
        let shift = (hi - limit).min(lo);
        if shift > 0 {
            c.source_in -= shift;
            continue;
        }
        if c.speed.is_none() {
            c.duration = (limit - lo).max(0);
            break;
        }
        c.duration -= 1;
        if c.duration <= 0 {
            break;
        }
    }
}
