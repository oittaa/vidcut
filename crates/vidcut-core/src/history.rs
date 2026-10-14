//! The undo history file, `$XDG_STATE_HOME/vidcut/history/<hash of the
//! project path>.jsonl`, a JSON line each ([`Line`]):
//!
//! 1. the project file it is of,
//! 2. the project before its oldest step,
//! 3. then the steps, undos and redos as they happened. Each step has its
//!    place on the undo stack (`at`, from 1), and each save ends with a
//!    [`Tail`]: the hash of the project text it wrote and the stacks' sizes.
//!
//! A save adds lines at the end. Loading reads only the last [`Tail`].
//! Undo reads back from the end as far as it needs: the step at place `d`
//! is the newest step line at `d` before the step above it
//! ([`HistoryFile::top`]). Past [`HISTORY_CAP`] steps the file is written
//! again with the newest 90% of the limit ([`HistoryFile::compact`]), its
//! first project moved forward over the steps that go.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::editor::{Entry, HISTORY_CAP, History, Moved};

/// The last line of a save.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Tail {
    /// The hash of the project text the save wrote ([`text_hash`]).
    pub hash: String,
    /// The steps there are to undo and to redo.
    pub undo: usize,
    pub redo: usize,
    /// The step lines in the file.
    pub steps: usize,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Line {
    Of(PathBuf),
    Base(Value),
    Step(Step),
    Undo(usize),
    Redo(usize),
    Saved(Tail),
}

/// A step and its place on the undo stack. `at` comes first, so a line's
/// place is read without parsing the rest ([`place`]).
#[derive(Serialize, Deserialize)]
struct Step {
    at: usize,
    #[serde(flatten)]
    entry: Entry,
}

fn line(l: &Line) -> String {
    let mut s = serde_json::to_string(l).expect("history lines are JSON");
    s.push('\n');
    s
}

fn step_line(at: usize, entry: &Entry) -> String {
    line(&Line::Step(Step {
        at,
        entry: entry.clone(),
    }))
}

/// A step line's place on the undo stack; `None` for other lines.
fn place(line: &str) -> Option<usize> {
    let rest = line.strip_prefix(r#"{"step":{"at":"#)?;
    let digits = rest.find(|c: char| !c.is_ascii_digit())?;
    rest[..digits].parse().ok()
}

fn entry_of(line: &str) -> std::io::Result<Entry> {
    match serde_json::from_str(line) {
        Ok(Line::Step(s)) => Ok(s.entry),
        _ => Err(bad("a step line that isn't one")),
    }
}

fn bad(what: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, what.to_string())
}

/// The hash of a project file's text, as the history keeps it: FNV-1a.
pub(crate) fn text_hash(text: &str) -> String {
    let h = text.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{h:016x}")
}

/// Whether a file holds more steps than the limit: its oldest then go.
pub(crate) fn due(tail: &Tail) -> bool {
    tail.steps > HISTORY_CAP
}

/// Files being compacted in this process.
static COMPACTING: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);

/// The bytes read at once going back through a file.
const CHUNK: u64 = 64 * 1024;

/// Calls `each` with the lines before byte `end` of `file`, last first,
/// and where each starts, until it says to stop. `end` is where a line
/// ends, after its `\n`.
fn lines_back(
    file: &mut File,
    end: u64,
    chunk: u64,
    mut each: impl FnMut(u64, &str) -> std::io::Result<bool>,
) -> std::io::Result<()> {
    // Bytes from `pos` to the end of the last line not yet given.
    let mut pending: Vec<u8> = Vec::new();
    let mut pos = end;
    while pos > 0 {
        let start = pos.saturating_sub(chunk);
        let mut read = vec![0; (pos - start) as usize];
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut read)?;
        read.extend_from_slice(&pending);
        pos = start;
        // Whole lines are those after a `\n`, and the first when at the
        // file's start.
        let mut stop = read.len();
        while let Some(nl) = read[..stop.saturating_sub(1)]
            .iter()
            .rposition(|&b| b == b'\n')
        {
            let text = std::str::from_utf8(&read[nl + 1..stop]).map_err(|_| bad("not text"))?;
            if !each(start + nl as u64 + 1, text)? {
                return Ok(());
            }
            stop = nl + 1;
        }
        read.truncate(stop);
        pending = read;
        if pos == 0 && !pending.is_empty() {
            let text = std::str::from_utf8(&pending).map_err(|_| bad("not text"))?;
            each(0, text)?;
        }
    }
    Ok(())
}

/// The history file of one project file.
#[derive(Debug, Clone)]
pub(crate) struct HistoryFile {
    path: PathBuf,
    of: PathBuf,
}

