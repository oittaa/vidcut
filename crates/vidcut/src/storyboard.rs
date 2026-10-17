use std::path::Path;

use serde_json::{Value, json};
use vidcut_av::export::renderer::FrameReader;
use vidcut_core::model::{Project, TrackKind};
use vidcut_core::render::{self, VideoPiece};
use vidcut_core::time::{RangeSpec, TimeSpec};
use vidcut_core::{Error, ErrorCode, Result};

use crate::threads::Work;

const MAX_CELLS: usize = 200;
const PAD: usize = 8;
const SCALE: usize = 2;
const LABEL: usize = 7 * SCALE + 6;
const BACKGROUND: [u8; 4] = [28, 28, 30, 255];
const INK: [u8; 4] = [225, 225, 230, 255];

struct Cell {
    frame: i64,
    piece: VideoPiece,
}

pub fn render(
    p: &Project,
    out: &Path,
    columns: u32,
    width: u32,
    every: Option<&TimeSpec>,
    range: Option<&RangeSpec>,
    playhead: Option<i64>,
) -> Result<Value> {
    let ctx = p.time_context(playhead);
    let (a, b) = match range {
        Some(r) => ctx.range(r)?,
        None => (0, p.end()),
    };
    if b <= a {
        return Err(Error::invalid("the storyboard's range is empty"));
    }
    let plan = render::flatten(p, a, b)?;
    let at = |piece: &VideoPiece, k: i64| piece.part(k, 1);
    let mut cells = Vec::new();
    match every {
        None => {
            let mut x = a;
            for piece in &plan.video {
                let n = piece.frames();
                if matches!(piece, VideoPiece::Source { .. }) {
                    cells.push(Cell {
                        frame: x + n / 2,
                        piece: at(piece, n / 2),
                    });
                }
                x += n;
            }
        }
        Some(every) => {
            let step = ctx.duration(every)?;
            if step < 1 {
                return Err(Error::usage("`every` must be at least one frame"));
            }
            let mut x = a;
            for piece in &plan.video {
                let n = piece.frames();
                let mut t = x + (a - x).rem_euclid(step);
                while t < x + n {
                    cells.push(Cell {
                        frame: t,
                        piece: at(piece, t - x),
                    });
                    t += step;
                }
                x += n;
            }
        }
    }
    if cells.is_empty() {
        return Err(Error::invalid("nothing to show: the range has no pictures"));
    }
    if cells.len() > MAX_CELLS {
        return Err(Error::usage(format!(
            "{} pictures is more than {MAX_CELLS}; give `every` or a shorter `range`",
            cells.len()
        )));
    }
    let columns = (columns.max(1) as usize).min(cells.len());
    let w = (width.clamp(32, 1920) & !1) as usize;
    let h = ((w as f64 * f64::from(p.settings.height) / f64::from(p.settings.width).max(1.0))
        .round() as usize
        & !1)
        .max(2);
    let rows = cells.len().div_ceil(columns);
    let (cw, ch) = (
        columns * w + (columns + 1) * PAD,
        rows * (h + LABEL) + (rows + 1) * PAD,
    );
    let pictures = pictures(&cells, p, w, h)?;
    let mut canvas: Vec<u8> = BACKGROUND.repeat(cw * ch);
    let mut out_cells = Vec::new();
    for (i, (cell, rgba)) in cells.iter().zip(&pictures).enumerate() {
        let (x, y) = (
            PAD + (i % columns) * (w + PAD),
            PAD + (i / columns) * (h + LABEL + PAD),
        );
        for row in 0..h {
            let dst = ((y + row) * cw + x) * 4;
            canvas[dst..dst + w * 4].copy_from_slice(&rgba[row * w * 4..(row + 1) * w * 4]);
        }
        let tc = p.settings.timecode(cell.frame);
        let clip = shown_clip(p, cell.frame);
        let label = match &clip {
            Some((id, track)) if track != "V1" => format!("{tc} {id} {track}"),
            Some((id, _)) => format!("{tc} {id}"),
            None => tc.clone(),
        };
        text(&mut canvas, cw, x, y + h + 4, &label, w);
        out_cells.push(json!({
            "frame": cell.frame, "tc": tc,
            "clip": clip.as_ref().map(|c| &c.0), "track": clip.as_ref().map(|c| &c.1),
            "x": x, "y": y, "width": w, "height": h,
        }));
    }
    let png = vidcut_av::export::still::png_rgba(cw as u32, ch as u32, &canvas)
        .map_err(|e| crate::export::av_error(&e, ErrorCode::Export, String::new()))?;
    let part = out.with_extension("part.png");
    std::fs::write(&part, png)
        .and_then(|()| std::fs::rename(&part, out))
        .map_err(|e| Error::new(ErrorCode::Export, format!("{}: {e}", out.display())))?;
    Ok(json!({
        "path": out, "width": cw, "height": ch, "columns": columns,
        "from": a, "to": b, "cells": out_cells,
    }))
}

