//! What an edit changed in a project, both ways. Undo steps keep this,
//! not copies of the project, so a step costs about what it changed.
//!
//! It is found by comparing the project's JSON before and after. Lists of
//! things with ids (media, tracks, clips, markers) are compared by id: an
//! inserted clip is one entry, and a ripple that moves every later clip by
//! the same frames is one run.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// What changed in one value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Change {
    /// Replaced: the value before and after, `None` where it is absent.
    #[serde(rename = "set")]
    Set {
        #[serde(
            rename = "was",
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present"
        )]
        before: Option<Value>,
        #[serde(
            rename = "is",
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present"
        )]
        after: Option<Value>,
    },
    /// An object, with what changed in each of its fields that did.
    #[serde(rename = "fields")]
    Fields(BTreeMap<String, Change>),
    /// A list of things with ids.
    #[serde(rename = "items")]
    Items(Box<Items>),
}

/// A value that is there, `null` too: a missing one is `None`.
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}

/// What changed in a list of things with ids.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Items {
    /// Those only before, with their places before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    removed: Vec<(usize, Value)>,
    /// Those only after, with their places after.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    added: Vec<(usize, Value)>,
    /// The ids of the others in order, before and after, when it changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    order: Option<(Vec<String>, Vec<String>)>,
    /// Runs of the others, side by side, whose one whole-number field
    /// moved by the same amount.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    runs: Vec<Run>,
    /// What changed in the others, by id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    changed: BTreeMap<String, Change>,
}

/// The things from `first` to `last` (by id, inclusive) whose `field` went
/// up by `by`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    first: String,
    last: String,
    field: String,
    by: i64,
}

impl Change {
    /// What changed from `before` to `after`; `None` when nothing did.
    pub fn between(before: &Value, after: &Value) -> Option<Self> {
        if before == after {
            return None;
        }
        Some(match (before, after) {
            (Value::Object(a), Value::Object(b)) => {
                let mut fields = BTreeMap::new();
                for k in a.keys().chain(b.keys().filter(|k| !a.contains_key(*k))) {
                    let change = match (a.get(k), b.get(k)) {
                        (Some(x), Some(y)) => Self::between(x, y),
                        (x, y) => Some(Self::Set {
                            before: x.cloned(),
                            after: y.cloned(),
                        }),
                    };
                    if let Some(c) = change {
                        fields.insert(k.clone(), c);
                    }
                }
                Self::Fields(fields)
            }
            (Value::Array(a), Value::Array(b)) => match (ids(a), ids(b)) {
                (Some(x), Some(y)) => Self::Items(Box::new(Items::between(a, &x, b, &y))),
                _ => Self::replaced(before, after),
            },
            _ => Self::replaced(before, after),
        })
    }

    fn replaced(before: &Value, after: &Value) -> Self {
        Self::Set {
            before: Some(before.clone()),
            after: Some(after.clone()),
        }
    }

    /// Makes the change in `value` (`forward`), or takes it back. Fails
    /// when `value` isn't what the change was found on.
    pub fn apply(&self, value: &mut Value, forward: bool) -> Result<(), String> {
        match self {
            Self::Set { before, after } => {
                let to = if forward { after } else { before };
                *value = to.clone().ok_or("a value that isn't there")?;
            }
            Self::Fields(fields) => {
                let object = value.as_object_mut().ok_or("not an object")?;
                for (k, change) in fields {
                    match change {
                        Self::Set { before, after } => {
                            match if forward { after } else { before } {
                                Some(v) => object.insert(k.clone(), v.clone()),
                                None => object.remove(k),
                            };
                        }
                        c => c.apply(object.get_mut(k).ok_or(format!("no {k}"))?, forward)?,
                    }
                }
            }
            Self::Items(items) => {
                items.apply(value.as_array_mut().ok_or("not a list")?, forward)?;
            }
        }
        Ok(())
    }
}