impl HistoryFile {
    /// `path`, the history of project file `of`.
    pub(crate) fn new(path: PathBuf, of: PathBuf) -> Self {
        Self { path, of }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// The last save's [`Tail`] and the byte after it: `None` when there is
    /// no file of this project, or no save in it.
    pub(crate) fn tail(&self) -> Option<(Tail, u64)> {
        let mut file = File::open(&self.path).ok()?;
        let mut first = String::new();
        {
            use std::io::BufRead;
            std::io::BufReader::new(&mut file)
                .read_line(&mut first)
                .ok()?;
        }
        match serde_json::from_str(&first).ok()? {
            Line::Of(of) if of == self.of => {}
            _ => return None,
        }
        let end = file.metadata().ok()?.len();
        // A save cut off at its end leaves a part of a line: it is past the
        // last tail.
        let mut found = None;
        lines_back(&mut file, end, CHUNK, |at, text| {
            if text.ends_with('\n')
                && let Ok(Line::Saved(t)) = serde_json::from_str(text)
            {
                found = Some((t, at + text.len() as u64));
                return Ok(false);
            }
            Ok(true)
        })
        .ok()?;
        found
    }

    /// The newest `n` steps to undo, newest first, and the next `m` to
    /// redo, next first, as of `tail` ending at byte `end`.
    pub(crate) fn top(
        &self,
        tail: &Tail,
        end: u64,
        n: usize,
        m: usize,
    ) -> std::io::Result<(Vec<Entry>, Vec<Entry>)> {
        let (n, m) = (n.min(tail.undo), m.min(tail.redo));
        let d = tail.undo;
        let mut file = File::open(&self.path)?;
        let mut undo = Vec::new();
        let mut target = d;
        // The next step to redo is the newest at the place above the undo
        // stack's top, and the ones after it to redo are after it in the
        // file: those are read until it is.
        let mut next: Option<(u64, String)> = None;
        let mut later: HashMap<usize, Vec<(u64, String)>> = HashMap::new();
        lines_back(&mut file, end, CHUNK, |at, text| {
            let Some(p) = place(text) else {
                return Ok(true);
            };
            if m > 0 && next.is_none() {
                if p == d + 1 {
                    next = Some((at, text.to_string()));
                } else if p > d + 1 && p <= d + m {
                    later.entry(p).or_default().push((at, text.to_string()));
                }
            }
            if undo.len() < n && p == target {
                undo.push(entry_of(text)?);
                target -= 1;
            }
            Ok(undo.len() < n || (m > 0 && next.is_none()))
        })?;
        if undo.len() < n {
            return Err(bad("fewer steps to undo than the file says"));
        }
        let mut redo = Vec::new();
        if let Some((mut since, text)) = next {
            redo.push(entry_of(&text)?);
            // Each one after the next was made after the one before it.
            for p in d + 2..=d + m {
                let (at, text) = later
                    .get(&p)
                    .and_then(|ls| ls.iter().filter(|(at, _)| *at > since).max_by_key(|l| l.0))
                    .ok_or_else(|| bad("fewer steps to redo than the file says"))?;
                redo.push(entry_of(text)?);
                since = *at;
            }
        }
        if redo.len() < m {
            return Err(bad("fewer steps to redo than the file says"));
        }
        Ok((undo, redo))
    }

    /// The whole history up to byte `end`: the project before its oldest
    /// step, and the stacks.
    pub(crate) fn read_all(&self, end: u64) -> std::io::Result<(Value, History)> {
        let mut text = String::new();
        File::open(&self.path)?
            .take(end)
            .read_to_string(&mut text)?;
        let mut lines = text.lines().skip(1);
        let Some(Ok(Line::Base(base))) = lines.next().map(serde_json::from_str) else {
            return Err(bad("no first project"));
        };
        let mut h = History::default();
        for l in lines {
            // A part of a line a save left: the lines around it stand.
            let Ok(l) = serde_json::from_str::<Line>(l) else {
                continue;
            };
            match l {
                Line::Step(s) => {
                    h.undo.truncate(s.at.saturating_sub(1));
                    h.undo.push(s.entry);
                    h.redo.clear();
                }
                Line::Undo(n) => shift(n, &mut h.undo, &mut h.redo),
                Line::Redo(n) => shift(n, &mut h.redo, &mut h.undo),
                Line::Of(_) | Line::Base(_) | Line::Saved(_) => {}
            }
        }
        Ok((base, h))
    }

    /// Writes the file anew: `base`, the project before `history`'s oldest
    /// step, then the steps, then a tail with `hash`.
    pub(crate) fn write_whole(
        &self,
        base: &Value,
        history: &History,
        hash: &str,
    ) -> std::io::Result<Tail> {
        let (text, tail) = whole(&self.of, base, history, hash);
        std::fs::create_dir_all(self.path.parent().expect("history dir"))?;
        crate::store::write_atomic(&self.path, &text)?;
        Ok(tail)
    }

    /// Adds `moved` after the save `tail` that ends at byte `end`, and then
    /// the new tail. Anything after `end`, a save cut off, goes.
    pub(crate) fn append(
        &self,
        tail: &Tail,
        end: u64,
        moved: Vec<Moved>,
        hash: &str,
    ) -> std::io::Result<Tail> {
        let mut t = tail.clone();
        let mut add = String::new();
        for m in moved {
            match m {
                Moved::Step(e) => {
                    t.undo += 1;
                    t.redo = 0;
                    t.steps += 1;
                    add.push_str(&step_line(t.undo, &e));
                }
                Moved::Undo(n) => {
                    let n = (n as usize).min(t.undo);
                    t.undo -= n;
                    t.redo += n;
                    add.push_str(&line(&Line::Undo(n)));
                }
                Moved::Redo(n) => {
                    let n = (n as usize).min(t.redo);
                    t.redo -= n;
                    t.undo += n;
                    add.push_str(&line(&Line::Redo(n)));
                }
            }
        }
        t.hash = hash.to_string();
        add.push_str(&line(&Line::Saved(t.clone())));
        let mut file = File::options().write(true).open(&self.path)?;
        file.set_len(end)?;
        file.seek(SeekFrom::Start(end))?;
        file.write_all(add.as_bytes())?;
        Ok(t)
    }

    /// When the file holds more steps than the limit: writes it again with
    /// the newest 90% of the limit's steps to undo and all those to redo, its first
    /// project moved forward over the steps that go. The work is done
    /// without `lock`; it is taken only to add the lines saved meanwhile and
    /// put the new file in place. One at a time for each file.
    pub(crate) fn compact<L>(
        &self,
        lock: impl FnOnce() -> std::io::Result<L>,
    ) -> std::io::Result<()> {
        {
            let mut running = COMPACTING.lock().unwrap_or_else(|e| e.into_inner());
            if !running
                .get_or_insert_with(HashSet::new)
                .insert(self.path.clone())
            {
                return Ok(());
            }
        }
        let result = self.compact_to(HISTORY_CAP, lock);
        if let Some(running) = COMPACTING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
        {
            running.remove(&self.path);
        }
        result
    }

    /// [`compact`](Self::compact) past `limit` steps, keeping 90% of it.
    fn compact_to<L>(
        &self,
        limit: usize,
        lock: impl FnOnce() -> std::io::Result<L>,
    ) -> std::io::Result<()> {
        let Some((tail, end)) = self.tail() else {
            return Ok(());
        };
        if tail.steps <= limit {
            return Ok(());
        }
        let keep = limit - limit / 10;
        let (mut base, mut h) = self.read_all(end)?;
        let gone = h.undo.len().saturating_sub(keep);
        for e in h.undo.drain(..gone) {
            e.change.apply(&mut base, true).map_err(|e| bad(&e))?;
        }
        let (mut text, mut kept) = whole(&self.of, &base, &h, &tail.hash);
        let _lock = lock()?;
        // Saved meanwhile: after `end`, on the stack's places less `gone`.
        // Unless the file was written anew: `tail` is no longer at `end`.
        let Some((_, now_end)) = self.tail() else {
            return Ok(());
        };
        if now_end < end || self.saved_before(end)? != Some(tail) {
            return Ok(());
        }
        let mut more = String::new();
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(end))?;
        file.take(now_end - end).read_to_string(&mut more)?;
        for l in more.lines() {
            let Ok(l) = serde_json::from_str::<Line>(l) else {
                continue;
            };
            text.push_str(&line(&match l {
                Line::Step(mut s) => {
                    s.at = s.at.saturating_sub(gone);
                    kept.steps += 1;
                    Line::Step(s)
                }
                Line::Saved(mut t) => {
                    t.undo = t.undo.saturating_sub(gone);
                    t.steps = kept.steps;
                    Line::Saved(t)
                }
                l => l,
            }));
        }
        crate::store::write_atomic(&self.path, &text)
    }
}

