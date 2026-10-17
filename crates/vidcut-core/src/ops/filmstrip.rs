use std::ops::RangeInclusive;

use crate::model::{Clip, Project};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StripFrame {
    pub number: i64,
    pub source: Option<i64>,
    pub inside: bool,
}

pub fn strip(
    p: &Project,
    c: &Clip,
    outgoing: bool,
    numbers: RangeInclusive<i64>,
) -> Vec<StripFrame> {
    let have = p.source_range(c);
    let zero = if outgoing { c.end() - 1 } else { c.start };
    numbers
        .map(|number| {
            let t = zero + number;
            let source = c.source_at(t);
            StripFrame {
                number,
                source: have
                    .as_ref()
                    .is_some_and(|h| h.contains(&source))
                    .then_some(source),
                inside: (c.start..c.end()).contains(&t),
            }
        })
        .collect()
}

fn room(p: &Project, c: &Clip, tail: bool) -> i64 {
    p.source_range(c)
        .map_or(i64::MAX / 4, |have| c.room(tail, have))
}

pub fn out_reach(p: &Project, out: &Clip) -> (i64, i64) {
    (1 - out.duration, room(p, out, true))
}

pub fn in_reach(p: &Project, inc: &Clip) -> (i64, i64) {
    (-room(p, inc, false), inc.duration - 1)
}

pub fn roll_reach(p: &Project, out: &Clip, inc: &Clip) -> (i64, i64) {
    let (out_lo, out_hi) = out_reach(p, out);
    let (in_lo, in_hi) = in_reach(p, inc);
    (out_lo.max(in_lo), out_hi.min(in_hi))
}

#[cfg(test)]
mod tests {
    use crate::test_support::Fixture;

    use super::*;

    fn two_clips() -> Fixture {
        let mut f = Fixture::new(&[(10.0, true), (4.0, true)]);
        f.append("m1", 0, 50);
        f.append("m2", 10, 60);
        f
    }

    fn clip<'a>(f: &'a Fixture, id: &str) -> &'a Clip {
        f.editor.project().clip(id).unwrap()
    }

    #[test]
    fn an_outgoing_strip_numbers_its_last_frame_zero_and_dims_the_frames_past_it() {
        let f = two_clips();
        let numbers: Vec<(i64, Option<i64>, bool)> =
            strip(f.editor.project(), clip(&f, "c1"), true, -1..=1)
                .into_iter()
                .map(|s| (s.number, s.source, s.inside))
                .collect();
        assert_eq!(
            numbers,
            [
                (-1, Some(48), true),
                (0, Some(49), true),
                (1, Some(50), false)
            ]
        );
    }

    #[test]
    fn an_incoming_strip_has_no_frame_before_its_media() {
        let mut f = Fixture::new(&[(10.0, true)]);
        f.append("m1", 0, 50);
        let numbers: Vec<(i64, Option<i64>, bool)> =
            strip(f.editor.project(), clip(&f, "c1"), false, -1..=0)
                .into_iter()
                .map(|s| (s.number, s.source, s.inside))
                .collect();
        assert_eq!(numbers, [(-1, None, false), (0, Some(0), true)]);
    }

    #[test]
    fn a_roll_reaches_as_far_as_both_clips_media_allow() {
        let f = two_clips();
        let p = f.editor.project();
        assert_eq!(roll_reach(p, clip(&f, "c1"), clip(&f, "c2")), (-10, 49));
    }
}
