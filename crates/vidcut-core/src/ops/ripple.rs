use std::collections::BTreeSet;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::model::Project;
use crate::request::CopiedNode;

use super::grade::{self, GradeOf};

const KEPT: [&str; 2] = ["label", "on"];
const WHOLE: [&str; 1] = ["keys"];

#[derive(Debug, Clone)]
enum Step {
    Key(String),
    At(usize),
}

impl Step {
    fn of<'a>(&self, v: &'a Value) -> Option<&'a Value> {
        match self {
            Step::Key(k) => v.get(k),
            Step::At(i) => v.get(i),
        }
    }

    fn of_mut<'a>(&self, v: &'a mut Value) -> Option<&'a mut Value> {
        match self {
            Step::Key(k) => v.get_mut(k),
            Step::At(i) => v.get_mut(i),
        }
    }
}

fn json(c: &CopiedNode) -> Result<Value> {
    serde_json::to_value(c).map_err(|e| Error::internal(e.to_string()))
}

fn changes(was: Option<&Value>, now: Option<&Value>, at: &[Step], out: &mut Vec<Vec<Step>>) {
    let deeper = |step: Step| [at, &[step]].concat();
    match (was, now) {
        (Some(Value::Object(a)), Some(Value::Object(b))) => {
            for k in a.keys().chain(b.keys()).collect::<BTreeSet<_>>() {
                changes(a.get(k), b.get(k), &deeper(Step::Key(k.clone())), out);
            }
        }
        (Some(Value::Array(a)), Some(Value::Array(b))) if a.len() == b.len() => {
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                changes(Some(x), Some(y), &deeper(Step::At(i)), out);
            }
        }
        (a, b) if a != b => out.push(at.to_vec()),
        _ => {}
    }
}

fn changed(was: &Value, now: &Value) -> Vec<Vec<Step>> {
    let mut out = Vec::new();
    let (Some(a), Some(b)) = (was.as_object(), now.as_object()) else {
        return out;
    };
    let keys: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
    for k in keys.into_iter().filter(|k| !KEPT.contains(&k.as_str())) {
        let at = [Step::Key(k.clone())];
        match WHOLE.contains(&k.as_str()) {
            true if a.get(k) != b.get(k) => out.push(at.to_vec()),
            true => {}
            false => changes(a.get(k), b.get(k), &at, &mut out),
        }
    }
    out
}

fn took(target: &mut Value, now: &Value, path: &[Step]) {
    let Some((step, rest)) = path.split_first() else {
        target.clone_from(now);
        return;
    };
    let source = step.of(now);
    if let Some(n) = source
        && let Some(t) = step.of_mut(target)
    {
        took(t, n, rest);
        return;
    }
    match (step, source) {
        (Step::Key(k), Some(n)) => {
            if let Some(o) = target.as_object_mut() {
                o.insert(k.clone(), n.clone());
            }
        }
        (Step::Key(k), None) => {
            if let Some(o) = target.as_object_mut() {
                o.remove(k);
            }
        }
        (Step::At(_), _) => target.clone_from(now),
    }
}

pub fn ripple(
    p: &mut Project,
    from: &GradeOf,
    n: u32,
    before: &CopiedNode,
    to: &[GradeOf],
) -> Result<Vec<String>> {
    let timeline = |o: &GradeOf| matches!(o, GradeOf::Timeline);
    if timeline(from) || to.iter().any(timeline) {
        return Err(Error::invalid(
            "a ripple goes from a clip grade to clip grades",
        ));
    }
    let now = json(&grade::copy(p, from, n)?)?;
    let paths = changed(&json(before)?, &now);
    let mut rippled = Vec::new();
    for one in to.iter().filter(|o| *o != from) {
        let GradeOf::Clip(id, _) = one else {
            continue;
        };
        let g = grade::stored(p, one)?;
        if g.corrector(n).is_none() || g.node(n).is_some_and(|d| d.locked) {
            continue;
        }
        let mut v = json(&grade::copy(p, one, n)?)?;
        for path in &paths {
            took(&mut v, &now, path);
        }
        let settings: CopiedNode = serde_json::from_value(v).map_err(|e| {
            Error::invalid(format!("node {n} of {id} does not take the changes: {e}"))
        })?;
        grade::paste(p, std::slice::from_ref(one), n, &settings)?;
        rippled.push(id.clone());
    }
    Ok(rippled)
}
