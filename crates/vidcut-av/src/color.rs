//! Color: SDR (BT.709) and HDR (BT.2100: PQ and HLG) pictures, the filters
//! that convert between them, and the tags an encoder writes.
//!
//! HDR into SDR is tone-mapped. SDR into HDR puts SDR white at 203 nits,
//! the reference white of ITU-R BT.2408. Untagged pictures are BT.709 when
//! HD (1280 wide, or over 576 lines), else BT.601, as players commonly take
//! them. On the GPU, libplacebo converts with the same math
//! ([`gpu_color`]), and so does the viewer's shader ([`shader_colors`]).

use vidcut_core::model::Hdr;

use crate::ffmpeg::{ffi, format, frame};

/// Luminance (nits) that SDR white maps to in HDR; while converting, linear
/// light 1.0 stands for it.
pub const REFERENCE_WHITE: f64 = 203.0;

/// The brightest HDR highlight the tone curve keeps apart (nits): what most
/// HDR video is mastered for. Brighter ones clip to white.
const PEAK: f64 = 1000.0;

/// The HDR transfer of `f`, or `None` for SDR.
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

/// What a picture's colors are, from its tags or else assumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Source {
    pub hdr: Option<Hdr>,
    matrix: Matrix,
    full_range: bool,
    rgb: bool,
}

/// A YUV picture's matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Matrix {
    Bt601,
    Bt709,
    Bt2020,
}

/// How a decoded picture's samples are read to get BT.709 RGB, as the
/// export reads them: what a display's shader converts from. Untagged
/// pictures get the export's assumptions (BT.709 when HD, else BT.601;
/// BT.2020 when HDR).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Colors {
    pub matrix: Matrix,
    /// Full range (0–255) rather than video range (16–235).
    pub full_range: bool,
    /// PQ or HLG: tone-mapped into SDR (`conversion`'s curve).
    pub hdr: Option<Hdr>,
}

/// The numbers a display's shader converts a picture's YUV to BT.709 RGB
/// with, as the export does (`conversion`): the matrix and range for SDR,
/// and for HDR the transfer to linear light, the gamut to BT.709, the tone
/// curve on the brightest channel and BT.709's transfer, the math of the
/// GPU export's shader ([`gpu_color`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShaderColors {
    /// Kr and Kb of the YUV matrix.
    pub kr: f64,
    pub kb: f64,
    pub full_range: bool,
    /// PQ or HLG: tone-mapped into SDR.
    pub hdr: Option<Hdr>,
    /// What linear light is multiplied by so that the reference white is
    /// 1.0: PQ's 10,000 nits or HLG's 1,000-nit display over 203.
    pub linear_scale: f64,
    /// Linear BT.2020 to BT.709, by rows (HDR only; the identity for SDR).
    pub gamut: [[f64; 3]; 3],
    /// The tone curve: the knee, then `scale`, `a` and `b` of
    /// `scale * (x + a) / (x + b)` above it.
    pub tone: [f64; 4],
}

/// The shader's numbers for pictures read as `colors`.
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

/// How `f`'s samples are read (see [`Colors`]).
pub fn colors_of(f: &frame::Video) -> Colors {
    let s = Source::of(f);
    Colors {
        matrix: s.matrix,
        full_range: s.full_range,
        hdr: s.hdr,
    }
}

/// Whether `src` goes into output of `target`'s dynamic range (`None`:
/// SDR BT.709) as it is: video-range YUV in the output's matrix and
/// transfer, which a scaler only resamples.
pub(crate) fn kept(src: Source, target: Option<Hdr>) -> bool {
    !src.rgb
        && !src.full_range
        && match (src.hdr, target) {
            (None, None) => src.matrix == Matrix::Bt709,
            (Some(s), Some(t)) => s == t && src.matrix == Matrix::Bt2020,
            _ => false,
        }
}

/// `scaled`, a copy of `f` at another size, tagged with the matrix `f` is
/// read with: an untagged picture's follows from its size.
pub(crate) fn keep_matrix(scaled: &mut frame::Video, f: &frame::Video) {
    use ffi::AVColorSpace as S;
    let matrix = match Source::of(f).matrix {
        Matrix::Bt601 => S::AVCOL_SPC_SMPTE170M,
        Matrix::Bt709 => S::AVCOL_SPC_BT709,
        Matrix::Bt2020 => S::AVCOL_SPC_BT2020_NCL,
    };
    // SAFETY: a plain field of its own frame.
    unsafe { (*scaled.as_mut_ptr()).colorspace = matrix };
}

/// How to read `f`'s samples as the light they have in PQ output, as the
/// export converts them: the YUV matrix and range, the transfer, and the
/// gamut to BT.2020 in linear light (the identity for HDR).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PqLight {
    pub kr: f64,
    pub kb: f64,
    pub full_range: bool,
    /// Already RGB (images): no matrix.
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

fn is_rgb(p: format::Pixel) -> bool {
    p.descriptor()
        .is_some_and(|d| unsafe { (*d.as_ptr()).flags } & ffi::AV_PIX_FMT_FLAG_RGB as u64 != 0)
}

/// The matrix an untagged SDR picture of `width`×`height` is taken to use:
/// players' common guess (BT.709 from 1280 wide or over 576 lines, else
/// BT.601), which the viewer reads them with too. A 1280×534 widescreen
/// film is HD this way, though it has fewer than 720 lines.
fn untagged_matrix(width: u32, height: u32) -> Matrix {
    if width >= 1280 || height > 576 {
        Matrix::Bt709
    } else {
        Matrix::Bt601
    }
}

