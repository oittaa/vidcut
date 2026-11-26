fn filter(x: f64) -> f64 {
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

pub(crate) struct Taps {
    pub(crate) bounds: Vec<i32>,
    pub(crate) weights: Vec<f64>,
    pub(crate) size: usize,
}

pub(crate) fn taps(input: u32, output: u32) -> Taps {
    let scale = f64::from(input) / f64::from(output);
    let filterscale = scale.max(1.0);
    let support = 2.0 * filterscale;
    let size = support.ceil() as usize * 2 + 1;
    let out = output as usize;
    let mut bounds = vec![0i32; out * 2];
    let mut weights = vec![0f64; out * size];
    for xx in 0..out {
        let center = (xx as f64 + 0.5) * scale;
        let ss = 1.0 / filterscale;
        let xmin = ((center - support + 0.5) as i64).max(0);
        let xmax = ((center + support + 0.5) as i64).min(i64::from(input)) - xmin;
        let k = &mut weights[xx * size..(xx + 1) * size];
        let mut ww = 0.0;
        for (x, slot) in k.iter_mut().enumerate().take(xmax.max(0) as usize) {
            let w = filter((x as f64 + xmin as f64 - center + 0.5) * ss);
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
    Taps {
        bounds,
        weights,
        size,
    }
}
