use vidcut_core::model::{ColorSpace, Hdr};

use crate::ffmpeg::{ffi, format, frame};

pub const REFERENCE_WHITE: f64 = 203.0;

const PEAK: f64 = 1000.0;

pub fn retag(f: &mut frame::Video, space: ColorSpace) {
    use ffi::AVColorPrimaries as P;
    use ffi::AVColorSpace as S;
    use ffi::AVColorTransferCharacteristic as T;
    use ffi::AVFrameSideDataType as D;
    let (primaries, transfer, matrix) = match space {
        ColorSpace::Tagged => return,
        ColorSpace::Bt709 => (P::AVCOL_PRI_BT709, T::AVCOL_TRC_BT709, S::AVCOL_SPC_BT709),
        ColorSpace::Bt601 => (
            P::AVCOL_PRI_SMPTE170M,
            T::AVCOL_TRC_SMPTE170M,
            S::AVCOL_SPC_SMPTE170M,
        ),
        ColorSpace::Pq => (
            P::AVCOL_PRI_BT2020,
            T::AVCOL_TRC_SMPTE2084,
            S::AVCOL_SPC_BT2020_NCL,
        ),
        ColorSpace::Hlg => (
            P::AVCOL_PRI_BT2020,
            T::AVCOL_TRC_ARIB_STD_B67,
            S::AVCOL_SPC_BT2020_NCL,
        ),
    };
    let raw = unsafe { f.as_mut_ptr() };
    // SAFETY: the frame's own color fields and side data, changed while only this holds it.
    unsafe {
        (*raw).color_primaries = primaries;
        (*raw).color_trc = transfer;
        (*raw).colorspace = matrix;
        for kind in [
            D::AV_FRAME_DATA_MASTERING_DISPLAY_METADATA,
            D::AV_FRAME_DATA_CONTENT_LIGHT_LEVEL,
            D::AV_FRAME_DATA_DYNAMIC_HDR_PLUS,
            D::AV_FRAME_DATA_DOVI_RPU_BUFFER,
            D::AV_FRAME_DATA_DOVI_METADATA,
        ] {
            ffi::av_frame_remove_side_data(raw, kind);
        }
    }
}

pub fn hdr_of(f: &frame::Video) -> Option<Hdr> {
    hdr_of_transfer(unsafe { (*f.as_ptr()).color_trc })
}

