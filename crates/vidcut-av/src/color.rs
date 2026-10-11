//! Color: SDR (BT.709) and HDR (BT.2100: PQ and HLG) pictures, the filters
//! that convert between them, and the tags an encoder writes.
//!
//! HDR into SDR is tone-mapped. SDR into HDR puts SDR white at 203 nits,
//! the reference white of ITU-R BT.2408 (Resolve's "203 nits" option,
//! p. 238). Untagged pictures are BT.709 when HD by mpv's rule (1280 wide,
//! or over 576 lines), else BT.601: mpv shows the preview, so the export
//! then matches it.

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Matrix {
    Bt601,
    Bt709,
    Bt2020,
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
        (Some(s), Some(t)) if s == t && !src.rgb && src.matrix == Matrix::Bt2020 => none(),
        // HDR into SDR: linear light with the reference white at 1.0, BT.709
        // primaries, the tone curve, BT.709's transfer.
        (Some(_), None) => {
            let back = if rgb_out {
                "zscale=t=709:p=709:r=full,format=gbrp16le"
            } else {
                "zscale=t=709:m=709:r=limited"
            };
            Conversion {
                scale: String::new(),
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
            scale: String::new(),
            after: format!(
                ",zscale={},format=gbrpf32le,\
                 zscale=p=2020:t={}:m=2020_ncl:r=limited:npl={REFERENCE_WHITE}",
                src.to_linear(),
                transfer(Some(t))
            ),
        },
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
