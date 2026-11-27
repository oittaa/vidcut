use crate::analysis::{Detections, Found};
use crate::model::PathSample;
use crate::motion::{FoundPath, Join, joined};

const RULES: Join = Join {
    start: 0.5,
    keep: 0.3,
    overlap: 0.2,
    wait_secs: 1.0,
    shortest_secs: 0.0,
    steady: 0.0,
};

fn found(boxes: std::ops::Range<usize>, cut: Option<usize>) -> Vec<FoundPath> {
    let face = |i: usize| Found {
        center: [0.1 + 0.02 * i as f32, 0.5],
        size: [0.25, 0.5],
        rotation: 0.0,
        score: 0.9,
    };
    let detections = Detections {
        pts: (0..30).map(|i| f64::from(i) / 25.0).collect(),
        found: (0..30)
            .map(|i| boxes.contains(&i).then(|| face(i)).into_iter().collect())
            .collect(),
        scene: (0..30)
            .map(|i| if Some(i) == cut { 20.0 } else { 0.0 })
            .collect(),
    };
    joined(&detections, 0.0, 25.0, (0, 30), &RULES)
}

fn span(paths: &[FoundPath]) -> (i64, i64) {
    let s = &paths[0].samples;
    (s[0].frame, s[s.len() - 1].frame)
}

#[test]
fn a_found_path_holds_its_first_and_last_box_a_fifth_of_a_second_longer() {
    let at = |frame, x| PathSample {
        frame,
        center: [x, 0.5],
        size: [0.25, 0.5],
        rotation: 0.0,
    };
    let paths = found(10..15, None);
    let samples = vec![at(5, 0.3), at(10, 0.3), at(14, 0.38), at(19, 0.38)];
    assert_eq!(paths[0].samples, samples);
}

#[test]
fn the_held_frames_are_no_gap_and_no_sight() {
    let paths = found(10..15, None);
    assert_eq!((paths[0].seen, paths[0].gaps.clone()), (5, vec![]));
}

#[test]
fn a_found_path_holds_no_box_before_the_shot_change() {
    assert_eq!(span(&found(10..15, Some(8))), (8, 19));
}

#[test]
fn a_found_path_holds_no_box_after_the_shot_change() {
    assert_eq!(span(&found(10..15, Some(17))), (5, 16));
}

#[test]
fn a_found_path_holds_no_box_outside_the_clip() {
    assert_eq!(span(&found(2..28, None)), (0, 29));
}
