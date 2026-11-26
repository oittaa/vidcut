const PRECISION: u32 = 22;

pub(super) fn coefficients(input: u32, output: u32) -> (Vec<i32>, Vec<i32>, u32) {
    let taps = crate::bicubic::taps(input, output);
    let one = f64::from(1u32 << PRECISION);
    let coef = taps
        .weights
        .iter()
        .map(|v| {
            if *v < 0.0 {
                (-0.5 + v * one) as i32
            } else {
                (0.5 + v * one) as i32
            }
        })
        .collect();
    (taps.bounds, coef, taps.size as u32)
}
