const PRECISION: u32 = 22;

fn bicubic(x: f64) -> f64 {
    let a = -0.5;
    let x = x.abs();
    if x < 1.0 {
        ((a + 2.0) * x - (a + 3.0)) * x * x + 1.0
    } else if x < 2.0 {
        (((x - 5.0) * x + 8.0) * x - 4.0) * a
    } else {
        0.0
    }
}

pub(super) fn coefficients(input: u32, output: u32) -> (Vec<i32>, Vec<i32>, u32) {
    let scale = f64::from(input) / f64::from(output);
    let filterscale = scale.max(1.0);
    let support = 2.0 * filterscale;
    let ksize = support.ceil() as usize * 2 + 1;
    let out = output as usize;
    let mut bounds = vec![0i32; out * 2];
    let mut kk = vec![0f64; out * ksize];
    for xx in 0..out {
        let center = (xx as f64 + 0.5) * scale;
        let ss = 1.0 / filterscale;
        let xmin = ((center - support + 0.5) as i64).max(0);
        let xmax = ((center + support + 0.5) as i64).min(i64::from(input)) - xmin;
        let k = &mut kk[xx * ksize..(xx + 1) * ksize];
        let mut ww = 0.0;
        for (x, slot) in k.iter_mut().enumerate().take(xmax.max(0) as usize) {
            let w = bicubic((x as f64 + xmin as f64 - center + 0.5) * ss);
            *slot = w;
            ww += w;
        }
        if ww != 0.0 {
            for slot in k.iter_mut().take(xmax.max(0) as usize) {
                *slot /= ww;
            }
        }
        bounds[xx * 2] = xmin as i32;
        bounds[xx * 2 + 1] = xmax as i32;
    }
    let one = f64::from(1u32 << PRECISION);
    let coef = kk
        .iter()
        .map(|v| {
            if *v < 0.0 {
                (-0.5 + v * one) as i32
            } else {
                (0.5 + v * one) as i32
            }
        })
        .collect();
    (bounds, coef, ksize as u32)
}