impl HistoryFile {
    /// The tail on the line that ends at byte `end`, if that line is one.
    fn saved_before(&self, end: u64) -> std::io::Result<Option<Tail>> {
        let mut file = File::open(&self.path)?;
        let mut found = None;
        lines_back(&mut file, end, CHUNK, |_, text| {
            if let Ok(Line::Saved(t)) = serde_json::from_str(text) {
                found = Some(t);
            }
            Ok(false)
        })?;
        Ok(found)
    }
}

/// The text of a whole file, and its tail.
fn whole(of: &Path, base: &Value, h: &History, hash: &str) -> (String, Tail) {
    let mut text = line(&Line::Of(of.to_path_buf()));
    text.push_str(&line(&Line::Base(base.clone())));
    let mut at = 0;
    for e in &h.undo {
        at += 1;
        text.push_str(&step_line(at, e));
    }
    // The steps to redo as if made and then undone.
    for e in h.redo.iter().rev() {
        at += 1;
        text.push_str(&step_line(at, e));
    }
    if !h.redo.is_empty() {
        text.push_str(&line(&Line::Undo(h.redo.len())));
    }
    let tail = Tail {
        hash: hash.to_string(),
        undo: h.undo.len(),
        redo: h.redo.len(),
        steps: at,
    };
    text.push_str(&line(&Line::Saved(tail.clone())));
    (text, tail)
}