/// The ids of a list's things, when each is an object with its own.
fn ids(list: &[Value]) -> Option<Vec<&str>> {
    let ids: Vec<&str> = list
        .iter()
        .map(|v| v.get("id").and_then(Value::as_str))
        .collect::<Option<_>>()?;
    let unique: HashSet<&str> = ids.iter().copied().collect();
    (unique.len() == ids.len()).then_some(ids)
}

fn id(v: &Value) -> &str {
    v.get("id").and_then(Value::as_str).unwrap_or_default()
}

impl Items {
    fn between(a: &[Value], ia: &[&str], b: &[Value], ib: &[&str]) -> Self {
        let (in_a, in_b): (HashSet<&str>, HashSet<&str>) =
            (ia.iter().copied().collect(), ib.iter().copied().collect());
        let only = |list: &[Value], ids: &[&str], other: &HashSet<&str>| {
            list.iter()
                .zip(ids)
                .enumerate()
                .filter(|(_, (_, id))| !other.contains(*id))
                .map(|(i, (v, _))| (i, v.clone()))
                .collect()
        };
        let kept_a: Vec<&str> = ia.iter().copied().filter(|i| in_b.contains(i)).collect();
        let kept_b: Vec<&str> = ib.iter().copied().filter(|i| in_a.contains(i)).collect();
        let mut items = Self {
            removed: only(a, ia, &in_b),
            added: only(b, ib, &in_a),
            order: (kept_a != kept_b).then(|| {
                let owned = |ids: &[&str]| ids.iter().map(|s| s.to_string()).collect();
                (owned(&kept_a), owned(&kept_b))
            }),
            ..Self::default()
        };
        let before: HashMap<&str, &Value> = ia.iter().copied().zip(a).collect();
        let after: HashMap<&str, &Value> = ib.iter().copied().zip(b).collect();
        // Side by side runs only where the order stays.
        let in_order = items.order.is_none();
        let mut run: Option<(Run, Change, usize)> = None;
        for id in kept_b {
            let Some(change) = Change::between(before[id], after[id]) else {
                items.close(run.take());
                continue;
            };
            let shift = moved(&change).filter(|_| in_order);
            if let (Some((field, by)), Some((r, _, n))) = (&shift, run.as_mut())
                && r.field == *field
                && r.by == *by
            {
                r.last = id.to_string();
                *n += 1;
                continue;
            }
            items.close(run.take());
            match shift {
                Some((field, by)) => {
                    let first = Run {
                        first: id.to_string(),
                        last: id.to_string(),
                        field,
                        by,
                    };
                    run = Some((first, change, 1));
                }
                None => {
                    items.changed.insert(id.to_string(), change);
                }
            }
        }
        items.close(run);
        items
    }

    /// Keeps a run of two or more as a run, one as its own change.
    fn close(&mut self, run: Option<(Run, Change, usize)>) {
        match run {
            Some((run, _, n)) if n > 1 => self.runs.push(run),
            Some((run, change, _)) => {
                self.changed.insert(run.first, change);
            }
            None => {}
        }
    }

    fn apply(&self, list: &mut Vec<Value>, forward: bool) -> Result<(), String> {
        let (gone, come) = if forward {
            (&self.removed, &self.added)
        } else {
            (&self.added, &self.removed)
        };
        let order = self
            .order
            .as_ref()
            .map(|(a, b)| if forward { b } else { a });
        // The others first: those that come and go don't move with them.
        for (id, change) in &self.changed {
            let v = list
                .iter_mut()
                .find(|v| self::id(v) == id)
                .ok_or(format!("no {id}"))?;
            change.apply(v, forward)?;
        }
        let moving: HashSet<&str> = gone.iter().chain(come).map(|(_, v)| id(v)).collect();
        for run in &self.runs {
            run.apply(list, &moving, forward)?;
        }
        let going: HashSet<&str> = gone.iter().map(|(_, v)| id(v)).collect();
        list.retain(|v| !going.contains(id(v)));
        if let Some(order) = order {
            let mut by_id: HashMap<String, Value> =
                list.drain(..).map(|v| (id(&v).to_string(), v)).collect();
            for i in order {
                list.push(by_id.remove(i).ok_or(format!("no {i}"))?);
            }
        }
        for (i, v) in come {
            if *i > list.len() {
                return Err(format!("no place {i} for {}", id(v)));
            }
            list.insert(*i, v.clone());
        }
        Ok(())
    }
}

