use std::collections::{BTreeMap, HashMap, HashSet};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Change {
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
    #[serde(rename = "fields")]
    Fields(BTreeMap<String, Change>),
    #[serde(rename = "items")]
    Items(Box<Items>),
}

fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Items {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    removed: Vec<(usize, Value)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    added: Vec<(usize, Value)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    order: Option<(Vec<String>, Vec<String>)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    runs: Vec<Run>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    changed: BTreeMap<String, Change>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    first: String,
    last: String,
    field: String,
    by: i64,
}

impl Change {
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

    pub fn of_items<T: Serialize + PartialEq>(
        a: &mut [T],
        b: &mut [T],
        id: impl Fn(&T) -> &str,
        mut each: impl FnMut(&mut T, &mut T) -> Option<Self>,
    ) -> Option<Self> {
        if a == b {
            return None;
        }
        let json = |x: &T| serde_json::to_value(x).expect("a project is JSON");
        let (ia, ib): (Vec<String>, Vec<String>) = (
            a.iter().map(|x| id(x).to_string()).collect(),
            b.iter().map(|x| id(x).to_string()).collect(),
        );
        let ra: Vec<&str> = ia.iter().map(String::as_str).collect();
        let rb: Vec<&str> = ib.iter().map(String::as_str).collect();
        let (in_a, in_b): (HashSet<&str>, HashSet<&str>) =
            (ra.iter().copied().collect(), rb.iter().copied().collect());
        if in_a.len() != ra.len() || in_b.len() != rb.len() {
            let all = |list: &[T]| Value::Array(list.iter().map(json).collect());
            return Some(Self::replaced(&all(a), &all(b)));
        }
        let only = |list: &[T], ids: &[&str], other: &HashSet<&str>| {
            list.iter()
                .zip(ids)
                .enumerate()
                .filter(|(_, (_, id))| !other.contains(*id))
                .map(|(i, (x, _))| (i, json(x)))
                .collect()
        };
        let (removed, added) = (only(a, &ra, &in_b), only(b, &rb, &in_a));
        let at_a: HashMap<&str, usize> = ra.iter().copied().zip(0..).collect();
        let at_b: HashMap<&str, usize> = rb.iter().copied().zip(0..).collect();
        let items = Items::build(&ra, &rb, removed, added, |id| {
            each(&mut a[at_a[id]], &mut b[at_b[id]])
        });
        Some(Self::Items(Box::new(items)))
    }

    pub fn apply_to_items<T: Serialize + DeserializeOwned>(
        &self,
        list: &mut Vec<T>,
        forward: bool,
        id: impl Fn(&T) -> &str,
        mut each: impl FnMut(&mut T, &Change, bool) -> Result<(), String>,
        mut shift: impl FnMut(&mut T, &str, i64) -> Result<(), String>,
    ) -> Result<(), String> {
        let items = match self {
            Self::Items(items) => items,
            Self::Set { before, after } => {
                *list = match if forward { after } else { before } {
                    Some(v) => from_json(v.clone())?,
                    None => Vec::new(),
                };
                return Ok(());
            }
            Self::Fields(_) => return Err("not a list".into()),
        };
        let (gone, come) = if forward {
            (&items.removed, &items.added)
        } else {
            (&items.added, &items.removed)
        };
        let order = items
            .order
            .as_ref()
            .map(|(a, b)| if forward { b } else { a });
        let at: HashMap<String, usize> = list
            .iter()
            .enumerate()
            .map(|(i, x)| (id(x).to_string(), i))
            .collect();
        let place = |k: &str| at.get(k).copied().ok_or(format!("no {k}"));
        for (k, change) in &items.changed {
            each(&mut list[place(k)?], change, forward)?;
        }
        let moving: HashSet<&str> = gone.iter().chain(come).map(|(_, v)| self::id(v)).collect();
        for run in &items.runs {
            let (first, last) = (place(&run.first)?, place(&run.last)?);
            let by = if forward { run.by } else { -run.by };
            for x in &mut list[first..=last] {
                if !moving.contains(id(x)) {
                    shift(x, &run.field, by)?;
                }
            }
        }
        let going: HashSet<&str> = gone.iter().map(|(_, v)| self::id(v)).collect();
        list.retain(|x| !going.contains(id(x)));
        if let Some(order) = order {
            let mut by_id: HashMap<String, T> =
                list.drain(..).map(|x| (id(&x).to_string(), x)).collect();
            for i in order {
                list.push(by_id.remove(i).ok_or(format!("no {i}"))?);
            }
        }
        for (i, v) in come {
            if *i > list.len() {
                return Err(format!("no place {i} for {}", self::id(v)));
            }
            list.insert(*i, from_json(v.clone())?);
        }
        Ok(())
    }