/// The colors a picture is tagged with for libplacebo: how its YUV is read
/// (matrix, range) and what its RGB is (primaries, transfer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Tags {
    pub matrix: ffi::AVColorSpace,
    pub range: ffi::AVColorRange,
    pub primaries: ffi::AVColorPrimaries,
    pub transfer: ffi::AVColorTransferCharacteristic,
}

impl Tags {
    /// What an encoder writes for `hdr` (`None`: SDR BT.709), video range.
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

/// How libplacebo, on the GPU, is to treat a YUV picture so that it comes
/// out as [`conversion`] makes it on the CPU.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct GpuColor {
    /// What libplacebo reads the picture as: the matrix and range the CPU
    /// reads it with, and the primaries and transfer it has once `shader`
    /// has run (the output's), or its own when only the matrix changes.
    pub input: Tags,
    /// What it writes: the output's colors.
    pub output: Tags,
    /// A user shader (mpv's hook format) doing the CPU's math on the RGB:
    /// the transfer to linear light, the primaries, the tone curve and the
    /// output's transfer. `None` when only the matrix and range change, as
    /// swscale converts SDR. libplacebo's own HDR conversion differs: it
    /// lifts black for the SDR display's contrast, maps the tone curve on
    /// intensity instead of the brightest channel, and leaves colors outside
    /// BT.709 outside the range.
    pub shader: Option<String>,
}

impl GpuColor {
    /// The CPU leaves the colors as they are.
    pub(crate) fn unchanged(&self) -> bool {
        self.shader.is_none() && self.input == self.output
    }
}

/// How the GPU converts YUV picture `f` to an output of `target`'s dynamic
/// range (`None`: SDR BT.709) as the CPU does. `None` for RGB pictures.
pub(crate) fn gpu_color(f: &frame::Video, target: Option<Hdr>) -> Option<GpuColor> {
    let src = Source::of(f);
    let (matrix, range) = src.yuv_tags()?;
    let output = Tags::output(target);
    Some(match (src.hdr, target) {
        // swscale converts the matrix and range only.
        (None, None) => GpuColor {
            input: Tags {
                matrix,
                range,
                ..output
            },
            output,
            shader: None,
        },
        // The same HDR transfer: nothing to convert.
        (Some(s), Some(t)) if s == t && src.matrix == Matrix::Bt2020 => GpuColor {
            input: output,
            output,
            shader: None,
        },
        _ => GpuColor {
            input: Tags {
                matrix,
                range,
                ..output
            },
            output,
            shader: Some(shader(src, target)),
        },
    })
}

/// Linear light's primaries while converting, as [`Source::primaries`]
/// names them for `zscale`.
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

    /// CIE xy of red, green and blue.
    fn xy(self) -> [(f64, f64); 3] {
        match self {
            Self::Bt709 => [(0.640, 0.330), (0.300, 0.600), (0.150, 0.060)],
            Self::Smpte170m => [(0.630, 0.340), (0.310, 0.595), (0.155, 0.070)],
            Self::Bt2020 => [(0.708, 0.292), (0.170, 0.797), (0.131, 0.046)],
        }
    }

    /// The matrix from linear RGB in these primaries to CIE XYZ, white
    /// (D65) at Y = 1.
    fn to_xyz(self) -> [[f64; 3]; 3] {
        const WHITE: (f64, f64) = (0.3127, 0.3290);
        let xyz = |(x, y): (f64, f64)| [x / y, 1.0, (1.0 - x - y) / y];
        let p = self.xy().map(xyz);
        // Columns are the primaries, scaled so that they add up to white.
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
    // The adjugate is the transposed cofactors.
    std::array::from_fn(|r| std::array::from_fn(|k| c(k, r) / det))
}

