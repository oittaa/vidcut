use crate::model::CompositeMode;

const BT709: [f64; 3] = [0.2126, 0.7152, 0.0722];
const BT601: [f64; 3] = [0.299, 0.587, 0.114];
const FROM_CB: [f64; 3] = [-0.001336, -0.343456, 1.770592];
const FROM_CR: [f64; 3] = [1.402406, -0.714457, 0.002013];

fn luma(c: [f64; 3], w: [f64; 3]) -> f64 {
    c[0] * w[0] + c[1] * w[1] + c[2] * w[2]
}

fn spread([r, g, b]: [f64; 3]) -> f64 {
    ((r - g) * (r - g) + (g - b) * (g - b) + (b - r) * (b - r)).sqrt()
}

fn ycc(c: [f64; 3]) -> [f64; 3] {
    let y = luma(c, BT601);
    [
        y,
        (c[2] - y) / (2.0 * (1.0 - BT601[2])),
        (c[0] - y) / (2.0 * (1.0 - BT601[0])),
    ]
}

fn rgb([y, cb, cr]: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| y + cb * FROM_CB[i] + cr * FROM_CR[i])
}

fn dodge(b: f64, a: f64) -> f64 {
    if a >= 1.0 {
        1.0
    } else {
        (b / (1.0 - a)).min(1.0)
    }
}

fn burn(b: f64, a: f64) -> f64 {
    if a <= 0.0 {
        0.0
    } else {
        1.0 - ((1.0 - b) / a).min(1.0)
    }
}

fn hard(by: f64, other: f64) -> f64 {
    if by <= 0.5 {
        2.0 * by * other
    } else {
        1.0 - 2.0 * (1.0 - by) * (1.0 - other)
    }
}

fn soft(a: f64, b: f64) -> f64 {
    if a <= 0.5 {
        return b - (1.0 - 2.0 * a) * b * (1.0 - b);
    }
    let d = if b <= 0.25 {
        ((16.0 * b - 12.0) * b + 4.0) * b
    } else {
        b.sqrt()
    };
    b + (2.0 * a - 1.0) * (d - b)
}

fn divide(b: f64, a: f64) -> f64 {
    match a <= 0.0 {
        true if b > 0.0 => 1.0,
        true => 0.0,
        false => (b / a).min(1.0),
    }
}

fn each(a: [f64; 3], b: [f64; 3], f: impl Fn(f64, f64) -> f64) -> [f64; 3] {
    std::array::from_fn(|i| f(a[i], b[i]))
}

pub fn mixed(mode: CompositeMode, a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    use CompositeMode as M;
    let sum = |c: [f64; 3]| c[0] + c[1] + c[2];
    match mode {
        M::Normal => a,
        M::Add | M::LinearDodge => each(a, b, |a, b| a + b),
        M::Subtract => each(a, b, |a, b| b - a),
        M::Difference => each(a, b, |a, b| (a - b).abs()),
        M::Multiply => each(a, b, |a, b| a * b),
        M::Screen => each(a, b, |a, b| 1.0 - (1.0 - a) * (1.0 - b)),
        M::Overlay => each(a, b, |a, b| hard(b, a)),
        M::Hardlight => each(a, b, hard),
        M::Softlight => each(a, b, soft),
        M::Darken => each(a, b, f64::min),
        M::Lighten => each(a, b, f64::max),
        M::ColorDodge => each(a, b, |a, b| dodge(b, a)),
        M::ColorBurn => each(a, b, |a, b| burn(b, a)),
        M::Exclusion => each(a, b, |a, b| a + b - 2.0 * a * b),
        M::Hue => {
            let wide = spread(a);
            let k = if wide > 0.0 { spread(b) / wide } else { 0.0 };
            let (la, lb) = (luma(a, BT709), luma(b, BT709));
            a.map(|x| lb + k * (x - la))
        }
        M::Saturation => {
            let wide = spread(b);
            let k = if wide > 0.0 { spread(a) / wide } else { 0.0 };
            let lb = luma(b, BT709);
            b.map(|x| lb + k * (x - lb))
        }
        M::Color => {
            let ([_, cb, cr], [y, ..]) = (ycc(a), ycc(b));
            rgb([y, cb, cr])
        }
        M::Luminosity => {
            let ([y, ..], [_, cb, cr]) = (ycc(a), ycc(b));
            rgb([y, cb, cr])
        }
        M::Divide => each(a, b, |a, b| divide(b, a)),
        M::LinearBurn => each(a, b, |a, b| a + b - 1.0),
        M::LinearLight => each(a, b, |a, b| b + 2.0 * a - 1.0),
        M::VividLight => each(a, b, |a, b| {
            if a <= 0.5 {
                burn(b, 2.0 * a)
            } else {
                dodge(b, 2.0 * a - 1.0)
            }
        }),
        M::PinLight => each(a, b, |a, b| {
            if a <= 0.5 {
                b.min(2.0 * a)
            } else {
                b.max(2.0 * a - 1.0)
            }
        }),
        M::HardMix => each(a, b, |a, b| if a + b >= 1.0 { 1.0 } else { 0.0 }),
        M::LighterColor => {
            if sum(a) > sum(b) {
                a
            } else {
                b
            }
        }
        M::DarkerColor => {
            if sum(a) < sum(b) {
                a
            } else {
                b
            }
        }
    }
}
