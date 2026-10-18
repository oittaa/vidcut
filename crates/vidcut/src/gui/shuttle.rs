use eframe::egui::{self, Key};

pub const HOLD: f64 = 0.25;
pub const SLOW: f64 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Jkl {
    Play(f64),
    Step(f64),
    Slow(f64),
    Stop,
}

#[derive(Debug, Default)]
pub struct Shuttle {
    held: Option<Held>,
}

#[derive(Debug, Clone, Copy)]
struct Held {
    dir: f64,
    since: f64,
    slow: bool,
}

impl Shuttle {
    pub fn keys(&mut self, pressed: &[f64], k: bool, j: bool, l: bool, now: f64) -> Vec<Jkl> {
        let mut out = Vec::new();
        for &dir in pressed {
            out.extend(self.let_go());
            match k {
                true => {
                    self.held = Some(Held {
                        dir,
                        since: now,
                        slow: false,
                    })
                }
                false => out.push(Jkl::Play(dir)),
            }
        }
        if let Some(h) = &mut self.held {
            let down = if h.dir > 0.0 { l } else { j };
            if !(k && down) {
                out.extend(self.let_go());
            } else if !h.slow && now - h.since >= HOLD {
                h.slow = true;
                out.push(Jkl::Slow(h.dir * SLOW));
            }
        }
        out
    }

    pub fn input(&mut self, input: &egui::InputState) -> Vec<Jkl> {
        let pressed: Vec<f64> = input
            .events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Key {
                    key: Key::L,
                    pressed: true,
                    repeat: false,
                    ..
                } => Some(1.0),
                egui::Event::Key {
                    key: Key::J,
                    pressed: true,
                    repeat: false,
                    ..
                } => Some(-1.0),
                _ => None,
            })
            .collect();
        self.keys(
            &pressed,
            input.key_down(Key::K),
            input.key_down(Key::J),
            input.key_down(Key::L),
            input.time,
        )
    }

    pub fn due(&self, now: f64) -> Option<f64> {
        self.held
            .filter(|h| !h.slow)
            .map(|h| (h.since + HOLD - now).max(0.0))
    }

    fn let_go(&mut self) -> Option<Jkl> {
        self.held.take().map(|h| match h.slow {
            true => Jkl::Stop,
            false => Jkl::Step(h.dir),
        })
    }
}

pub fn first_press(input: &egui::InputState, key: Key) -> bool {
    input.events.iter().any(|e| {
        matches!(
            e,
            egui::Event::Key { key: k, pressed: true, repeat: false, .. } if *k == key
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(events: Vec<egui::Event>, down: &[Key]) -> egui::InputState {
        let mut input = egui::InputState::default();
        input.events = events;
        input.keys_down = down.iter().copied().collect();
        input
    }

    fn key(key: Key, pressed: bool, repeat: bool) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn a_repeat_of_l_held_alone_is_not_a_press() {
        let input = input(vec![key(Key::L, true, true)], &[Key::L]);
        assert_eq!(Shuttle::default().input(&input), []);
    }

    #[test]
    fn a_tap_of_j_while_k_is_down_steps_a_frame_backward() {
        let input = input(
            vec![key(Key::J, true, false), key(Key::J, false, false)],
            &[Key::K],
        );
        assert_eq!(Shuttle::default().input(&input), [Jkl::Step(-1.0)]);
    }

    #[test]
    fn a_hold_that_could_still_be_a_tap_wants_a_frame_when_it_turns_slow() {
        let mut s = Shuttle::default();
        s.keys(&[1.0], true, false, true, 0.0);
        assert_eq!(s.due(0.125), Some(0.125));
    }

    #[test]
    fn slow_play_wants_no_frame_for_the_hold() {
        let mut s = Shuttle::default();
        s.keys(&[1.0], true, false, true, 0.0);
        s.keys(&[], true, false, true, 0.3);
        assert_eq!(s.due(0.4), None);
    }

    #[test]
    fn no_key_held_wants_no_frame() {
        assert_eq!(Shuttle::default().due(1.0), None);
    }

    #[test]
    fn l_alone_plays_forward() {
        assert_eq!(
            Shuttle::default().keys(&[1.0], false, false, true, 0.0),
            [Jkl::Play(1.0)]
        );
    }

    #[test]
    fn a_tap_of_l_with_k_held_steps_a_frame_forward() {
        assert_eq!(
            Shuttle::default().keys(&[1.0], true, false, false, 0.0),
            [Jkl::Step(1.0)]
        );
    }

    #[test]
    fn l_held_with_k_does_nothing_while_it_could_still_be_a_tap() {
        let mut s = Shuttle::default();
        s.keys(&[1.0], true, false, true, 0.0);
        assert_eq!(s.keys(&[], true, false, true, 0.2), []);
    }

    #[test]
    fn l_let_go_within_a_quarter_second_steps_a_frame_forward() {
        let mut s = Shuttle::default();
        s.keys(&[1.0], true, false, true, 0.0);
        assert_eq!(s.keys(&[], true, false, false, 0.2), [Jkl::Step(1.0)]);
    }

    #[test]
    fn j_held_with_k_for_a_quarter_second_plays_backward_at_half_speed() {
        let mut s = Shuttle::default();
        s.keys(&[-1.0], true, true, false, 0.0);
        assert_eq!(s.keys(&[], true, true, false, 0.25), [Jkl::Slow(-0.5)]);
    }

    #[test]
    fn slow_play_starts_once() {
        let mut s = Shuttle::default();
        s.keys(&[1.0], true, false, true, 0.0);
        s.keys(&[], true, false, true, 0.3);
        assert_eq!(s.keys(&[], true, false, true, 0.6), []);
    }

    #[test]
    fn letting_l_go_stops_slow_play() {
        let mut s = Shuttle::default();
        s.keys(&[1.0], true, false, true, 0.0);
        s.keys(&[], true, false, true, 0.3);
        assert_eq!(s.keys(&[], true, false, false, 0.9), [Jkl::Stop]);
    }

    #[test]
    fn letting_k_go_stops_slow_play() {
        let mut s = Shuttle::default();
        s.keys(&[-1.0], true, true, false, 0.0);
        s.keys(&[], true, true, false, 0.3);
        assert_eq!(s.keys(&[], false, true, false, 0.9), [Jkl::Stop]);
    }

    #[test]
    fn letting_k_go_and_pressing_l_again_stops_slow_play_then_plays() {
        let mut s = Shuttle::default();
        s.keys(&[1.0], true, false, true, 0.0);
        s.keys(&[], true, false, true, 0.3);
        assert_eq!(
            s.keys(&[1.0], false, false, true, 0.9),
            [Jkl::Stop, Jkl::Play(1.0)]
        );
    }

    #[test]
    fn j_pressed_during_slow_play_forward_stops_it_and_waits_to_tell_a_tap() {
        let mut s = Shuttle::default();
        s.keys(&[1.0], true, false, true, 0.0);
        s.keys(&[], true, false, true, 0.3);
        assert_eq!(s.keys(&[-1.0], true, true, true, 0.9), [Jkl::Stop]);
    }
}
