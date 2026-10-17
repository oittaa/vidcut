use std::collections::BTreeSet;
use std::sync::Mutex;
use std::time::Instant;

use super::*;

const SIZE: (u32, u32) = (128, 64);

const BLACK: [u16; 3] = [16, 128, 128];
const BLOCK: [u16; 3] = [235, 240, 16];

static VERDICTS: Mutex<Vec<(&str, format::Pixel, bool)>> = Mutex::new(Vec::new());

pub(crate) fn gives_bwdifs(f: &frame::Video) -> bool {
    let filter = crate::decode::deinterlacer(f.format());
    let layout = sw_format(f);
    let mut verdicts = VERDICTS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&(.., given)) = verdicts.iter().find(|v| (v.0, v.1) == (filter, layout)) {
        return given;
    }
    let started = Instant::now();
    let checked = off(f);
    let took = started.elapsed().as_millis();
    let given = match checked {
        Ok(off) if off.iter().all(BTreeSet::is_empty) => {
            crate::debug!(
                "decoding",
                "{filter} gives bwdif's {layout:?} frames: they are deinterlaced on the GPU \
                 (checked in {took} ms)"
            );
            true
        }
        Ok(off) => {
            crate::debug!(
                "decoding",
                "{filter} doesn't give bwdif's {layout:?} frames, in {}: they come down to be \
                 deinterlaced (checked in {took} ms)",
                rows(&off)
            );
            false
        }
        Err(e) => {
            crate::debug!(
                "decoding",
                "{layout:?} frames come down to be deinterlaced: {filter} couldn't be checked: {e}"
            );
            false
        }
    };
    verdicts.push((filter, layout, given));
    given
}

fn off(f: &frame::Video) -> Result<Vec<BTreeSet<usize>>> {
    let (frames, _) = frames_of(f)?;
    let layout = sw_format(f);
    // SAFETY: the frame's own frames context, alive as long as it.
    let like = unsafe { frames_like(frames, SIZE)? };
    let time_base = ffmpeg::Rational::new(1, 25);
    let mut off = vec![BTreeSet::new(); 4];
    for top_first in [true, false] {
        let Some(pictures) = (0..3)
            .map(|k| test_frame(layout, k, top_first))
            .collect::<Option<Vec<_>>>()
        else {
            return Err(Error::Invalid(format!("no test frames come in {layout:?}")));
        };
        let uploaded = pictures
            .iter()
            .map(|p| upload(&like, p))
            .collect::<Result<Vec<_>>>()?;
        let gpu = through(
            crate::decode::deinterlacing(&uploaded[0], time_base, crate::decode::Reading::Stream)?,
            uploaded,
        )?;
        let cpu = through(
            crate::decode::deinterlacing(&pictures[0], time_base, crate::decode::Reading::Stream)?,
            pictures,
        )?;
        if (gpu.len(), cpu.len()) != (3, 3) {
            return Err(Error::Invalid(format!(
                "it gave {} frames and bwdif {} for 3",
                gpu.len(),
                cpu.len()
            )));
        }
        for (g, c) in gpu.iter().zip(&cpu) {
            let g = downloaded(g)?;
            let rows = rows_off(c, &g).ok_or_else(|| {
                Error::Invalid(format!(
                    "its {:?} frames can't be compared with bwdif's {:?}",
                    g.format(),
                    c.format()
                ))
            })?;
            for (component, row) in rows {
                off[component].insert(row);
            }
        }
    }
    Ok(off)
}

