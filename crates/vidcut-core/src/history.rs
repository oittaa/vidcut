use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::editor::{Entry, HISTORY_CAP, History, Moved};
use crate::steps::{Step, Steps, StepsCache};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Tail {
    pub hash: String,
    pub undo: usize,
    pub redo: usize,
    pub steps: usize,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Line {
    Of(PathBuf),
    Base(Value),
    Step(Placed),
    Undo(usize),
    Redo(usize),
    Saved(Tail),
}

#[derive(Serialize, Deserialize)]
struct Placed {
    at: usize,
    #[serde(flatten)]
    entry: Entry,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Brief {
    Of(serde::de::IgnoredAny),
    Base(serde::de::IgnoredAny),
    Step(BriefStep),
    Undo(usize),
    Redo(usize),
    Saved(serde::de::IgnoredAny),
}

#[derive(Deserialize)]
struct BriefStep {
    at: usize,
    label: String,
    #[serde(default)]
    by: Option<crate::request::Client>,
    #[serde(default)]
    time: Option<u64>,
    #[serde(default)]
    opened: Option<u64>,
    #[serde(default)]
    clips: Vec<String>,
    #[serde(default)]
    spans: Vec<crate::steps::Span>,
    #[serde(default)]
    parts: Vec<String>,
}

const MARK: u64 = 256;

fn line(l: &Line) -> String {
    let mut s = serde_json::to_string(l).expect("history lines are JSON");
    s.push('\n');
    s
}

fn step_line(at: usize, entry: &Entry) -> String {
    line(&Line::Step(Placed {
        at,
        entry: entry.clone(),
    }))
}

fn place(line: &str) -> Option<usize> {
    let rest = line.strip_prefix(r#"{"step":{"at":"#)?;
    let digits = rest.find(|c: char| !c.is_ascii_digit())?;
    rest[..digits].parse().ok()
}

fn label_of(line: &str) -> std::io::Result<String> {
    let rest = line
        .strip_prefix(r#"{"step":{"at":"#)
        .ok_or_else(|| bad("a step line that isn't one"))?;
    let rest = rest
        .trim_start_matches(|c: char| c.is_ascii_digit())
        .strip_prefix(r#","label":"#)
        .ok_or_else(|| bad("a step line without its label first"))?;
    String::deserialize(&mut serde_json::Deserializer::from_str(rest))
        .map_err(|_| bad("a step's label that isn't text"))
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

pub(crate) fn text_hash(text: &str) -> String {
    crate::fingerprint::digest(text.as_bytes())
}

pub(crate) fn due(tail: &Tail) -> bool {
    tail.steps > HISTORY_CAP
}

static COMPACTING: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);

const CHUNK: u64 = 64 * 1024;

fn lines_back(
    file: &mut File,
    end: u64,
    chunk: u64,
    mut each: impl FnMut(u64, &str) -> std::io::Result<bool>,
) -> std::io::Result<()> {
    let mut pending: Vec<u8> = Vec::new();
    let mut pos = end;
    while pos > 0 {
        let start = pos.saturating_sub(chunk.max(pending.len() as u64));
        let mut read = vec![0; (pos - start) as usize];
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut read)?;
        read.extend_from_slice(&pending);
        pos = start;
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

#[derive(Debug, Clone)]
pub(crate) struct HistoryFile {
    path: PathBuf,
    of: PathBuf,
}

impl HistoryFile {
    pub(crate) fn new(path: PathBuf, of: PathBuf) -> Self {
        Self { path, of }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn tail(&self) -> Option<(Tail, u64)> {
        self.read_tail().ok().flatten()
    }

    pub(crate) fn read_tail(&self) -> std::io::Result<Option<(Tail, u64)>> {
        let mut file = match File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let mut first = String::new();
        {
            use std::io::BufRead;
            std::io::BufReader::new(&mut file).read_line(&mut first)?;
        }
        match serde_json::from_str(&first) {
            Ok(Line::Of(of)) if of == self.of => {}
            Ok(Line::Of(of)) => {
                return Err(bad(&format!("it is the history of {}", of.display())));
            }
            _ => return Err(bad("its first line doesn't say whose history it is")),
        }
        let end = file.metadata()?.len();
        let mut found = None;
        lines_back(&mut file, end, CHUNK, |at, text| {
            if text.ends_with('\n')
                && let Ok(Line::Saved(t)) = serde_json::from_str(text)
            {
                found = Some((t, at + text.len() as u64));
                return Ok(false);
            }
            Ok(true)
        })?;
        found
            .map(Some)
            .ok_or_else(|| bad("it has no complete save"))
    }

    pub(crate) fn top(
        &self,
        tail: &Tail,
        end: u64,
        n: usize,
        m: usize,
    ) -> std::io::Result<(Vec<Entry>, Vec<Entry>)> {
        self.top_with(tail, end, n, m, entry_of)
    }

    pub(crate) fn top_labels(
        &self,
        tail: &Tail,
        end: u64,
        n: usize,
        m: usize,
    ) -> std::io::Result<(Vec<String>, Vec<String>)> {
        self.top_with(tail, end, n, m, label_of)
    }

    fn top_with<T>(
        &self,
        tail: &Tail,
        end: u64,
        n: usize,
        m: usize,
        read: impl Fn(&str) -> std::io::Result<T>,
    ) -> std::io::Result<(Vec<T>, Vec<T>)> {
        let (n, m) = (n.min(tail.undo), m.min(tail.redo));
        let d = tail.undo;
        let mut file = File::open(&self.path)?;
        let mut undo = Vec::new();
        let mut target = d;
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
                undo.push(read(text)?);
                target -= 1;
            }
            Ok(undo.len() < n || (m > 0 && next.is_none()))
        })?;
        if undo.len() < n {
            return Err(bad("fewer steps to undo than the file says"));
        }
        let mut redo = Vec::new();
        if let Some((mut since, text)) = next {
            redo.push(read(&text)?);
            for p in d + 2..=d + m {
                let (at, text) = later
                    .get(&p)
                    .and_then(|ls| ls.iter().filter(|(at, _)| *at > since).max_by_key(|l| l.0))
                    .ok_or_else(|| bad("fewer steps to redo than the file says"))?;
                redo.push(read(text)?);
                since = *at;
            }
        }
        if redo.len() < m {
            return Err(bad("fewer steps to redo than the file says"));
        }
        Ok((undo, redo))
    }

    pub(crate) fn read_steps(&self, end: u64, cache: &mut StepsCache) -> std::io::Result<()> {
        let mut file = File::open(&self.path)?;
        let mut mark = vec![0; MARK.min(end) as usize];
        file.seek(SeekFrom::Start(end - mark.len() as u64))?;
        file.read_exact(&mut mark)?;
        let kept = cache.path == self.path && cache.end > 0 && cache.end <= end && {
            let mut was = vec![0; cache.mark.len()];
            file.seek(SeekFrom::Start(cache.end - was.len() as u64))?;
            file.read_exact(&mut was)?;
            was == cache.mark
        };
        let (mut steps, start) = if kept {
            (std::mem::take(&mut cache.steps), cache.end)
        } else {
            (Steps::default(), 0)
        };
        let mut text = String::new();
        file.seek(SeekFrom::Start(start))?;
        file.take(end - start).read_to_string(&mut text)?;
        for l in text.lines() {
            match serde_json::from_str::<Brief>(l) {
                Ok(Brief::Step(b)) => {
                    steps.done = b.at.saturating_sub(1).min(steps.line.len());
                    steps.place(Step {
                        label: b.label,
                        by: b.by,
                        time: b.time,
                        opened: b.opened,
                        clips: b.clips,
                        spans: b.spans,
                        parts: b.parts,
                    });
                }
                Ok(Brief::Undo(n)) => steps.back(n),
                Ok(Brief::Redo(n)) => steps.forward(n),
                _ => {}
            }
        }
        *cache = StepsCache {
            path: self.path.clone(),
            end,
            mark,
            steps,
        };
        Ok(())
    }

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

fn whole(of: &Path, base: &Value, h: &History, hash: &str) -> (String, Tail) {
    let mut text = line(&Line::Of(of.to_path_buf()));
    text.push_str(&line(&Line::Base(base.clone())));
    let mut at = 0;
    for e in &h.undo {
        at += 1;
        text.push_str(&step_line(at, e));
    }
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

    fn step(k: i64) -> Entry {
        Entry::new(
            format!("step {k}"),
            Change::between(&json!({"n": k - 1}), &json!({"n": k})).unwrap(),
        )
    }

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
        entries.iter().map(|e| e.step.label.as_str()).collect()
    }

    #[test]
    fn a_line_many_reads_long_comes_whole() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lines");
        let long = format!(
            "{}
",
            "x".repeat(1000)
        );
        std::fs::write(
            &path,
            format!(
                "a
{long}"
            ),
        )
        .unwrap();
        let mut got = Vec::new();
        lines_back(&mut File::open(&path).unwrap(), 1003, 4, |at, text| {
            got.push((at, text.len()));
            Ok(true)
        })
        .unwrap();
        assert_eq!(got, [(2, 1001), (0, 2)]);
    }

    #[test]
    fn a_steps_label_is_read_without_its_change() {
        assert_eq!(label_of(&step_line(12, &step(12))).unwrap(), "step 12");
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
        let moved = vec![Moved::Undo(1), Moved::Step(Box::new(step(3)))];
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
            file.append(&tail, end, vec![Moved::Step(Box::new(step(12)))], "y")
        };
        file.compact_to(10, meanwhile).unwrap();
        let (tail, end) = file.tail().unwrap();
        let (undo, _) = file.top(&tail, end, 2, 0).unwrap();
        assert_eq!((tail.undo, labels(&undo)), (10, vec!["step 12", "step 11"]));
    }

    #[test]
    fn a_step_written_before_steps_had_sources_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let file = HistoryFile::new(dir.path().join("h.jsonl"), dir.path().join("p.vidcut"));
        let of = serde_json::to_string(&dir.path().join("p.vidcut")).unwrap();
        let text = format!(
            "{{\"of\":{of}}}\n{{\"base\":{{\"n\":0}}}}\n\
             {{\"step\":{{\"at\":1,\"label\":\"old\",\"change\":{{\"set\":{{\"was\":0,\"is\":1}}}}}}}}\n\
             {{\"saved\":{{\"hash\":\"x\",\"undo\":1,\"redo\":0,\"steps\":1}}}}\n"
        );
        std::fs::write(file.path(), text).unwrap();
        let (_, end) = file.tail().unwrap();
        let mut cache = StepsCache::default();
        file.read_steps(end, &mut cache).unwrap();
        assert_eq!(cache.steps.line, [Step::labeled("old")]);
    }

    #[test]
    fn steps_read_again_after_a_save_match_a_fresh_read() {
        let dir = tempfile::tempdir().unwrap();
        let file = history(dir.path(), 3);
        let mut cache = StepsCache::default();
        let (tail, end) = file.tail().unwrap();
        file.read_steps(end, &mut cache).unwrap();
        file.append(
            &tail,
            end,
            vec![Moved::Undo(1), Moved::Step(Box::new(step(4)))],
            "y",
        )
        .unwrap();
        let (_, end) = file.tail().unwrap();
        file.read_steps(end, &mut cache).unwrap();
        let mut fresh = StepsCache::default();
        file.read_steps(end, &mut fresh).unwrap();
        assert_eq!(cache.steps, fresh.steps);
    }

    #[test]
    fn a_missing_history_is_no_history() {
        let dir = tempfile::tempdir().unwrap();
        let file = HistoryFile::new(dir.path().join("h.jsonl"), dir.path().join("p.vidcut"));
        assert!(file.read_tail().unwrap().is_none());
    }

    #[test]
    fn a_history_without_a_save_is_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        let file = history(dir.path(), 1);
        let text = std::fs::read_to_string(file.path()).unwrap();
        let cut = text.lines().take(2).collect::<Vec<_>>().join("\n");
        std::fs::write(file.path(), cut).unwrap();
        assert!(file.read_tail().is_err());
    }
}