fn mul(m: [[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    m.map(|row| row[0] * v[0] + row[1] * v[1] + row[2] * v[2])
}

fn mat_mul(a: [[f64; 3]; 3], b: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    std::array::from_fn(|r| std::array::from_fn(|k| (0..3).map(|i| a[r][i] * b[i][k]).sum()))
}

/// The matrix taking linear RGB in `from` primaries to `to`: the identity
/// itself for the same, not one off by rounding.
fn gamut(from: Primaries, to: Primaries) -> [[f64; 3]; 3] {
    if from == to {
        return [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    }
    mat_mul(invert(to.to_xyz()), from.to_xyz())
}

/// The tone curve `tonemap=mobius` applies to the brightest channel: the
/// identity up to `KNEE`, rolling off to 1.0 at the peak. Returns the
/// curve's `(scale, a, b)` above the knee: `scale * (x + a) / (x + b)`.
fn mobius() -> (f64, f64, f64) {
    let j = KNEE;
    let peak = mobius_peak();
    let a = -j * j * (peak - 1.0) / (j * j - 2.0 * j + peak);
    let b = (j * j - 2.0 * j * peak + peak) / (peak - 1.0).max(1e-6);
    ((b * b + 2.0 * b * j + j * j) / (b - a), a, b)
}

/// Where the tone curve leaves the identity, in reference whites.
const KNEE: f64 = 0.8;

/// The peak the tone curve rolls off to, in reference whites, as the CPU's
/// filter is given it (to four decimals).
fn mobius_peak() -> f64 {
    (PEAK / REFERENCE_WHITE * 1e4).round() / 1e4
}

/// GLSL: `c` (the RGB in `hdr`'s transfer) to linear light, the reference
/// white at 1.0, as `zscale` (zimg) computes it: PQ's EOTF, HLG's inverse
/// OETF and 1.2 system gamma channel by channel on a 1000-nit display (zimg's
/// approximate path, which FFmpeg's `zscale` takes), BT.1886 as a 2.4 power.
fn glsl_to_linear(hdr: Option<Hdr>) -> String {
    match hdr {
        Some(Hdr::Pq) => format!(
            "    vec3 p = pow(max(c, 0.0), vec3(1.0 / 78.84375));\n\
             \x20   vec3 num = max(p - vec3(0.8359375), 0.0);\n\
             \x20   vec3 den = max(vec3(18.8515625) - 18.6875 * p, vec3(1.17549435e-38));\n\
             \x20   c = pow(num / den, vec3(1.0 / 0.1593017578125)) * {};\n",
            glsl_float(10000.0 / REFERENCE_WHITE)
        ),
        Some(Hdr::Hlg) => format!(
            "    c = max(c, 0.0);\n\
             \x20   c = mix((exp((c - vec3(0.55991073)) / 0.17883277) + vec3(0.28466892)) / 12.0,\n\
             \x20           c * c / 3.0, lessThanEqual(c, vec3(0.5)));\n\
             \x20   c = pow(c, vec3(1.2)) * {};\n",
            glsl_float(1000.0 / REFERENCE_WHITE)
        ),
        None => "    c = pow(max(c, 0.0), vec3(2.4));\n".into(),
    }
}

/// GLSL: linear light `c` (reference white at 1.0) to `hdr`'s transfer, as
/// zimg computes it. Negative values (colors outside the primaries) are
/// black; values above white stay above it, for the integer conversion to
/// clip, as on the CPU.
fn glsl_from_linear(hdr: Option<Hdr>) -> String {
    match hdr {
        Some(Hdr::Pq) => format!(
            "    vec3 l = max(c * {}, 0.0);\n\
             \x20   vec3 p = pow(l, vec3(0.1593017578125));\n\
             \x20   vec3 q = pow(vec3(1.0) + (vec3(0.8359375 - 1.0) + (18.8515625 - 18.6875) * p)\n\
             \x20                / (vec3(1.0) + 18.6875 * p), vec3(78.84375));\n\
             \x20   c = mix(vec3(0.0), q, greaterThan(l, vec3(0.0)));\n",
            glsl_float(REFERENCE_WHITE / 10000.0)
        ),
        Some(Hdr::Hlg) => format!(
            "    vec3 l = pow(max(c * {}, 0.0), vec3(1.0 / 1.2));\n\
             \x20   c = mix(0.17883277 * log(max(12.0 * l - vec3(0.28466892), 1e-30)) + vec3(0.55991073),\n\
             \x20           sqrt(3.0 * l), lessThanEqual(l, vec3(1.0 / 12.0)));\n",
            glsl_float(REFERENCE_WHITE / 1000.0)
        ),
        None => "    c = pow(max(c, 0.0), vec3(1.0 / 2.4));\n".into(),
    }
}

fn glsl_float(v: f64) -> String {
    format!("{v:.10}")
}

/// A GLSL `mat3` that multiplies a column vector as `m` does (GLSL takes
/// the columns in order).
fn glsl_mat3(m: [[f64; 3]; 3]) -> String {
    let cols: Vec<String> = (0..3)
        .flat_map(|k| (0..3).map(move |r| (r, k)))
        .map(|(r, k)| glsl_float(m[r][k]))
        .collect();
    format!("mat3({})", cols.join(", "))
}

/// The libplacebo user shader converting `src` pictures' RGB to an output of
/// `target`'s dynamic range as [`conversion`] does on the CPU: linear light
/// with the reference white at 1.0, the output's primaries, the tone curve
/// on the brightest channel (HDR into SDR), the output's transfer. It hooks
/// the scaled picture, as the CPU scales before converting.
fn shader(src: Source, target: Option<Hdr>) -> String {
    let (from, to) = (
        Primaries::of(src),
        if target.is_some() {
            Primaries::Bt2020
        } else {
            Primaries::Bt709
        },
    );
    let mut body = glsl_to_linear(src.hdr);
    if from != to {
        body += &format!("    c = {} * c;\n", glsl_mat3(gamut(from, to)));
    }
    if src.hdr.is_some() && target.is_none() {
        let (scale, a, b) = mobius();
        body += &format!(
            "    float sig = max(max(max(c.r, c.g), c.b), 1e-6);\n\
             \x20   c *= (sig <= {knee} ? sig : {scale} * (sig + {a}) / (sig + {b})) / sig;\n",
            knee = glsl_float(KNEE),
            scale = glsl_float(scale),
            a = glsl_float(a),
            b = glsl_float(b),
        );
    }
    body += &glsl_from_linear(target);
    format!(
        "//!HOOK SCALED\n\
         //!BIND HOOKED\n\
         //!DESC vidcut colors\n\
         {HALF_STEP}\
         vec4 hook()\n\
         {{\n\
         \x20   vec4 color = HOOKED_tex(HOOKED_pos);\n\
         \x20   vec3 c = color.rgb;\n\
         \x20   c += vidcut_half_step(c);\n\
         {body}\
         \x20   c += vidcut_half_step(c);\n\
         \x20   return vec4(c, color.a);\n\
         }}\n"
    )
}

/// libplacebo keeps a picture between passes in half floats, and the
/// NVIDIA GPU stores them rounded toward zero: up to a whole step low (on
/// PQ, almost half a 10-bit code), half a step on average, which left the
/// GPU's pictures half a level darker than the CPU's. The shader takes the
/// middle of the step a value it reads came from, and moves a value it
/// writes half a step up, so that it is stored rounded to the nearest.
const HALF_STEP: &str = "vec3 vidcut_half_step(vec3 v)\n\
     {\n\
     \x20   return sign(v) * exp2(floor(log2(max(abs(v), vec3(1e-20)))) - 11.0);\n\
     }\n";

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

    /// An RGB picture (an image), not YUV.
    pub(crate) fn rgb(self) -> bool {
        self.rgb
    }

    /// The matrix and range a YUV picture is read with, as FFmpeg tags
    /// them. `None` for RGB.
    pub(crate) fn yuv_tags(self) -> Option<(ffi::AVColorSpace, ffi::AVColorRange)> {
        use ffi::AVColorSpace as S;
        if self.rgb {
            return None;
        }
        let matrix = match self.matrix {
            Matrix::Bt601 => S::AVCOL_SPC_SMPTE170M,
            Matrix::Bt709 => S::AVCOL_SPC_BT709,
            Matrix::Bt2020 => S::AVCOL_SPC_BT2020_NCL,
        };
        let range = if self.full_range {
            ffi::AVColorRange::AVCOL_RANGE_JPEG
        } else {
            ffi::AVColorRange::AVCOL_RANGE_MPEG
        };
        Some((matrix, range))
    }

    /// `zscale`'s primaries of the input.
    fn primaries(self) -> &'static str {
        match (self.hdr, self.matrix) {
            (Some(_), _) | (None, Matrix::Bt2020) => "2020",
            (None, Matrix::Bt709) => "709",
            (None, Matrix::Bt601) => "170m",
        }
    }

    /// `zscale`'s description of the input and of the linear light it is
    /// converted to, in the same primaries (zimg otherwise takes the output
    /// primaries from the frame, which may have none).
    fn to_linear(self) -> String {
        format!(
            "{}:t=linear:p={}:npl={REFERENCE_WHITE}",
            self.zscale_input(),
            self.primaries()
        )
    }

    /// `zscale`'s description of the input.
    fn zscale_input(self) -> String {
        let matrix = match self.matrix {
            Matrix::Bt601 => "170m",
            Matrix::Bt709 => "709",
            Matrix::Bt2020 => "2020_ncl",
        };
        let primaries = self.primaries();
        let transfer = transfer(self.hdr);
        if self.rgb {
            format!("tin={transfer}:pin={primaries}:rin=full")
        } else {
            let range = if self.full_range { "full" } else { "limited" };
            format!("tin={transfer}:pin={primaries}:min={matrix}:rin={range}")
        }
    }
}

/// `zscale`'s name for a transfer.
fn transfer(hdr: Option<Hdr>) -> &'static str {
    match hdr {
        Some(Hdr::Pq) => "smpte2084",
        Some(Hdr::Hlg) => "arib-std-b67",
        None => "709",
    }
}