/// Moves the newest `n` steps from one stack to the other, as many as there
/// are.
fn shift(n: usize, from: &mut Vec<Entry>, to: &mut Vec<Entry>) {
    for _ in 0..n {
        let Some(e) = from.pop() else { return };
        to.push(e);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::change::Change;

    /// Step `k`: `n` from `k - 1` to `k`.
    fn step(k: i64) -> Entry {
        Entry {
            label: format!("step {k}"),
            change: Change::between(&json!({"n": k - 1}), &json!({"n": k})).unwrap(),
        }
    }

    /// A history of steps 1..=`n` on `{"n": 0}`.
    fn history(dir: &Path, n: i64) -> HistoryFile {
        let file = HistoryFile::new(dir.join("h.jsonl"), dir.join("p.vidcut"));
        let h = History {
            undo: (1..=n).map(step).collect(),
            redo: Vec::new(),
        };
        file.write_whole(&json!({"n": 0}), &h, "x").unwrap();
        file
    }

    fn labels(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|e| e.label.as_str()).collect()
    }

    #[test]
    fn lines_come_last_first_across_reads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lines");
        std::fs::write(&path, "a\nbb\nccc\n").unwrap();
        let mut got = Vec::new();
        lines_back(&mut File::open(&path).unwrap(), 9, 2, |at, text| {
            got.push((at, text.to_string()));
            Ok(true)
        })
        .unwrap();
        assert_eq!(
            got,
            [
                (5, "ccc\n".into()),
                (2, "bb\n".into()),
                (0, "a\n".to_string())
            ]
        );
    }

    #[test]
    fn the_step_to_undo_is_the_newest_at_its_place() {
        let dir = tempfile::tempdir().unwrap();
        let file = history(dir.path(), 2);
        let (tail, end) = file.tail().unwrap();
        let moved = vec![Moved::Undo(1), Moved::Step(step(3))];
        file.append(&tail, end, moved, "y").unwrap();
        let (tail, end) = file.tail().unwrap();
        let (undo, _) = file.top(&tail, end, 2, 0).unwrap();
        assert_eq!(labels(&undo), ["step 3", "step 1"]);
    }

    #[test]
    fn the_steps_to_redo_come_in_the_order_they_were_made() {
        let dir = tempfile::tempdir().unwrap();
        let file = history(dir.path(), 3);
        let (tail, end) = file.tail().unwrap();
        file.append(&tail, end, vec![Moved::Undo(3)], "y").unwrap();
        let (tail, end) = file.tail().unwrap();
        let (_, redo) = file.top(&tail, end, 0, 3).unwrap();
        assert_eq!(labels(&redo), ["step 1", "step 2", "step 3"]);
    }

    #[test]
    fn a_compaction_keeps_90_percent_of_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let file = history(dir.path(), 11);
        file.compact_to(10, || Ok(())).unwrap();
        let (tail, end) = file.tail().unwrap();
        let (base, h) = file.read_all(end).unwrap();
        assert_eq!((tail.undo, base), (9, json!({"n": 2})));
        assert_eq!(labels(&h.undo)[0], "step 3");
    }

    #[test]
    fn a_step_saved_during_a_compaction_stays() {
        let dir = tempfile::tempdir().unwrap();
        let file = history(dir.path(), 11);
        let meanwhile = || {
            let (tail, end) = file.tail().unwrap();
            file.append(&tail, end, vec![Moved::Step(step(12))], "y")
        };
        file.compact_to(10, meanwhile).unwrap();
        let (tail, end) = file.tail().unwrap();
        let (undo, _) = file.top(&tail, end, 2, 0).unwrap();
        assert_eq!((tail.undo, labels(&undo)), (10, vec!["step 12", "step 11"]));
    }
}
