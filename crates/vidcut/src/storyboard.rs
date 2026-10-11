//! Storyboards (`storyboard.render`): one PNG with a labeled picture of
//! every shot of the program, or of every N frames, so a whole edit can be
//! looked at at once (by an agent as much as a person).

use std::path::Path;

use serde_json::{Value, json};
use vidcut_core::model::{Project, TrackKind};
use vidcut_core::render::{self, VideoPiece};
use vidcut_core::time::{RangeSpec, TimeSpec, format_timecode};
use vidcut_core::{Error, ErrorCode, Result};

/// The most pictures one storyboard holds.
const MAX_CELLS: usize = 200;
const PAD: usize = 8;
/// Label glyphs are 5×7 pixels drawn at this scale.
const SCALE: usize = 2;
const LABEL: usize = 7 * SCALE + 6;
const BACKGROUND: [u8; 4] = [28, 28, 30, 255];
const INK: [u8; 4] = [225, 225, 230, 255];

struct Cell {
    frame: i64,
    piece: VideoPiece,
}

/// Renders the storyboard of `p` into `out` (PNG): `columns` pictures per
/// row, each `width` wide. One picture per shot (the middle frame of each
/// stretch from one source), or one every `every` frames.
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
    let at = |piece: &VideoPiece, k: i64| match piece {
        VideoPiece::Source {
            media,
            path,
            origin,
            source_frame,
            ..
        } => VideoPiece::Source {
            media: media.clone(),
            path: path.clone(),
            origin: *origin,
            source_frame: source_frame + k,
            frames: 1,
        },
        VideoPiece::Black { .. } => VideoPiece::Black { frames: 1 },
    };
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
        let tc = format_timecode(cell.frame, p.settings.fps, p.settings.start_tc);
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
        .map_err(|e| Error::new(ErrorCode::Export, e.to_string()))?;
    let part = out.with_extension("part.png");
    std::fs::write(&part, png)
        .and_then(|()| std::fs::rename(&part, out))
        .map_err(|e| Error::new(ErrorCode::Export, format!("{}: {e}", out.display())))?;
    Ok(json!({
        "path": out, "width": cw, "height": ch, "columns": columns,
        "from": a, "to": b, "cells": out_cells,
    }))
}

/// The cells' pictures, rendered on a few threads (each opens its source).
fn pictures(cells: &[Cell], p: &Project, w: usize, h: usize) -> Result<Vec<Vec<u8>>> {
    let fps = p.settings.fps;
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get().min(6));
    let chunk = cells.len().div_ceil(threads);
    std::thread::scope(|s| {
        let handles: Vec<_> = cells
            .chunks(chunk)
            .map(|part| {
                s.spawn(move || {
                    part.iter()
                        .map(|c| {
                            vidcut_av::export::still::rgba(&c.piece, fps, w as u32, h as u32)
                                .map(|image| image.rgba)
                        })
                        .collect::<vidcut_av::error::Result<Vec<_>>>()
                })
            })
            .collect();
        let mut all = Vec::with_capacity(cells.len());
        for handle in handles {
            let part = handle
                .join()
                .map_err(|_| Error::internal("a storyboard picture panicked"))?
                .map_err(|e| Error::new(ErrorCode::Export, e.to_string()))?;
            all.extend(part);
        }
        Ok(all)
    })
}

/// The clip whose picture shows at `frame`, and its track: the highest
/// visible one (top-wins, as export renders).
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

/// Rows of a 5×7 glyph, high bit on the left; `None` for characters the
/// labels never use.
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

/// Draws `s` at (`x`, `y`) on an RGBA canvas `cw` pixels wide, at most
/// `max_w` pixels long.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_draw_known_glyphs_and_skip_others() {
        let cw = 200;
        let mut canvas = BACKGROUND.repeat(cw * 30);
        text(&mut canvas, cw, 0, 0, "1x", 100);
        let ink = canvas.chunks(4).filter(|p| *p == INK).count();
        // "1" has 10 lit pixels, each 2×2; "x" isn't in the font.
        assert_eq!(ink, 10 * SCALE * SCALE);
        let mut clipped = BACKGROUND.repeat(cw * 30);
        text(&mut clipped, cw, 0, 0, "1111", 20);
        assert_eq!(
            clipped.chunks(4).filter(|p| *p == INK).count(),
            10 * SCALE * SCALE,
            "only one glyph fits in 20 px"
        );
    }
}