/// How a normalizer brings a source to its output: flags and options for
/// the `scale` filter, and filters to run after padding (which happens in
/// the source's format), before the final `format`.
pub(crate) struct Conversion {
    pub flags: &'static str,
    pub scale: String,
    pub after: String,
}

/// swscale's flags for a picture it only resamples.
const RESAMPLE: &str = "bicubic";

/// swscale's flags for a picture whose colors it converts too (YUV to RGB
/// or back, another matrix or range): left to its defaults, it rounds them
/// low (export.md §4).
pub(crate) const CONVERT: &str = "bicubic+accurate_rnd+full_chroma_int";

/// `scale` options that keep a YUV picture's matrix and range (its tags,
/// or what is assumed without them) while it is resampled: told nothing,
/// swscale converts to BT.601. Empty for RGB pictures.
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

/// The conversion from `src` to an `out` picture of `target`'s dynamic
/// range (`None`: SDR BT.709).
pub(crate) fn conversion(src: Source, out: format::Pixel, target: Option<Hdr>) -> Conversion {
    let rgb_out = is_rgb(out);
    let in_matrix = match src.matrix {
        Matrix::Bt601 => "bt601",
        Matrix::Bt709 => "bt709",
        Matrix::Bt2020 => "bt2020",
    };
    let in_range = if src.full_range { "full" } else { "limited" };
    let none = || Conversion {
        flags: RESAMPLE,
        scale: String::new(),
        after: String::new(),
    };
    // swscale only scales YUV that `zscale` converts, or that stays as it
    // is: told the matrix and range on both sides, it keeps them whatever
    // the graph's links say.
    let keep = if src.rgb {
        String::new()
    } else {
        format!(
            ":in_color_matrix={in_matrix}:in_range={in_range}\
             :out_color_matrix={in_matrix}:out_range={in_range}"
        )
    };
    match (src.hdr, target) {
        // SDR to SDR: swscale converts the matrix and range as it scales.
        (None, None) if src.rgb && rgb_out => none(),
        (None, None) if src.rgb => Conversion {
            flags: CONVERT,
            scale: ":out_color_matrix=bt709:out_range=limited".into(),
            after: String::new(),
        },
        (None, None) if rgb_out => Conversion {
            flags: CONVERT,
            scale: format!(
                ":in_color_matrix={in_matrix}:in_range={in_range},format={}",
                name(out)
            ),
            after: String::new(),
        },
        (None, None) => Conversion {
            flags: if src.matrix == Matrix::Bt709 && !src.full_range {
                RESAMPLE
            } else {
                CONVERT
            },
            scale: format!(
                ":in_color_matrix={in_matrix}:in_range={in_range}:out_color_matrix=bt709:out_range=limited"
            ),
            after: String::new(),
        },
        // The same HDR transfer: nothing to convert.
        (Some(s), Some(t)) if s == t && !src.rgb && src.matrix == Matrix::Bt2020 => Conversion {
            flags: RESAMPLE,
            scale: keep,
            after: String::new(),
        },
        // HDR into SDR: linear light with the reference white at 1.0, BT.709
        // primaries, the tone curve, BT.709's transfer.
        (Some(_), None) => {
            let back = if rgb_out {
                "zscale=t=709:p=709:r=full,format=gbrp16le".to_string()
            } else {
                format!("zscale=t=709:m=709:r=limited{}", zscale_writes(out))
            };
            Conversion {
                flags: RESAMPLE,
                scale: keep,
                after: format!(
                    ",zscale={},format=gbrpf32le,zscale=p=709,\
                     tonemap=tonemap=mobius:param=0.8:desat=0:peak={:.4},{back}",
                    src.to_linear(),
                    PEAK / REFERENCE_WHITE
                ),
            }
        }
        // Into HDR: linear light with SDR white at the reference white.
        (_, Some(t)) => Conversion {
            flags: RESAMPLE,
            scale: keep,
            after: format!(
                ",zscale={},format=gbrpf32le,\
                 zscale=p=2020:t={}:m=2020_ncl:r=limited:npl={REFERENCE_WHITE}{}",
                src.to_linear(),
                transfer(Some(t)),
                zscale_writes(out)
            ),
        },
    }
}

