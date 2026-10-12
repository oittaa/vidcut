//! Color: SDR (BT.709) and HDR (BT.2100: PQ and HLG) pictures, the filters
//! that convert between them, and the tags an encoder writes.
//!
//! HDR into SDR is tone-mapped. SDR into HDR puts SDR white at 203 nits,
//! the reference white of ITU-R BT.2408. Untagged pictures are BT.709 when HD by mpv's rule (1280 wide,
//! or over 576 lines), else BT.601: mpv shows the preview, so the export
//! then matches it. On the GPU, libplacebo converts with the same math
//! ([`gpu_color`]).

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

/// How `f`'s samples are read (see [`Colors`]).
pub fn colors_of(f: &frame::Video) -> Colors {
    let s = Source::of(f);
    Colors {
        matrix: s.matrix,
        full_range: s.full_range,
        hdr: s.hdr,
    }
}

fn is_rgb(p: format::Pixel) -> bool {
    p.descriptor()
        .is_some_and(|d| unsafe { (*d.as_ptr()).flags } & ffi::AV_PIX_FMT_FLAG_RGB as u64 != 0)
}

/// The matrix an untagged SDR picture of `width`×`height` is taken to use:
/// mpv's guess (BT.709 from 1280 wide or over 576 lines, else BT.601), so
/// the export decodes it as the preview shows it. A 1280×534 widescreen
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
    use ffi::AVColorSpace as S;
    let src = Source::of(f);
    if src.rgb {
        return None;
    }
    let output = Tags::output(target);
    let matrix = match src.matrix {
        Matrix::Bt601 => S::AVCOL_SPC_SMPTE170M,
        Matrix::Bt709 => S::AVCOL_SPC_BT709,
        Matrix::Bt2020 => S::AVCOL_SPC_BT2020_NCL,
    };
    let range = if src.full_range {
        ffi::AVColorRange::AVCOL_RANGE_JPEG
    } else {
        ffi::AVColorRange::AVCOL_RANGE_MPEG
    };
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

/// The matrix taking linear RGB in `from` primaries to `to`.
fn gamut(from: Primaries, to: Primaries) -> [[f64; 3]; 3] {
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

/// How a normalizer brings a source to its output: options for the `scale`
/// filter, and filters to run after padding (which happens in the source's
/// format), before the final `format`.
pub(crate) struct Conversion {
    pub scale: String,
    pub after: String,
}

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
        scale: String::new(),
        after: String::new(),
    };
    // swscale only scales YUV that `zscale` converts, or that stays as it
    // is: told nothing, it converts the matrix to BT.601 (the graph's
    // source carries no colorspace), and BT.2020 colors came out wrong.
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
            scale: ":out_color_matrix=bt709:out_range=limited".into(),
            after: String::new(),
        },
        (None, None) if rgb_out => Conversion {
            scale: format!(
                ":in_color_matrix={in_matrix}:in_range={in_range},format={}",
                name(out)
            ),
            after: String::new(),
        },
        (None, None) => Conversion {
            scale: format!(
                ":in_color_matrix={in_matrix}:in_range={in_range}:out_color_matrix=bt709:out_range=limited"
            ),
            after: String::new(),
        },
        // The same HDR transfer: nothing to convert.
        (Some(s), Some(t)) if s == t && !src.rgb && src.matrix == Matrix::Bt2020 => Conversion {
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
    fn hdr_is_scaled_in_its_own_matrix_and_zscale_writes_the_yuv() {
        use format::Pixel as P;
        // swscale keeps BT.2020 for zscale to read; zscale writes NV12's
        // planar layout.
        let pq = Source::of(&picture(3840, 2160, Some(Hdr::Pq)));
        let c = conversion(pq, P::NV12, None);
        assert!(c.scale.contains(":in_color_matrix=bt2020:in_range=limited"));
        assert!(
            c.scale
                .contains(":out_color_matrix=bt2020:out_range=limited")
        );
        assert!(
            c.after
                .contains("tonemap=mobius:param=0.8:desat=0:peak=4.9261")
        );
        assert!(
            c.after
                .ends_with("zscale=t=709:m=709:r=limited,format=yuv420p")
        );
        // Kept as PQ: only scaled.
        let c = conversion(pq, P::P010LE, Some(Hdr::Pq));
        assert!(c.scale.contains(":out_color_matrix=bt2020"));
        assert_eq!(c.after, "");
        // SD into HLG for NVENC: BT.601 kept, P010's planar layout.
        let sd = Source::of(&picture(720, 480, None));
        let c = conversion(sd, P::P010LE, Some(Hdr::Hlg));
        assert!(c.scale.contains(":in_color_matrix=bt601"));
        assert!(c.scale.contains(":out_color_matrix=bt601"));
        assert!(c.after.contains("p=170m"));
        assert!(
            c.after
                .ends_with("t=arib-std-b67:m=2020_ncl:r=limited:npl=203,format=yuv420p10le")
        );
    }

    #[test]
    fn the_gpu_converts_with_the_cpus_math() {
        use ffi::AVColorPrimaries as P;
        use ffi::AVColorSpace as S;
        use ffi::AVColorTransferCharacteristic as T;
        // SD into SDR: libplacebo changes the matrix only.
        let sd = gpu_color(&picture(720, 480, None), None).unwrap();
        assert_eq!(sd.input.matrix, S::AVCOL_SPC_SMPTE170M);
        assert_eq!(sd.input.transfer, T::AVCOL_TRC_BT709);
        assert_eq!(sd.shader, None);
        assert!(!sd.unchanged());
        // HD into SDR and PQ into PQ: nothing to do.
        assert!(
            gpu_color(&picture(1920, 1080, None), None)
                .unwrap()
                .unchanged()
        );
        let kept = gpu_color(&picture(1920, 1080, Some(Hdr::Pq)), Some(Hdr::Pq)).unwrap();
        assert!(kept.unchanged());
        assert_eq!(kept.output.transfer, T::AVCOL_TRC_SMPTE2084);
        // PQ into SDR: read as BT.2020, said to be in BT.709 already, the
        // shader converting and tone-mapping.
        let pq = gpu_color(&picture(3840, 2160, Some(Hdr::Pq)), None).unwrap();
        assert_eq!(pq.input.matrix, S::AVCOL_SPC_BT2020_NCL);
        assert_eq!(pq.input.primaries, P::AVCOL_PRI_BT709);
        assert_eq!(pq.input.transfer, T::AVCOL_TRC_BT709);
        assert_eq!(pq.output, Tags::output(None));
        let shader = pq.shader.unwrap();
        assert!(shader.starts_with("//!HOOK SCALED\n//!BIND HOOKED\n"));
        assert!(shader.contains("78.84375"), "PQ's EOTF");
        assert!(shader.contains("c = mat3(1.66"), "BT.2020 to BT.709");
        assert!(shader.contains("sig <= 0.8000000000"), "the tone curve");
        assert!(shader.contains("vec3(1.0 / 2.4)"), "BT.709's transfer");
        // SDR into HLG: no tone curve.
        let hlg = gpu_color(&picture(1920, 1080, None), Some(Hdr::Hlg)).unwrap();
        assert_eq!(hlg.input.transfer, T::AVCOL_TRC_ARIB_STD_B67);
        let shader = hlg.shader.unwrap();
        assert!(!shader.contains("float sig"));
        assert!(shader.contains("vec3(2.4)") && shader.contains("0.17883277"));
    }

    #[test]
    fn untagged_pictures_are_guessed_as_mpv_guesses() {
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