fn pictures(cells: &[Cell], p: &Project, w: usize, h: usize) -> Result<Vec<Vec<u8>>> {
    let fps = p.settings.fps;
    let decoding = crate::preferences::gpu_decoding();
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get().min(6));
    let chunk = cells.len().div_ceil(threads);
    let cancel = vidcut_av::cancel::current();
    std::thread::scope(|s| {
        let handles = cells
            .chunks(chunk)
            .enumerate()
            .map(|(i, part)| {
                let cancel = cancel.clone();
                let decoding = &decoding;
                let name = format!("vidcut-story-{i}");
                crate::threads::scoped(s, name, Work::current(), move || {
                    let _guard = cancel.map(vidcut_av::cancel::Guard::set);
                    let mut reader = match decoding.own_decoder() {
                        Some(gpu) => FrameReader::decoding_on(gpu),
                        None => FrameReader::default(),
                    };
                    part.iter()
                        .map(|c| {
                            reader
                                .rgba(&c.piece, fps, w as u32, h as u32)
                                .map(|image| image.rgba)
                                .map_err(from_av)
                        })
                        .collect::<Result<Vec<_>>>()
                })
            })
            .collect::<std::io::Result<Vec<_>>>()
            .map_err(|e| Error::internal(format!("starting a storyboard thread: {e}")))?;
        let mut all = Vec::with_capacity(cells.len());
        for handle in handles {
            let part = handle
                .join()
                .map_err(|_| Error::internal("a storyboard picture panicked"))??;
            all.extend(part);
        }
        Ok(all)
    })
}

fn from_av(e: vidcut_av::error::Error) -> Error {
    if vidcut_av::export::is_cancelled(&e) {
        Error::new(ErrorCode::Cancelled, "cancelled")
    } else {
        crate::export::av_error(&e, ErrorCode::Export, String::new())
    }
}

fn shown_clip(p: &Project, frame: i64) -> Option<(String, String)> {
    p.tracks
        .iter()
        .rev()
        .filter(|t| t.kind == TrackKind::Video && t.video_enabled)
        .find_map(|t| {
            t.clips
                .iter()
                .find(|c| c.video && c.enabled && c.start <= frame && frame < c.end())
                .map(|c| (c.id.clone(), t.id.clone()))
        })
}

fn glyph(c: char) -> Option<[u8; 7]> {
    Some(match c {
        '0' => [0x0e, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0e],
        '1' => [0x04, 0x0c, 0x04, 0x04, 0x04, 0x04, 0x0e],
        '2' => [0x0e, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1f],
        '3' => [0x1f, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0e],
        '4' => [0x02, 0x06, 0x0a, 0x12, 0x1f, 0x02, 0x02],
        '5' => [0x1f, 0x10, 0x1e, 0x01, 0x01, 0x11, 0x0e],
        '6' => [0x06, 0x08, 0x10, 0x1e, 0x11, 0x11, 0x0e],
        '7' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0e, 0x11, 0x11, 0x0e, 0x11, 0x11, 0x0e],
        '9' => [0x0e, 0x11, 0x11, 0x0f, 0x01, 0x02, 0x0c],
        ':' => [0x00, 0x0c, 0x0c, 0x00, 0x0c, 0x0c, 0x00],
        ';' => [0x00, 0x0c, 0x0c, 0x00, 0x0c, 0x04, 0x08],
        'c' => [0x00, 0x00, 0x0e, 0x10, 0x10, 0x11, 0x0e],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0a, 0x04],
        ' ' => [0; 7],
        _ => return None,
    })
}

fn text(canvas: &mut [u8], cw: usize, x: usize, y: usize, s: &str, max_w: usize) {
    let advance = 6 * SCALE;
    for (i, c) in s.chars().enumerate() {
        let gx = x + i * advance;
        if gx + 5 * SCALE > x + max_w {
            break;
        }
        let Some(rows) = glyph(c) else { continue };
        for (row, bits) in rows.iter().enumerate() {
            for col in 0..5 {
                if bits & (0x10 >> col) == 0 {
                    continue;
                }
                for dy in 0..SCALE {
                    for dx in 0..SCALE {
                        let px = ((y + row * SCALE + dy) * cw + gx + col * SCALE + dx) * 4;
                        if px + 4 <= canvas.len() {
                            canvas[px..px + 4].copy_from_slice(&INK);
                        }
                    }
                }
            }
        }
    }
}