/// The one whole-number field a change moved, and by how much.
fn moved(change: &Change) -> Option<(String, i64)> {
    let Change::Fields(fields) = change else {
        return None;
    };
    let (k, c) = fields.iter().next().filter(|_| fields.len() == 1)?;
    let Change::Set {
        before: Some(a),
        after: Some(b),
    } = c
    else {
        return None;
    };
    Some((k.clone(), b.as_i64()?.checked_sub(a.as_i64()?)?))
}

impl Run {
    /// Moves the run's field, skipping things that come or go.
    fn apply(
        &self,
        list: &mut [Value],
        moving: &HashSet<&str>,
        forward: bool,
    ) -> Result<(), String> {
        let at = |id: &str| list.iter().position(|v| self::id(v) == id);
        let first = at(&self.first).ok_or(format!("no {}", self.first))?;
        let last = at(&self.last).ok_or(format!("no {}", self.last))?;
        let by = if forward { self.by } else { -self.by };
        for v in &mut list[first..=last] {
            if moving.contains(id(v)) {
                continue;
            }
            let n = v.get(&self.field).and_then(Value::as_i64).ok_or(format!(
                "no whole number {} in {}",
                self.field,
                id(v)
            ))?;
            v[&self.field] = (n + by).into();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn change(before: Value, after: Value) -> Value {
        serde_json::to_value(Change::between(&before, &after).unwrap()).unwrap()
    }

    #[test]
    fn a_changed_field_is_all_that_is_kept() {
        assert_eq!(
            change(json!({"a": 1, "b": 2}), json!({"a": 1, "b": 3})),
            json!({"fields": {"b": {"set": {"was": 2, "is": 3}}}})
        );
    }

    #[test]
    fn an_inserted_item_is_one_entry() {
        let a = json!([{"id": "c1"}, {"id": "c2"}]);
        let b = json!([{"id": "c1"}, {"id": "c3"}, {"id": "c2"}]);
        assert_eq!(
            change(a, b),
            json!({"items": {"added": [[1, {"id": "c3"}]]}})
        );
    }

    #[test]
    fn things_moved_by_the_same_frames_are_one_run() {
        let a =
            json!([{"id": "c1", "start": 0}, {"id": "c2", "start": 50}, {"id": "c3", "start": 90}]);
        let b = json!([{"id": "c2", "start": 40}, {"id": "c3", "start": 80}]);
        assert_eq!(
            change(a, b),
            json!({"items": {
                "removed": [[0, {"id": "c1", "start": 0}]],
                "runs": [{"first": "c2", "last": "c3", "field": "start", "by": -10}]
            }})
        );
    }

    #[test]
    fn a_missing_field_is_told_from_null() {
        assert_eq!(
            change(json!({"a": null}), json!({})),
            json!({"fields": {"a": {"set": {"was": null}}}})
        );
    }

    #[test]
    fn a_change_goes_there_and_back() {
        let a = json!({"t": [{"id": "c1", "s": 0}, {"id": "c2", "s": 5, "n": "x"}, {"id": "c3", "s": 9}]});
        let b = json!({"t": [{"id": "c3", "s": 4}, {"id": "c4", "s": 7}, {"id": "c2", "s": 0}], "u": 1});
        let c = Change::between(&a, &b).unwrap();
        let mut v = a.clone();
        c.apply(&mut v, true).unwrap();
        assert_eq!(v, b);
        c.apply(&mut v, false).unwrap();
        assert_eq!(v, a);
    }
}