/// The format `zscale` is to write YUV for `out` in: NV12 and P010 (frames
/// for NVENC), which it can't write, as their planar layouts, which swscale
/// only interleaves. Left to choose, it wrote RGB, and swscale's conversion
/// to YUV came out over half a level brighter than zimg's.
fn zscale_writes(out: format::Pixel) -> &'static str {
    match out {
        format::Pixel::NV12 => ",format=yuv420p",
        format::Pixel::P010LE => ",format=yuv420p10le",
        _ => "",
    }
}

/// Whether [`conversion`] takes `zscale` from `src` into `target`'s
/// dynamic range: a change of transfer or primaries.
pub(crate) fn needs_zscale(src: Source, target: Option<Hdr>) -> bool {
    match (src.hdr, target) {
        (None, None) => false,
        (Some(s), Some(t)) => !(s == t && !src.rgb && src.matrix == Matrix::Bt2020),
        _ => true,
    }
}

/// [`conversion`] for an FFmpeg without `zscale` (zimg), slower: swscale
/// scales, pads and reads the picture as float RGB in its own transfer and
/// primaries (`before`, the `scale` and `after` of a [`Conversion`]); the
/// CPU does `zscale`'s math on it ([`CpuColors`]) and writes planar YUV as
/// zimg does ([`write_yuv`]); swscale repacks that, or writes the float RGB
/// as RGB or as YUV the CPU doesn't write (`back`, a graph of its own).
pub(crate) struct Split {
    pub before: Conversion,
    pub cpu: CpuColors,
    /// The planar YUV the CPU writes; `None`: `back` takes the float RGB.
    pub write: Option<format::Pixel>,
    /// The filters from there into the output's format; `None`: `write` is
    /// it.
    pub back: Option<String>,
}

pub(crate) fn split(src: Source, out: format::Pixel, target: Option<Hdr>) -> Split {
    let in_matrix = match src.matrix {
        Matrix::Bt601 => "bt601",
        Matrix::Bt709 => "bt709",
        Matrix::Bt2020 => "bt2020",
    };
    let in_range = if src.full_range { "full" } else { "limited" };
    // Scaled and padded in its own colors, as for `zscale`, then read as RGB.
    let (scale, to_rgb) = if src.rgb {
        (String::new(), String::new())
    } else {
        (
            format!(
                ":in_color_matrix={in_matrix}:in_range={in_range}\
                 :out_color_matrix={in_matrix}:out_range={in_range}"
            ),
            format!(",scale=flags={CONVERT}:in_color_matrix={in_matrix}:in_range={in_range}"),
        )
    };
    let (write, back) = if is_rgb(out) {
        (None, Some(format!("format=gbrp16le,format={}", name(out))))
    } else if let Some(planar) = planar_yuv(out) {
        (
            Some(planar),
            (planar != out).then(|| format!("format={}", name(out))),
        )
    } else {
        // swscale clips the RGB it reads at white, which zimg keeps above.
        let matrix = if target.is_some() { "bt2020" } else { "bt709" };
        let yuv = format!(
            "scale=flags={CONVERT}:out_color_matrix={matrix}:out_range=limited,format={}",
            name(out)
        );
        (None, Some(yuv))
    };
    Split {
        before: Conversion {
            flags: RESAMPLE,
            scale,
            after: format!("{to_rgb},format=gbrpf32le"),
        },
        cpu: CpuColors::new(src, target),
        write,
        back,
    }
}

