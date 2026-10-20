use super::text::{self, Dialect};
use super::{Read, Timed, clock};
use crate::model::Place;

pub fn time(s: &str) -> Option<f64> {
    let s = s.trim();
    let (hms, frac) = match s.rfind([',', '.']) {
        Some(i) => (&s[..i], &s[i + 1..]),
        None => (s, ""),
    };
    let parts: Vec<&str> = hms.split(':').collect();
    let nums: Option<Vec<f64>> = parts
        .iter()
        .map(|p| {
            let p = p.trim();
            (!p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
                .then(|| p.parse::<f64>().ok())
                .flatten()
        })
        .collect();
    let nums = nums?;
    let secs = match nums.as_slice() {
        [h, m, s] => h * 3600.0 + m * 60.0 + s,
        [m, s] => m * 60.0 + s,
        _ => return None,
    };
    if !frac.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let frac = if frac.is_empty() {
        0.0
    } else {
        frac.parse::<f64>().ok()? / 10f64.powi(frac.len() as i32)
    };
    Some(secs + frac)
}

pub fn timing(line: &str) -> Option<(f64, f64)> {
    let (a, b) = line.split_once("-->")?;
    let b = b.split_whitespace().next()?;
    Some((time(a)?, time(b)?))
}

fn is_index(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty() && t.chars().all(|c| c.is_ascii_digit())
}

pub fn read(source: &str) -> Read {
    let lines: Vec<&str> = source.lines().collect();
    let mut cues = Vec::new();
    let mut warnings = Vec::new();
    let mut current: Option<(f64, f64, Vec<&str>)> = None;
    let mut i = 0;
    let finish = |c: Option<(f64, f64, Vec<&str>)>, cues: &mut Vec<Timed>| {
        if let Some((start, end, lines)) = c {
            let (body, place) = text::markup(&lines.join("\n"), Dialect::Srt, &[]);
            cues.push(Timed {
                start,
                end,
                text: text::clean(&body),
                place: place.unwrap_or_default(),
                words: Vec::new(),
            });
        }
    };
    while i < lines.len() {
        let line = lines[i];
        let next_is_timing = lines.get(i + 1).and_then(|l| timing(l)).is_some();
        if is_index(line) && next_is_timing {
            finish(current.take(), &mut cues);
            i += 1;
            continue;
        }
        if let Some((start, end)) = timing(line) {
            finish(current.take(), &mut cues);
            current = Some((start, end, Vec::new()));
            i += 1;
            continue;
        }
        match current.as_mut() {
            Some(_) if line.trim().is_empty() => {}
            Some((_, _, body)) => body.push(line),
            None if !line.trim().is_empty() => {
                warnings.push(format!("line {}: text outside a cue, left out", i + 1));
            }
            None => {}
        }
        i += 1;
    }
    finish(current.take(), &mut cues);
    for c in &mut cues {
        c.text = c
            .text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n");
    }
    Read {
        cues,
        warnings,
        language: None,
    }
}

pub fn write(cues: &[Timed], positions: bool) -> String {
    let mut out = String::new();
    for (n, c) in cues.iter().enumerate() {
        let prefix = match (positions, c.place) {
            (true, Place::Top) => "{\\an8}",
            (true, Place::Middle) => "{\\an5}",
            _ => "",
        };
        out.push_str(&format!(
            "{}\r\n{} --> {}\r\n{prefix}{}\r\n\r\n",
            n + 1,
            clock(c.start, ','),
            clock(c.end, ','),
            c.text.replace('\n', "\r\n")
        ));
    }
    out
}
