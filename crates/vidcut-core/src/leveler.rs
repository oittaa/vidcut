use std::collections::VecDeque;
use std::ops::RangeInclusive;

use crate::error::{Error, Result};
use crate::model::{DialogueLeveler, LevelerPreset};
use crate::request::DialogueLevelerSet;

pub const OUTPUT_DB: RangeInclusive<f64> = 0.0..=6.0;
const SWITCHED_ON_GAIN_DB: f64 = 2.0;
pub const PRESETS: [LevelerPreset; 4] = [
    LevelerPreset::AllowWiderDynamics,
    LevelerPreset::OptimizeModerateLevels,
    LevelerPreset::MoreLiftForLowLevels,
    LevelerPreset::LiftSoftWhisperySources,
];
const HOPS_PER_SECOND: u32 = 100;
const WINDOW_HOPS: usize = 5;
const AHEAD_HOPS: usize = 20;
const GATE_DB: f64 = -70.0;
const OVER_FLOOR_DB: f64 = 10.0;
const FLOOR_RISE_DB: f64 = 6.0 / HOPS_PER_SECOND as f64;
const REFERENCE_FALL_DB: f64 = 6.0 / HOPS_PER_SECOND as f64;
const SPEECH_HOLD_HOPS: u32 = 40;
const LOUD_WITHIN_DB: f64 = 2.0;
const LIFT_DEAD_DB: f64 = 3.0;
const CUT_FALL_DB: f64 = 20.0 / HOPS_PER_SECOND as f64;
const CUT_RELEASE: f64 = 1.0 / 60.0;
const LIFT_SMOOTHING: f64 = 1.0 / 5.0;
const QUIET_SMOOTHING: f64 = 1.0 / 20.0;

pub fn label(preset: LevelerPreset) -> &'static str {
    match preset {
        LevelerPreset::AllowWiderDynamics => "Allow Wider Dynamics",
        LevelerPreset::OptimizeModerateLevels => "Optimize Moderate Levels",
        LevelerPreset::MoreLiftForLowLevels => "More Lift For Low Levels",
        LevelerPreset::LiftSoftWhisperySources => "Lift Soft Whispery Sources",
    }
}

pub fn first() -> DialogueLeveler {
    DialogueLeveler {
        on: true,
        preset: LevelerPreset::AllowWiderDynamics,
        reduce_loud: false,
        lift_soft: false,
        background_reduction: false,
        output_gain_db: 0.0,
    }
}

fn switched_on() -> DialogueLeveler {
    DialogueLeveler {
        reduce_loud: true,
        lift_soft: true,
        background_reduction: true,
        output_gain_db: SWITCHED_ON_GAIN_DB,
        ..first()
    }
}

fn out_of_range(v: f64) -> Result<()> {
    if OUTPUT_DB.contains(&v) {
        return Ok(());
    }
    Err(Error::invalid(format!(
        "dialogue leveler output gain {v} is outside {}..{} dB",
        OUTPUT_DB.start(),
        OUTPUT_DB.end()
    )))
}

pub fn check(set: &DialogueLevelerSet) -> Result<()> {
    set.output_gain_db.map_or(Ok(()), out_of_range)
}

pub fn patched(old: Option<DialogueLeveler>, set: &DialogueLevelerSet) -> Result<DialogueLeveler> {
    check(set)?;
    let mut l = old.unwrap_or_else(|| match set.on {
        Some(true) => switched_on(),
        _ => first(),
    });
    l.on = set.on.unwrap_or(l.on);
    l.preset = set.preset.unwrap_or(l.preset);
    l.reduce_loud = set.reduce_loud.unwrap_or(l.reduce_loud);
    l.lift_soft = set.lift_soft.unwrap_or(l.lift_soft);
    l.background_reduction = set.background_reduction.unwrap_or(l.background_reduction);
    l.output_gain_db = set.output_gain_db.unwrap_or(l.output_gain_db);
    Ok(l)
}

pub fn problem(l: &DialogueLeveler) -> Option<String> {
    out_of_range(l.output_gain_db).err().map(|e| e.message)
}