/// The planar YUV [`write_yuv`] writes for `out`: itself, or for NV12 and
/// P010 the planar layout they interleave. `None` for others (alpha, big
/// endian, other subsampling).
fn planar_yuv(out: format::Pixel) -> Option<format::Pixel> {
    use format::Pixel as P;
    match out {
        P::NV12 => return Some(P::YUV420P),
        P::P010LE => return Some(P::YUV420P10LE),
        _ => {}
    }
    let d = out.descriptor()?;
    // SAFETY: FFmpeg's static descriptor of the format.
    let d = unsafe { &*d.as_ptr() };
    let flag = |f: std::ffi::c_int| d.flags & f as u64 != 0;
    let fits = flag(ffi::AV_PIX_FMT_FLAG_PLANAR)
        && !flag(ffi::AV_PIX_FMT_FLAG_RGB)
        && !flag(ffi::AV_PIX_FMT_FLAG_BE)
        && !flag(ffi::AV_PIX_FMT_FLAG_ALPHA)
        && d.nb_components == 3
        && (8..=16).contains(&d.comp[0].depth)
        && d.log2_chroma_w <= 1
        && d.log2_chroma_h <= 1;
    fits.then_some(out)
}

/// Writes `rgb`, planar float RGB (`gbrpf32le`) in an output's transfer and
/// primaries, into `out`'s planar YUV ([`planar_yuv`]) as zimg writes it:
/// `target`'s matrix (BT.709 for SDR, BT.2020 for HDR), video range, each
/// subsampled block's chroma from its average, codes rounded and clipped
/// only to the format's (light above white stays above it).
pub(crate) fn write_yuv(rgb: &frame::Video, out: &mut frame::Video, target: Option<Hdr>) {
    let (kr, kb) = if target.is_some() {
        (0.2627f32, 0.0593f32)
    } else {
        (0.2126, 0.0722)
    };
    let kg = 1.0 - kr - kb;
    let desc = out.format().descriptor().expect("a known format");
    // SAFETY: FFmpeg's static descriptor of the format.
    let (sx, sy, depth) = unsafe {
        let d = &*desc.as_ptr();
        (
            u32::from(d.log2_chroma_w),
            u32::from(d.log2_chroma_h),
            d.comp[0].depth,
        )
    };
    let (w, h) = (out.width() as usize, out.height() as usize);
    let (cw, ch) = (w.div_ceil(1 << sx), h.div_ceil(1 << sy));
    let unit = (1u32 << (depth - 8)) as f32;
    let max = ((1u32 << depth) - 1) as f32;
    let code = |base: f32, span: f32, v: f32| (unit * (base + span * v)).round().clamp(0.0, max);
    // The float planes: G, B, R.
    let read = |plane: usize, x: usize, y: usize| {
        let at = y * rgb.stride(plane) + 4 * x;
        let d = rgb.data(plane);
        f32::from_le_bytes([d[at], d[at + 1], d[at + 2], d[at + 3]])
    };
    let pixel = |x: usize, y: usize| [read(2, x, y), read(0, x, y), read(1, x, y)];
    let luma = |[r, g, b]: [f32; 3]| kr * r + kg * g + kb * b;
    let strides = [out.stride(0), out.stride(1), out.stride(2)];
    let wide = depth > 8;
    let put = |row: &mut [u8], x: usize, v: f32| {
        if wide {
            row[2 * x..2 * x + 2].copy_from_slice(&(v as u16).to_le_bytes());
        } else {
            row[x] = v as u8;
        }
    };
    // SAFETY: the three planes are apart in memory; each slice covers its
    // plane's rows, `stride` bytes apart.
    let [y_plane, u_plane, v_plane] = unsafe {
        let raw = out.as_mut_ptr();
        [(0, h), (1, ch), (2, ch)]
            .map(|(p, rows)| std::slice::from_raw_parts_mut((*raw).data[p], strides[p] * rows))
    };
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(ch);
    // Chroma rows per thread, and the luma rows they cover.
    let rows = ch.div_ceil(threads);
    std::thread::scope(|scope| {
        let chunks = y_plane
            .chunks_mut(strides[0] * (rows << sy))
            .zip(u_plane.chunks_mut(strides[1] * rows))
            .zip(v_plane.chunks_mut(strides[2] * rows))
            .enumerate();
        for (i, ((ys, us), vs)) in chunks {
            scope.spawn(move || {
                let first = i * rows;
                for (dy, row) in ys.chunks_mut(strides[0]).enumerate() {
                    let y = (first << sy) + dy;
                    for x in 0..w {
                        put(row, x, code(16.0, 219.0, luma(pixel(x, y))));
                    }
                }
                let lines = us.chunks_mut(strides[1]).zip(vs.chunks_mut(strides[2]));
                for (dy, (u_row, v_row)) in lines.enumerate() {
                    let cy = first + dy;
                    for cx in 0..cw {
                        let mut sum = [0f32; 3];
                        let mut n = 0.0;
                        for y in (cy << sy)..((cy + 1) << sy).min(h) {
                            for x in (cx << sx)..((cx + 1) << sx).min(w) {
                                let p = pixel(x, y);
                                (0..3).for_each(|k| sum[k] += p[k]);
                                n += 1.0;
                            }
                        }
                        let [r, g, b] = sum.map(|s| s / n);
                        let l = luma([r, g, b]);
                        put(u_row, cx, code(128.0, 224.0, (b - l) / (2.0 * (1.0 - kb))));
                        put(v_row, cx, code(128.0, 224.0, (r - l) / (2.0 * (1.0 - kr))));
                    }
                }
            });
        }
    });
}

/// `zscale`'s math, as the GPU's shader does it ([`shader`]), on the CPU:
/// float RGB in a source's transfer and primaries to linear light (the
/// reference white at 1.0), the output's primaries, the tone curve on the
/// brightest channel (HDR into SDR), the output's transfer.
#[derive(Debug, Clone)]
pub(crate) struct CpuColors {
    /// The source's transfer to linear light, and linear light to the
    /// output's.
    from: Curve,
    to: Curve,
    gamut: Option<[[f32; 3]; 3]>,
    /// The knee, then `scale`, `a` and `b` of the curve above it.
    tone: Option<[f32; 4]>,
}