pub fn hdr_of_transfer(trc: ffi::AVColorTransferCharacteristic) -> Option<Hdr> {
    use ffi::AVColorTransferCharacteristic as T;
    match trc {
        T::AVCOL_TRC_SMPTE2084 => Some(Hdr::Pq),
        T::AVCOL_TRC_ARIB_STD_B67 => Some(Hdr::Hlg),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Source {
    pub hdr: Option<Hdr>,
    matrix: Matrix,
    full_range: bool,
    rgb: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Matrix {
    Bt601,
    Bt709,
    Bt2020,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Colors {
    pub matrix: Matrix,
    pub full_range: bool,
    pub hdr: Option<Hdr>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShaderColors {
    pub kr: f64,
    pub kb: f64,
    pub full_range: bool,
    pub hdr: Option<Hdr>,
    pub linear_scale: f64,
    pub gamut: [[f64; 3]; 3],
    pub tone: [f64; 4],
}

pub fn shader_colors(colors: Colors) -> ShaderColors {
    let (kr, kb) = match colors.matrix {
        Matrix::Bt601 => (0.299, 0.114),
        Matrix::Bt709 => (0.2126, 0.0722),
        Matrix::Bt2020 => (0.2627, 0.0593),
    };
    let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let (scale, a, b) = mobius();
    ShaderColors {
        kr,
        kb,
        full_range: colors.full_range,
        hdr: colors.hdr,
        linear_scale: match colors.hdr {
            Some(Hdr::Pq) => 10000.0 / REFERENCE_WHITE,
            Some(Hdr::Hlg) => 1000.0 / REFERENCE_WHITE,
            None => 1.0,
        },
        gamut: if colors.hdr.is_some() {
            gamut(Primaries::Bt2020, Primaries::Bt709)
        } else {
            identity
        },
        tone: [KNEE, scale, a, b],
    }
}

pub fn to_bt2020(colors: Colors) -> [[f64; 3]; 3] {
    let from = match (colors.hdr, colors.matrix) {
        (Some(_), _) | (None, Matrix::Bt2020) => Primaries::Bt2020,
        (None, Matrix::Bt709) => Primaries::Bt709,
        (None, Matrix::Bt601) => Primaries::Smpte170m,
    };
    gamut(from, Primaries::Bt2020)
}

pub fn colors_of(f: &frame::Video) -> Colors {
    let s = Source::of(f);
    Colors {
        matrix: s.matrix,
        full_range: s.full_range,
        hdr: s.hdr,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PqLight {
    pub kr: f64,
    pub kb: f64,
    pub full_range: bool,
    pub rgb: bool,
    pub hdr: Option<Hdr>,
    pub gamut: [[f64; 3]; 3],
}

pub(crate) fn pq_light(f: &frame::Video) -> PqLight {
    let s = Source::of(f);
    let sc = shader_colors(Colors {
        matrix: s.matrix,
        full_range: s.full_range,
        hdr: s.hdr,
    });
    PqLight {
        kr: sc.kr,
        kb: sc.kb,
        full_range: s.full_range,
        rgb: s.rgb,
        hdr: s.hdr,
        gamut: gamut(Primaries::of(s), Primaries::Bt2020),
    }
}

pub(crate) fn is_rgb(p: format::Pixel) -> bool {
    p.descriptor()
        .is_some_and(|d| unsafe { (*d.as_ptr()).flags } & ffi::AV_PIX_FMT_FLAG_RGB as u64 != 0)
}

fn untagged_matrix(width: u32, height: u32) -> Matrix {
    if width >= 1280 || height > 576 {
        Matrix::Bt709
    } else {
        Matrix::Bt601
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Tags {
    pub matrix: ffi::AVColorSpace,
    pub range: ffi::AVColorRange,
    pub primaries: ffi::AVColorPrimaries,
    pub transfer: ffi::AVColorTransferCharacteristic,
}

impl Tags {
    pub(crate) fn output(hdr: Option<Hdr>) -> Self {
        use ffi::AVColorPrimaries as P;
        use ffi::AVColorSpace as S;
        use ffi::AVColorTransferCharacteristic as T;
        let (matrix, primaries, transfer) = match hdr {
            None => (S::AVCOL_SPC_BT709, P::AVCOL_PRI_BT709, T::AVCOL_TRC_BT709),
            Some(Hdr::Pq) => (
                S::AVCOL_SPC_BT2020_NCL,
                P::AVCOL_PRI_BT2020,
                T::AVCOL_TRC_SMPTE2084,
            ),
            Some(Hdr::Hlg) => (
                S::AVCOL_SPC_BT2020_NCL,
                P::AVCOL_PRI_BT2020,
                T::AVCOL_TRC_ARIB_STD_B67,
            ),
        };
        Self {
            matrix,
            range: ffi::AVColorRange::AVCOL_RANGE_MPEG,
            primaries,
            transfer,
        }
    }
}

pub(crate) fn tag_frame(f: &mut frame::Video, hdr: Option<Hdr>) {
    let tags = Tags::output(hdr);
    // SAFETY: plain fields of a frame the caller owns.
    unsafe {
        let f = &mut *f.as_mut_ptr();
        f.colorspace = tags.matrix;
        f.color_range = tags.range;
        f.color_primaries = tags.primaries;
        f.color_trc = tags.transfer;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Primaries {
    Bt709,
    Smpte170m,
    Bt2020,
}

impl Primaries {
    fn of(src: Source) -> Self {
        match src.primaries() {
            "2020" => Self::Bt2020,
            "170m" => Self::Smpte170m,
            _ => Self::Bt709,
        }
    }

    fn xy(self) -> [(f64, f64); 3] {
        match self {
            Self::Bt709 => [(0.640, 0.330), (0.300, 0.600), (0.150, 0.060)],
            Self::Smpte170m => [(0.630, 0.340), (0.310, 0.595), (0.155, 0.070)],
            Self::Bt2020 => [(0.708, 0.292), (0.170, 0.797), (0.131, 0.046)],
        }
    }

    fn to_xyz(self) -> [[f64; 3]; 3] {
        const WHITE: (f64, f64) = (0.3127, 0.3290);
        let xyz = |(x, y): (f64, f64)| [x / y, 1.0, (1.0 - x - y) / y];
        let p = self.xy().map(xyz);
        let m = [
            [p[0][0], p[1][0], p[2][0]],
            [p[0][1], p[1][1], p[2][1]],
            [p[0][2], p[1][2], p[2][2]],
        ];
        let s = mul(invert(m), xyz(WHITE));
        m.map(|row| [row[0] * s[0], row[1] * s[1], row[2] * s[2]])
    }
}

fn invert(m: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let c = |r: usize, k: usize| {
        let (r1, r2) = ((r + 1) % 3, (r + 2) % 3);
        let (k1, k2) = ((k + 1) % 3, (k + 2) % 3);
        m[r1][k1] * m[r2][k2] - m[r1][k2] * m[r2][k1]
    };
    let det = m[0][0] * c(0, 0) + m[0][1] * c(0, 1) + m[0][2] * c(0, 2);
    std::array::from_fn(|r| std::array::from_fn(|k| c(k, r) / det))
}

fn mul(m: [[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    m.map(|row| row[0] * v[0] + row[1] * v[1] + row[2] * v[2])
}

fn mat_mul(a: [[f64; 3]; 3], b: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    std::array::from_fn(|r| std::array::from_fn(|k| (0..3).map(|i| a[r][i] * b[i][k]).sum()))
}

pub fn output_gamut(from: Option<Hdr>, to: Option<Hdr>) -> [[f64; 3]; 3] {
    let primaries = |hdr: Option<Hdr>| match hdr {
        Some(_) => Primaries::Bt2020,
        None => Primaries::Bt709,
    };
    gamut(primaries(from), primaries(to))
}

fn gamut(from: Primaries, to: Primaries) -> [[f64; 3]; 3] {
    if from == to {
        return [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    }
    mat_mul(invert(to.to_xyz()), from.to_xyz())
}

fn mobius() -> (f64, f64, f64) {
    let j = KNEE;
    let peak = mobius_peak();
    let a = -j * j * (peak - 1.0) / (j * j - 2.0 * j + peak);
    let b = (j * j - 2.0 * j * peak + peak) / (peak - 1.0).max(1e-6);
    ((b * b + 2.0 * b * j + j * j) / (b - a), a, b)
}

const KNEE: f64 = 0.8;

fn mobius_peak() -> f64 {
    (PEAK / REFERENCE_WHITE * 1e4).round() / 1e4
}

impl Source {
    pub(crate) fn of(f: &frame::Video) -> Self {
        use ffi::AVColorSpace as S;
        use format::Pixel as P;
        let raw = unsafe { &*f.as_ptr() };
        let hdr = hdr_of(f);
        let matrix = match raw.colorspace {
            S::AVCOL_SPC_BT709 => Matrix::Bt709,
            S::AVCOL_SPC_BT470BG | S::AVCOL_SPC_SMPTE170M | S::AVCOL_SPC_FCC => Matrix::Bt601,
            S::AVCOL_SPC_BT2020_NCL | S::AVCOL_SPC_BT2020_CL => Matrix::Bt2020,
            _ if hdr.is_some() => Matrix::Bt2020,
            _ => untagged_matrix(f.width(), f.height()),
        };
        Self {
            hdr,
            matrix,
            full_range: raw.color_range == ffi::AVColorRange::AVCOL_RANGE_JPEG
                || matches!(f.format(), P::YUVJ420P | P::YUVJ422P | P::YUVJ444P),
            rgb: is_rgb(f.format()),
        }
    }

    fn primaries(self) -> &'static str {
        match (self.hdr, self.matrix) {
            (Some(_), _) | (None, Matrix::Bt2020) => "2020",
            (None, Matrix::Bt709) => "709",
            (None, Matrix::Bt601) => "170m",
        }
    }
}

pub(crate) const CONVERT: &str = "bicubic+accurate_rnd+full_chroma_int";

pub(crate) fn keep(f: &frame::Video) -> String {
    let src = Source::of(f);
    if src.rgb {
        return String::new();
    }
    let matrix = match src.matrix {
        Matrix::Bt601 => "bt601",
        Matrix::Bt709 => "bt709",
        Matrix::Bt2020 => "bt2020",
    };
    let range = if src.full_range { "full" } else { "limited" };
    format!("in_color_matrix={matrix}:in_range={range}:out_color_matrix={matrix}:out_range={range}")
}

pub(crate) fn tag(ctx: *mut ffi::AVCodecContext, hdr: Option<Hdr>) {
    use ffi::AVColorPrimaries as P;
    use ffi::AVColorSpace as S;
    use ffi::AVColorTransferCharacteristic as T;
    let (p, t, s) = match hdr {
        None => (P::AVCOL_PRI_BT709, T::AVCOL_TRC_BT709, S::AVCOL_SPC_BT709),
        Some(Hdr::Pq) => (
            P::AVCOL_PRI_BT2020,
            T::AVCOL_TRC_SMPTE2084,
            S::AVCOL_SPC_BT2020_NCL,
        ),
        Some(Hdr::Hlg) => (
            P::AVCOL_PRI_BT2020,
            T::AVCOL_TRC_ARIB_STD_B67,
            S::AVCOL_SPC_BT2020_NCL,
        ),
    };
    unsafe {
        (*ctx).color_primaries = p;
        (*ctx).color_trc = t;
        (*ctx).colorspace = s;
        (*ctx).color_range = ffi::AVColorRange::AVCOL_RANGE_MPEG;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gamut_matrix_is_bt2020_to_bt709s() {
        let m = gamut(Primaries::Bt2020, Primaries::Bt709);
        let want = [
            [1.6605, -0.5876, -0.0728],
            [-0.1246, 1.1329, -0.0083],
            [-0.0182, -0.1006, 1.1187],
        ];
        for r in 0..3 {
            assert!((m[r].iter().sum::<f64>() - 1.0).abs() < 1e-9);
            for c in 0..3 {
                assert!((m[r][c] - want[r][c]).abs() < 1e-4, "{m:?}");
            }
        }
    }

    #[test]
    fn untagged_pictures_are_guessed_as_players_guess() {
        for (w, h, hd) in [
            (1280, 534, true),
            (1920, 800, true),
            (1920, 1080, true),
            (720, 1280, true),
            (720, 576, false),
            (720, 480, false),
            (1024, 576, false),
            (640, 360, false),
        ] {
            let want = if hd { Matrix::Bt709 } else { Matrix::Bt601 };
            assert_eq!(untagged_matrix(w, h), want, "{w}x{h}");
        }
    }
}