    fn replaced(before: &Value, after: &Value) -> Self {
        Self::Set {
            before: Some(before.clone()),
            after: Some(after.clone()),
        }
    }

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
        let (removed, added) = (only(a, ia, &in_b), only(b, ib, &in_a));
        let before: HashMap<&str, &Value> = ia.iter().copied().zip(a).collect();
        let after: HashMap<&str, &Value> = ib.iter().copied().zip(b).collect();
        Self::build(ia, ib, removed, added, |id| {
            Change::between(before[id], after[id])
        })
    }

    fn build(
        ia: &[&str],
        ib: &[&str],
        removed: Vec<(usize, Value)>,
        added: Vec<(usize, Value)>,
        mut change: impl FnMut(&str) -> Option<Change>,
    ) -> Self {
        let (in_a, in_b): (HashSet<&str>, HashSet<&str>) =
            (ia.iter().copied().collect(), ib.iter().copied().collect());
        let kept_a: Vec<&str> = ia.iter().copied().filter(|i| in_b.contains(i)).collect();
        let kept_b: Vec<&str> = ib.iter().copied().filter(|i| in_a.contains(i)).collect();
        let mut items = Self {
            removed,
            added,
            order: (kept_a != kept_b).then(|| {
                let owned = |ids: &[&str]| ids.iter().map(|s| s.to_string()).collect();
                (owned(&kept_a), owned(&kept_b))
            }),
            ..Self::default()
        };
        let in_order = items.order.is_none();
        let mut run: Option<(Run, Change, usize)> = None;
        for id in kept_b {
            let Some(change) = change(id) else {
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
        let at: HashMap<String, usize> = list
            .iter()
            .enumerate()
            .map(|(i, v)| (id(v).to_string(), i))
            .collect();
        for (id, change) in &self.changed {
            let i = at.get(id).ok_or(format!("no {id}"))?;
            change.apply(&mut list[*i], forward)?;
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

pub fn apply_json<T: Serialize + DeserializeOwned>(
    x: &mut T,
    change: &Change,
    forward: bool,
) -> Result<(), String> {
    let mut v = serde_json::to_value(&*x).map_err(|e| e.to_string())?;
    change.apply(&mut v, forward)?;
    *x = from_json(v)?;
    Ok(())
}

pub fn shift_json<T: Serialize + DeserializeOwned>(
    x: &mut T,
    field: &str,
    by: i64,
) -> Result<(), String> {
    let mut v = serde_json::to_value(&*x).map_err(|e| e.to_string())?;
    let n = v
        .get(field)
        .and_then(Value::as_i64)
        .ok_or(format!("no whole number {field} in {}", id(&v)))?;
    v[field] = (n + by).into();
    *x = from_json(v)?;
    Ok(())
}

fn from_json<T: DeserializeOwned>(v: Value) -> Result<T, String> {
    serde_json::from_value(v).map_err(|e| e.to_string())
}

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
    fn things_as_they_are_change_as_their_json_does() {
        #[derive(Serialize, PartialEq)]
        struct Thing {
            id: &'static str,
            start: i64,
        }
        let thing = |id, start| Thing { id, start };
        let mut a = [thing("c1", 0), thing("c2", 50), thing("c3", 90)];
        let mut b = [thing("c1", 0), thing("c3", 80), thing("c4", 0)];
        let each = |x: &mut Thing, y: &mut Thing| {
            Change::between(
                &serde_json::to_value(&*x).unwrap(),
                &serde_json::to_value(&*y).unwrap(),
            )
        };
        let found = Change::of_items(&mut a, &mut b, |t| t.id, each).unwrap();
        assert_eq!(
            serde_json::to_value(found).unwrap(),
            json!({"items": {
                "removed": [[1, {"id": "c2", "start": 50}]],
                "added": [[2, {"id": "c4", "start": 0}]],
                "changed": {"c3": {"fields": {"start": {"set": {"was": 90, "is": 80}}}}}
            }})
        );
    }

    #[test]
    fn a_change_is_made_in_things_as_they_are() {
        #[derive(Debug, Serialize, Deserialize, PartialEq)]
        struct Thing {
            id: String,
            start: i64,
        }
        let thing = |id: &str, start| Thing {
            id: id.into(),
            start,
        };
        let a =
            json!([{"id": "c1", "start": 0}, {"id": "c2", "start": 50}, {"id": "c3", "start": 90}]);
        let b = json!([{"id": "c2", "start": 40}, {"id": "c3", "start": 80}]);
        let ripple = Change::between(&a, &b).unwrap();
        let mut list = vec![thing("c1", 0), thing("c2", 50), thing("c3", 90)];
        ripple
            .apply_to_items(&mut list, true, |t| t.id.as_str(), apply_json, shift_json)
            .unwrap();
        assert_eq!(list, [thing("c2", 40), thing("c3", 80)]);
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