impl CpuColors {
    pub(crate) fn new(src: Source, target: Option<Hdr>) -> Self {
        let from = Primaries::of(src);
        let to = if target.is_some() {
            Primaries::Bt2020
        } else {
            Primaries::Bt709
        };
        let (scale, a, b) = mobius();
        // Light up to the output's brightest: PQ's 10,000 nits, HLG's
        // 1,000-nit display; SDR's white, which the tone curve keeps it to.
        let top = match target {
            Some(Hdr::Pq) => 10000.0 / REFERENCE_WHITE as f32,
            Some(Hdr::Hlg) => 1000.0 / REFERENCE_WHITE as f32,
            None => 1.0,
        };
        Self {
            from: Curve::new(to_linear, src.hdr, 1.0, false),
            to: Curve::new(from_linear, target, top, true),
            gamut: (from != to).then(|| gamut(from, to).map(|row| row.map(|v| v as f32))),
            tone: (src.hdr.is_some() && target.is_none()).then_some([
                KNEE as f32,
                scale as f32,
                a as f32,
                b as f32,
            ]),
        }
    }

    /// One pixel's RGB.
    fn pixel(&self, rgb: [f32; 3]) -> [f32; 3] {
        let mut c = rgb.map(|v| self.from.at(v));
        if let Some(m) = self.gamut {
            c = m.map(|row| row[0] * c[0] + row[1] * c[1] + row[2] * c[2]);
        }
        if let Some([knee, scale, a, b]) = self.tone {
            let sig = c[0].max(c[1]).max(c[2]).max(1e-6);
            let mapped = if sig <= knee {
                sig
            } else {
                scale * (sig + a) / (sig + b)
            };
            c = c.map(|v| v * mapped / sig);
        }
        c.map(|v| self.to.at(v))
    }

    /// Converts `f`, planar float RGB (`gbrpf32le`), in place, its rows
    /// shared among the CPU's cores.
    pub(crate) fn run(&self, f: &mut frame::Video) {
        assert_eq!(f.format(), format::Pixel::GBRPF32LE);
        let (width, height) = (f.width() as usize, f.height() as usize);
        if width == 0 || height == 0 {
            return;
        }
        let strides = [f.stride(0), f.stride(1), f.stride(2)];
        let threads = std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .min(height);
        let rows = height.div_ceil(threads);
        // SAFETY: the three planes are apart in memory; each slice covers
        // its plane's rows, `stride` bytes apart.
        let planes = unsafe {
            let raw = f.as_mut_ptr();
            [0, 1, 2].map(|p| std::slice::from_raw_parts_mut((*raw).data[p], strides[p] * height))
        };
        let [g, b, r] = planes;
        std::thread::scope(|scope| {
            let chunks = g
                .chunks_mut(strides[0] * rows)
                .zip(b.chunks_mut(strides[1] * rows))
                .zip(r.chunks_mut(strides[2] * rows));
            for ((g, b), r) in chunks {
                scope.spawn(move || {
                    let lines = g
                        .chunks_mut(strides[0])
                        .zip(b.chunks_mut(strides[1]))
                        .zip(r.chunks_mut(strides[2]));
                    for ((g, b), r) in lines {
                        let samples = g[..4 * width]
                            .as_chunks_mut::<4>()
                            .0
                            .iter_mut()
                            .zip(b[..4 * width].as_chunks_mut::<4>().0)
                            .zip(r[..4 * width].as_chunks_mut::<4>().0);
                        for ((g, b), r) in samples {
                            let rgb = [*r, *g, *b].map(f32::from_le_bytes);
                            let [rv, gv, bv] = self.pixel(rgb).map(f32::to_le_bytes);
                            (*r, *g, *b) = (rv, gv, bv);
                        }
                    }
                });
            }
        });
    }
}

/// A transfer function `f` for `hdr`, as a table of its values from 0 to
/// `top` in [`Curve::STEPS`] steps, interpolated: of the input, or of its
/// fourth root (`root`) where `f` is steep near black. Computed outside it.
#[derive(Clone)]
struct Curve {
    f: fn(Option<Hdr>, f32) -> f32,
    hdr: Option<Hdr>,
    top: f32,
    root: bool,
    values: Vec<f32>,
}

impl std::fmt::Debug for Curve {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "Curve({:?} to {})", self.hdr, self.top)
    }
}

impl Curve {
    const STEPS: usize = 4096;

    fn new(f: fn(Option<Hdr>, f32) -> f32, hdr: Option<Hdr>, top: f32, root: bool) -> Self {
        let values = (0..=Self::STEPS)
            .map(|i| {
                let u = i as f32 / Self::STEPS as f32;
                f(hdr, top * if root { u * u * u * u } else { u })
            })
            .collect();
        Self {
            f,
            hdr,
            top,
            root,
            values,
        }
    }

    fn at(&self, x: f32) -> f32 {
        // Below 0, above the top and NaN: computed.
        if !(x >= 0.0 && x < self.top) {
            return (self.f)(self.hdr, x);
        }
        let u = x / self.top;
        let u = if self.root { u.sqrt().sqrt() } else { u };
        let pos = u * Self::STEPS as f32;
        let i = (pos as usize).min(Self::STEPS - 1);
        let t = pos - i as f32;
        self.values[i] + t * (self.values[i + 1] - self.values[i])
    }
}