unsafe fn frames_like(frames: *mut ffi::AVBufferRef, (w, h): (u32, u32)) -> Result<Frames> {
    // SAFETY: `frames` is live; `like`, made on its device, has a `hwctx` of the same kind.
    unsafe {
        let from = &*((*frames).data as *const ffi::AVHWFramesContext);
        let mut like = ffi::av_hwframe_ctx_alloc(from.device_ref);
        if like.is_null() {
            return Err(Error::Invalid("no memory for GPU frames".into()));
        }
        let to = &mut *((*like).data as *mut ffi::AVHWFramesContext);
        to.format = from.format;
        to.sw_format = from.sw_format;
        to.width = w as i32;
        to.height = h as i32;
        if from.format == ffi::AVPixelFormat::AV_PIX_FMT_VULKAN {
            let from = &*(from.hwctx as *const ffi::AVVulkanFramesContext);
            let to = &mut *(to.hwctx as *mut ffi::AVVulkanFramesContext);
            to.tiling = from.tiling;
            to.usage = from.usage;
            to.flags = from.flags;
            to.format = from.format;
            to.img_flags = from.img_flags;
            to.nb_layers = from.nb_layers;
        }
        let ret = ffi::av_hwframe_ctx_init(like);
        if ret < 0 {
            ffi::av_buffer_unref(&mut like);
            return Err(Error::Ffmpeg {
                context: "setting up GPU frames to test with".into(),
                source: ffmpeg::Error::from(ret),
            });
        }
        Ok(Frames(like))
    }
}

fn upload(frames: &Frames, picture: &frame::Video) -> Result<frame::Video> {
    let mut f = frame::Video::empty();
    // SAFETY: `f` takes a buffer of `frames`' pool, which is the picture's size and layout.
    unsafe {
        let ret = ffi::av_hwframe_get_buffer(frames.0, f.as_mut_ptr(), 0);
        if ret < 0 {
            return Err(Error::Ffmpeg {
                context: "getting a GPU frame to test with".into(),
                source: ffmpeg::Error::from(ret),
            });
        }
        let ret = ffi::av_hwframe_transfer_data(f.as_mut_ptr(), picture.as_ptr(), 0);
        if ret < 0 {
            return Err(Error::Ffmpeg {
                context: "uploading a test frame".into(),
                source: ffmpeg::Error::from(ret),
            });
        }
    }
    props(&mut f, picture)?;
    Ok(f)
}

fn through(mut graph: GpuGraph, frames: Vec<frame::Video>) -> Result<Vec<frame::Video>> {
    let failed = |source| Error::Ffmpeg {
        context: "deinterlacing a test frame".into(),
        source,
    };
    {
        let mut input = graph.get("in").expect("source");
        let mut source = input.source();
        for f in &frames {
            source.add(f).map_err(failed)?;
        }
        source.flush().map_err(failed)?;
    }
    let mut output = graph.get("out").expect("sink");
    let mut sink = output.sink();
    let mut out = Vec::new();
    loop {
        let mut f = frame::Video::empty();
        match sink.frame(&mut f) {
            Ok(()) => out.push(f),
            Err(ffmpeg::Error::Eof) => return Ok(out),
            Err(e) => return Err(failed(e)),
        }
    }
}

fn test_frame(layout: format::Pixel, k: usize, top_first: bool) -> Option<frame::Video> {
    let desc = described(layout)?;
    let mut f = blank(layout, SIZE);
    for c in 0..i32::from(desc.nb_components) {
        let (w, h) = component_size(desc, c, SIZE);
        for y in 0..h {
            let t = 2 * k + usize::from((y % 2 == 1) == top_first);
            let row = y * SIZE.1 as usize / h;
            let line: Vec<u16> = (0..w)
                .map(|x| {
                    let x = x * SIZE.0 as usize / w;
                    let top = row < 16 && (8 + 12 * t..40 + 12 * t).contains(&x);
                    let bottom = row >= 48 && (88 - 12 * t..120 - 12 * t).contains(&x);
                    level(desc, c, if top || bottom { BLOCK } else { BLACK })
                })
                .collect();
            write_line(&mut f, c, y, &line);
        }
    }
    // SAFETY: plain fields of its own frame.
    unsafe {
        let raw = f.as_mut_ptr();
        (*raw).flags |= ffi::AV_FRAME_FLAG_INTERLACED;
        if top_first {
            (*raw).flags |= ffi::AV_FRAME_FLAG_TOP_FIELD_FIRST;
        }
    }
    f.set_pts(Some(k as i64));
    Some(f)
}