impl DialogueLeveler {
    pub fn heard(&self) -> bool {
        self.on && (self.reduce_loud || self.lift_soft || self.background_reduction)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Reach {
    cut_db: f64,
    lift_db: f64,
    lift_share: f64,
    quiet_db: f64,
}

fn reach(preset: LevelerPreset) -> Reach {
    let (cut_db, lift_db, lift_share, quiet_db) = match preset {
        LevelerPreset::AllowWiderDynamics => (3.0, 4.3, 0.6, 5.0),
        LevelerPreset::OptimizeModerateLevels => (10.0, 4.3, 0.6, 1.2),
        LevelerPreset::MoreLiftForLowLevels => (6.0, 9.5, 1.0, 5.0),
        LevelerPreset::LiftSoftWhisperySources => (6.0, 10.2, 1.0, 4.0),
    };
    Reach {
        cut_db,
        lift_db,
        lift_share,
        quiet_db,
    }
}

#[derive(Debug, Clone)]
pub struct Leveler {
    settings: DialogueLeveler,
    reach: Reach,
    channels: usize,
    hop: usize,
    energy: [f64; WINDOW_HOPS],
    slot: usize,
    floor: f64,
    reference: Option<f64>,
    held: u32,
    loud_held: u32,
    shorts: VecDeque<f64>,
    cut: f64,
    lift: f64,
    quiet: f64,
    last_gain: f32,
    delayed: VecDeque<f32>,
    pulled: Vec<f32>,
    ready: VecDeque<f32>,
}

fn to_gain(db: f64) -> f32 {
    10f64.powf(db / 20.0) as f32
}

impl Leveler {
    pub fn new(settings: DialogueLeveler, rate: u32, channels: u16) -> Self {
        Self {
            settings,
            reach: reach(settings.preset),
            channels: usize::from(channels).max(1),
            hop: (rate / HOPS_PER_SECOND).max(1) as usize,
            energy: [0.0; WINDOW_HOPS],
            slot: 0,
            floor: GATE_DB,
            reference: None,
            held: 0,
            loud_held: 0,
            shorts: VecDeque::new(),
            cut: 0.0,
            lift: 0.0,
            quiet: 0.0,
            last_gain: 1.0,
            delayed: VecDeque::new(),
            pulled: Vec::new(),
            ready: VecDeque::new(),
        }
    }

    pub fn settings(&self) -> DialogueLeveler {
        self.settings
    }

    fn level(&mut self) -> f64 {
        let frames = (self.pulled.len() / self.channels).max(1);
        let sum: f64 = self
            .pulled
            .chunks_exact(self.channels)
            .map(|frame| {
                let mid = frame.iter().map(|v| f64::from(*v)).sum::<f64>() / frame.len() as f64;
                mid * mid
            })
            .sum();
        self.energy[self.slot] = sum / frames as f64;
        self.slot = (self.slot + 1) % WINDOW_HOPS;
        let mean = self.energy.iter().sum::<f64>() / WINDOW_HOPS as f64;
        10.0 * mean.max(1e-12).log10()
    }

    fn step(&mut self) -> f32 {
        let short = self.level();
        self.floor = short.min(self.floor + FLOOR_RISE_DB);
        if short > (self.floor + OVER_FLOOR_DB).max(GATE_DB) {
            self.held = SPEECH_HOLD_HOPS;
        } else {
            self.held = self.held.saturating_sub(1);
        }
        let speech = self.held > 0;
        self.shorts.push_back(short);
        let due = match self.shorts.len() > AHEAD_HOPS {
            true => self.shorts.pop_front().unwrap_or(short),
            false => short,
        };
        let reference = self
            .reference
            .map_or(due, |r| due.max(r - REFERENCE_FALL_DB));
        self.reference = Some(reference);
        if speech && reference - short < LOUD_WITHIN_DB {
            self.loud_held = SPEECH_HOLD_HOPS;
        } else {
            self.loud_held = self.loud_held.saturating_sub(1);
        }
        let reach = self.reach;
        let want_cut = if self.loud_held > 0 && self.settings.reduce_loud {
            reach.cut_db
        } else {
            0.0
        };
        self.cut = if want_cut > self.cut {
            want_cut.min(self.cut + CUT_FALL_DB)
        } else {
            self.cut + (want_cut - self.cut) * CUT_RELEASE
        };
        let want_lift = if speech && self.settings.lift_soft {
            ((reference - due - LIFT_DEAD_DB) * reach.lift_share).clamp(0.0, reach.lift_db)
        } else {
            0.0
        };
        self.lift += (want_lift - self.lift) * LIFT_SMOOTHING;
        let want_quiet = if !speech && self.settings.background_reduction {
            reach.quiet_db
        } else {
            0.0
        };
        self.quiet += (want_quiet - self.quiet) * QUIET_SMOOTHING;
        let output = if self.settings.reduce_loud {
            self.settings.output_gain_db
        } else {
            0.0
        };
        to_gain(output - self.cut + self.lift - self.quiet)
    }

    pub fn read<E>(
        &mut self,
        n: usize,
        out: &mut Vec<f32>,
        mut pull: impl FnMut(usize, &mut Vec<f32>) -> std::result::Result<(), E>,
    ) -> std::result::Result<(), E> {
        let ch = self.channels;
        while self.ready.len() < n * ch {
            self.pulled.clear();
            pull(self.hop, &mut self.pulled)?;
            self.pulled.resize(self.hop * ch, 0.0);
            let gain = self.step();
            self.delayed.extend(self.pulled.iter().copied());
            if self.delayed.len() > AHEAD_HOPS * self.hop * ch {
                let from = self.last_gain;
                for frame in 0..self.hop {
                    let g = from + (gain - from) * (frame + 1) as f32 / self.hop as f32;
                    for _ in 0..ch {
                        let sample = self.delayed.pop_front().unwrap_or(0.0);
                        self.ready.push_back(sample * g);
                    }
                }
                self.last_gain = gain;
            }
        }
        out.extend(self.ready.drain(..n * ch));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48000;

    fn set() -> DialogueLevelerSet {
        DialogueLevelerSet::default()
    }

    fn on(change: impl FnOnce(&mut DialogueLeveler)) -> DialogueLeveler {
        let mut l = first();
        change(&mut l);
        l
    }

    fn tone(from: usize, count: usize, loud: usize, quiet: f32) -> Vec<f32> {
        (from..from + count)
            .map(|i| {
                let a = if i < loud { 0.5 } else { quiet };
                a * (i as f32 * 0.13).sin()
            })
            .collect()
    }

    fn run(settings: DialogueLeveler, seconds: f64, loud: usize, quiet: f32) -> Vec<f32> {
        let frames = (seconds * f64::from(RATE)) as usize;
        let mut l = Leveler::new(settings, RATE, 1);
        let mut at = 0;
        let mut out = Vec::new();
        let source = |n: usize, o: &mut Vec<f32>| -> std::result::Result<(), ()> {
            o.extend(tone(at, n, loud, quiet));
            at += n;
            Ok(())
        };
        l.read(frames, &mut out, source).unwrap();
        out
    }

    fn rms_db(x: &[f32]) -> f64 {
        let p = x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len() as f64;
        10.0 * p.log10()
    }

    fn tenths(v: f64) -> f64 {
        (v * 10.0).round() / 10.0
    }

    fn steady_change(settings: DialogueLeveler) -> f64 {
        let out = run(settings, 2.0, usize::MAX, 0.5);
        let last = &out[out.len() - 9600..];
        tenths(rms_db(last) - 20.0 * 0.5f64.log10() + 10.0 * 2f64.log10())
    }

    #[test]
    fn a_leveler_with_no_switch_on_is_not_heard() {
        assert!(!first().heard());
    }

    #[test]
    fn a_leveler_that_is_off_is_not_heard() {
        assert!(
            !on(|l| {
                l.on = false;
                l.reduce_loud = true;
            })
            .heard()
        );
    }

    #[test]
    fn reduce_loud_is_heard() {
        assert!(on(|l| l.reduce_loud = true).heard());
    }

    #[test]
    fn an_output_gain_past_six_db_is_refused() {
        let mut s = set();
        s.output_gain_db = Some(6.5);
        assert!(check(&s).is_err());
    }

    #[test]
    fn a_patch_changes_only_the_fields_given() {
        let mut s = set();
        s.lift_soft = Some(true);
        let l = patched(Some(on(|l| l.reduce_loud = true)), &s).unwrap();
        assert_eq!((l.reduce_loud, l.lift_soft), (true, true));
    }

    #[test]
    fn a_stored_output_gain_outside_the_range_is_a_problem() {
        let l = on(|l| l.output_gain_db = 9.0);
        assert_eq!(
            problem(&l),
            Some("dialogue leveler output gain 9 is outside 0..6 dB".to_string())
        );
    }

    #[test]
    fn reduce_loud_cuts_speech_by_three_db() {
        assert_eq!(steady_change(on(|l| l.reduce_loud = true)), -3.0);
    }

    #[test]
    fn optimize_moderate_levels_cuts_speech_by_ten_db() {
        assert_eq!(
            steady_change(on(|l| {
                l.reduce_loud = true;
                l.preset = LevelerPreset::OptimizeModerateLevels;
            })),
            -10.0
        );
    }

    #[test]
    fn output_gain_adds_to_reduce_loud() {
        assert_eq!(
            steady_change(on(|l| {
                l.reduce_loud = true;
                l.output_gain_db = 6.0;
            })),
            3.0
        );
    }

    #[test]
    fn output_gain_does_nothing_without_reduce_loud() {
        assert_eq!(
            steady_change(on(|l| {
                l.lift_soft = true;
                l.output_gain_db = 6.0;
            })),
            0.0
        );
    }

    #[test]
    fn lift_soft_raises_a_quiet_part_after_a_loud_one() {
        let settings = on(|l| {
            l.lift_soft = true;
            l.preset = LevelerPreset::MoreLiftForLowLevels;
        });
        let out = run(settings, 1.5, 48000, 0.05);
        let part = &out[62400..72000];
        let change = rms_db(part) - 20.0 * 0.05f64.log10() + 10.0 * 2f64.log10();
        assert_eq!(tenths(change), 9.5);
    }

    #[test]
    fn a_leveler_with_no_process_keeps_the_sound_in_time() {
        let out = run(first(), 0.1, usize::MAX, 0.5);
        assert_eq!(out[100], tone(100, 1, usize::MAX, 0.5)[0]);
    }
}