/// `v` in `hdr`'s transfer to linear light, the reference white at 1.0
/// ([`glsl_to_linear`]).
fn to_linear(hdr: Option<Hdr>, v: f32) -> f32 {
    let v = v.max(0.0);
    match hdr {
        Some(Hdr::Pq) => {
            let p = v.powf(1.0 / 78.843_75);
            let num = (p - 0.835_937_5).max(0.0);
            let den = (18.851_563 - 18.6875 * p).max(f32::MIN_POSITIVE);
            (num / den).powf(1.0 / 0.159_301_76) * (10000.0 / REFERENCE_WHITE as f32)
        }
        Some(Hdr::Hlg) => {
            let l = if v <= 0.5 {
                v * v / 3.0
            } else {
                (((v - 0.559_910_7) / 0.178_832_77).exp() + 0.284_668_92) / 12.0
            };
            l.powf(1.2) * (1000.0 / REFERENCE_WHITE as f32)
        }
        None => v.powf(2.4),
    }
}

/// Linear light `v` (the reference white at 1.0) to `hdr`'s transfer
/// ([`glsl_from_linear`]).
fn from_linear(hdr: Option<Hdr>, v: f32) -> f32 {
    match hdr {
        Some(Hdr::Pq) => {
            let l = v * (REFERENCE_WHITE as f32 / 10000.0);
            if l <= 0.0 {
                return 0.0;
            }
            let p = l.powf(0.159_301_76);
            ((0.835_937_5 + 18.851_563 * p) / (1.0 + 18.6875 * p)).powf(78.843_75)
        }
        Some(Hdr::Hlg) => {
            let l = (v * (REFERENCE_WHITE as f32 / 1000.0))
                .max(0.0)
                .powf(1.0 / 1.2);
            if l <= 1.0 / 12.0 {
                (3.0 * l).sqrt()
            } else {
                0.178_832_77 * (12.0 * l - 0.284_668_92).max(1e-30).ln() + 0.559_910_7
            }
        }
        None => v.max(0.0).powf(1.0 / 2.4),
    }
}

fn name(p: format::Pixel) -> String {
    p.descriptor()
        .map(|d| d.name().to_string())
        .unwrap_or_default()
}

/// Tags an encoder's output as SDR BT.709 (`None`) or BT.2100 with `hdr`'s
/// transfer; limited range.
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

    /// A 4:2:0 picture of `w`×`h` in `hdr`'s transfer (`None`: untagged).
    fn picture(w: u32, h: u32, hdr: Option<Hdr>) -> frame::Video {
        use ffi::AVColorTransferCharacteristic as T;
        let mut f = frame::Video::new(format::Pixel::YUV420P10LE, w, h);
        unsafe {
            (*f.as_mut_ptr()).color_trc = match hdr {
                Some(Hdr::Pq) => T::AVCOL_TRC_SMPTE2084,
                Some(Hdr::Hlg) => T::AVCOL_TRC_ARIB_STD_B67,
                None => T::AVCOL_TRC_UNSPECIFIED,
            };
        }
        f
    }

    /// A picture shrunk from an untagged one is read with the matrix the
    /// untagged one is read with (its size says which), not the one its
    /// own size would say; a tagged picture's tag goes along.
    #[test]
    fn shrunk_pictures_keep_their_sources_matrix() {
        use ffi::AVColorSpace as S;
        let shrunk = |w, h, tag| {
            let mut f = picture(w, h, None);
            unsafe { (*f.as_mut_ptr()).colorspace = tag };
            let mut small = picture(320, 180, None);
            keep_matrix(&mut small, &f);
            Source::of(&small).matrix
        };
        // Untagged: HD is BT.709, SD BT.601, though 320x180 alone is SD.
        assert_eq!(shrunk(1920, 1080, S::AVCOL_SPC_UNSPECIFIED), Matrix::Bt709);
        assert_eq!(shrunk(720, 480, S::AVCOL_SPC_UNSPECIFIED), Matrix::Bt601);
        // Tagged, whatever the size.
        assert_eq!(shrunk(720, 480, S::AVCOL_SPC_BT709), Matrix::Bt709);
        assert_eq!(shrunk(3840, 2160, S::AVCOL_SPC_BT2020_NCL), Matrix::Bt2020);
        // Untouched, the small one alone would be read as SD.
        assert_eq!(Source::of(&picture(320, 180, None)).matrix, Matrix::Bt601);
    }

    #[test]
    fn the_gamut_matrix_is_bt2020_to_bt709s() {
        let m = gamut(Primaries::Bt2020, Primaries::Bt709);
        let want = [
            [1.6605, -0.5876, -0.0728],
            [-0.1246, 1.1329, -0.0083],
            [-0.0182, -0.1006, 1.1187],
        ];
        for r in 0..3 {
            // White stays white.
            assert!((m[r].iter().sum::<f64>() - 1.0).abs() < 1e-9);
            for c in 0..3 {
                assert!((m[r][c] - want[r][c]).abs() < 1e-4, "{m:?}");
            }
        }
    }

    #[test]
    fn untagged_pictures_are_guessed_as_players_guess() {
        for (w, h, hd) in [
            (1280, 534, true), // a widescreen film in HD
            (1920, 800, true),
            (1920, 1080, true),
            (720, 1280, true), // portrait HD
            (720, 576, false), // PAL
            (720, 480, false), // NTSC
            (1024, 576, false),
            (640, 360, false),
        ] {
            let want = if hd { Matrix::Bt709 } else { Matrix::Bt601 };
            assert_eq!(untagged_matrix(w, h), want, "{w}x{h}");
        }
    }
}