fn rows_off(cpu: &frame::Video, gpu: &frame::Video) -> Option<Vec<(usize, usize)>> {
    let little = |f: &frame::Video| {
        described(f.format()).filter(|d| d.flags & ffi::AV_PIX_FMT_FLAG_BE as u64 == 0)
    };
    let (dc, dg) = (little(cpu)?, little(gpu)?);
    let size = (gpu.width(), gpu.height());
    if (cpu.width(), cpu.height()) != size || dc.nb_components != dg.nb_components {
        return None;
    }
    let mut off = Vec::new();
    for c in 0..usize::from(dg.nb_components) {
        let (w, h) = component_size(dg, c as i32, size);
        if component_size(dc, c as i32, size) != (w, h) {
            return None;
        }
        let depth = dg.comp[c].depth as u32;
        let (level, top) = (1 << (depth - 8), (1 << depth) - 1);
        for y in 0..h {
            let (want, got) = (samples(cpu, dc, c, y, w), samples(gpu, dg, c, y, w));
            if want
                .iter()
                .zip(&got)
                .any(|(&a, &b)| b > top || a.abs_diff(b) > level)
            {
                off.push((c, y));
            }
        }
    }
    Some(off)
}

fn samples(
    f: &frame::Video,
    desc: &ffi::AVPixFmtDescriptor,
    c: usize,
    y: usize,
    w: usize,
) -> Vec<u32> {
    let comp = desc.comp[c];
    let plane = comp.plane as usize;
    let (data, stride) = (f.data(plane), f.stride(plane));
    let wide = comp.shift + comp.depth > 8;
    (0..w)
        .map(|x| {
            let i = y * stride + comp.offset as usize + x * comp.step as usize;
            let raw = if wide {
                u16::from_le_bytes([data[i], data[i + 1]])
            } else {
                u16::from(data[i])
            };
            u32::from(raw >> comp.shift)
        })
        .collect()
}

fn rows(off: &[BTreeSet<usize>]) -> String {
    let names = ["Y", "Cb", "Cr", "alpha"];
    let mut parts = Vec::new();
    for (rows, name) in off.iter().zip(names) {
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        for &r in rows {
            match ranges.last_mut() {
                Some((_, end)) if *end + 1 == r => *end = r,
                _ => ranges.push((r, r)),
            }
        }
        if ranges.is_empty() {
            continue;
        }
        let ranges: Vec<String> = ranges
            .iter()
            .map(|&(a, b)| {
                if a == b {
                    a.to_string()
                } else {
                    format!("{a}-{b}")
                }
            })
            .collect();
        parts.push(format!("{name} rows {}", ranges.join(", ")));
    }
    parts.join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_frame_is_bwdifs() {
        let picture = test_frame(format::Pixel::YUV422P10LE, 0, true).unwrap();
        assert_eq!(rows_off(&picture, &picture), Some(vec![]));
    }

    #[test]
    fn nv12s_chroma_is_compared_with_planar_chroma() {
        let cpu = test_frame(format::Pixel::YUV420P, 1, false).unwrap();
        let gpu = test_frame(format::Pixel::NV12, 1, false).unwrap();
        assert_eq!(rows_off(&cpu, &gpu), Some(vec![]));
    }

    #[test]
    fn an_edge_row_a_level_off_is_bwdifs() {
        let cpu = test_frame(format::Pixel::NV12, 0, true).unwrap();
        let mut gpu = cpu.clone();
        let stride = gpu.stride(0);
        gpu.data_mut(0)[63 * stride + 100] = 236;
        assert_eq!(rows_off(&cpu, &gpu), Some(vec![]));
    }

    #[test]
    fn an_edge_row_two_levels_off_isnt_bwdifs() {
        let cpu = test_frame(format::Pixel::NV12, 0, true).unwrap();
        let mut gpu = cpu.clone();
        let stride = gpu.stride(0);
        gpu.data_mut(0)[63 * stride + 100] = 237;
        assert_eq!(rows_off(&cpu, &gpu), Some(vec![(0, 63)]));
    }

    #[test]
    fn a_sample_above_the_bits_isnt_bwdifs() {
        let mut cpu = test_frame(format::Pixel::YUV422P10LE, 0, true).unwrap();
        let mut gpu = cpu.clone();
        cpu.data_mut(0)[20..22].copy_from_slice(&1023u16.to_le_bytes());
        gpu.data_mut(0)[20..22].copy_from_slice(&1024u16.to_le_bytes());
        assert_eq!(rows_off(&cpu, &gpu), Some(vec![(0, 0)]));
    }
}
