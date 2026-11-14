use serde_json::{Value, json};

use crate::composition::{Frames, cleaned};
use crate::effect_keys::Keyed;
use crate::error::{Error, Result};
use crate::model::{
    Composition, Connect, Ease, EffectValue, Node, PathSample, Pattern, Project, Tool, ToolType,
};
use crate::request::{NodeChange, NodeInput, PatternTrim};

use super::effect::kind_applied;
use super::locate;
use super::picture::{node_checked, takes};

fn comp_of<'a>(p: &'a mut Project, id: &str) -> Result<&'a mut Composition> {
    let (ti, ci) = locate(p, id)?;
    takes(p, ti, &p.tracks[ti].clips[ci])?;
    let start = Frames::of(p, &p.tracks[ti].clips[ci]).zero;
    let c = &mut p.tracks[ti].clips[ci];
    if c.generator.is_some() {
        return Err(Error::invalid(format!(
            "{id} is a generator, which takes no composition"
        )));
    }
    let picture = c.picture.get_or_insert_with(Default::default);
    Ok(picture.composition.get_or_insert_with(|| Composition {
        start,
        ..Composition::plain()
    }))
}

fn kept<'a>(p: &'a mut Project, id: &str) -> Result<&'a mut Composition> {
    let (ti, ci) = locate(p, id)?;
    p.tracks[ti].clips[ci]
        .picture
        .as_mut()
        .and_then(|x| x.composition.as_mut())
        .ok_or_else(|| Error::not_found(format!("{id} has no composition")))
}

fn found(c: &Composition, id: &str, name: &str) -> Result<usize> {
    c.index(name).ok_or_else(|| {
        Error::not_found(format!(
            "{id}'s composition has no node {name}; picture.get lists them"
        ))
    })
}

fn one_of(c: &Composition, tool: &Tool, id: &str) -> Result<()> {
    let once = matches!(tool, Tool::MediaIn | Tool::MediaOut);
    match once && c.nodes.iter().any(|n| n.tool == *tool) {
        true => Err(Error::invalid(format!(
            "{id}'s composition has a {} node already",
            tool.label()
        ))),
        false => Ok(()),
    }
}

fn shifted(c: &mut Composition, from: [f64; 2]) {
    let [x, y] = from;
    let taken = c
        .nodes
        .iter()
        .any(|n| (n.place[0] - x).abs() < 0.5 && (n.place[1] - y).abs() < 0.5);
    if taken {
        for n in &mut c.nodes {
            if n.place[0] > x - 0.5 && (n.place[1] - y).abs() < 0.5 {
                n.place[0] += 1.0;
            }
        }
    }
}

fn above(c: &Composition, [x, mut y]: [f64; 2]) -> [f64; 2] {
    y -= 1.0;
    while c
        .nodes
        .iter()
        .any(|n| (n.place[0] - x).abs() < 0.5 && (n.place[1] - y).abs() < 0.5)
    {
        y -= 1.0;
    }
    [x, y]
}

fn apart(c: &Composition) -> [f64; 2] {
    let low = c.nodes.iter().map(|n| n.place[1]).fold(0.0, f64::max);
    [0.0, low + 1.0]
}

fn insert(c: &mut Composition, mut node: Node, after: Option<&str>, id: &str) -> Result<()> {
    let after = match after {
        Some(a) => Some(c.nodes[found(c, id, a)?].clone()),
        None => None,
    };
    let picture = |a: &Node| a.tool.flows() || a.tool == Tool::MediaIn;
    match after {
        Some(a) if node.tool.flows() && picture(&a) => {
            let spot = [a.place[0] + 1.0, a.place[1]];
            shifted(c, spot);
            node.place = spot;
            for n in &mut c.nodes {
                if n.input.as_deref() == Some(a.name.as_str()) {
                    n.input = Some(node.name.clone());
                }
            }
            node.input = Some(a.name);
        }
        Some(a) if node.tool.is_mask() && a.tool.works() => {
            node.place = above(c, a.place);
            node.effect_mask = a.effect_mask.clone();
            if let Some(i) = c.index(&a.name) {
                c.nodes[i].effect_mask = Some(node.name.clone());
            }
        }
        Some(a) if node.tool.is_mask() && a.tool.is_mask() => {
            node.place = above(c, a.place);
            for n in &mut c.nodes {
                if n.effect_mask.as_deref() == Some(a.name.as_str()) {
                    n.effect_mask = Some(node.name.clone());
                }
            }
            node.effect_mask = Some(a.name);
        }
        _ => node.place = apart(c),
    }
    c.nodes.push(node);
    Ok(())
}

fn tidied(p: &mut Project, id: &str) -> Result<()> {
    let (ti, ci) = locate(p, id)?;
    let c = &mut p.tracks[ti].clips[ci];
    if let Some(picture) = c.picture.take() {
        c.picture = super::picture::checked(&picture)?;
    }
    Ok(())
}

pub fn add(p: &mut Project, id: &str, tool: ToolType, after: Option<&str>) -> Result<Value> {
    let c = comp_of(p, id)?;
    let made = tool.made();
    one_of(c, &made, id)?;
    let name = c.free_name(made.base());
    insert(c, Node::new(&name, made, [0.0, 0.0]), after, id)?;
    tidied(p, id)?;
    Ok(json!({ "clip": id, "node": name }))
}

pub fn copied(p: &mut Project, id: &str, name: &str) -> Result<Node> {
    let c = kept(p, id)?;
    Ok(c.nodes[found(c, id, name)?].clone())
}

fn kept_paths(n: &mut Node, ids: &[String]) {
    if let Tool::Tracker(t) = &mut n.tool {
        for pattern in &mut t.patterns {
            pattern.path = pattern.path.take().filter(|path| ids.contains(path));
        }
    }
}

fn kept_connect(d: &Composition, n: &mut Node, to: Option<String>) {
    let Some(m) = n.tool.mask_mut() else {
        return;
    };
    let tracker = |name: &str| d.node(name).and_then(|t| t.tool.tracker()).is_some();
    m.connect = m.connect.take().and_then(|k| {
        let name = to.unwrap_or(k.tracker);
        tracker(&name).then_some(Connect { tracker: name, ..k })
    });
    if m.connect.is_some() {
        m.center = None;
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Landing<'a> {
    After(&'a str),
    At([f64; 2]),
    Over(&'a str),
    Apart,
}

fn replaced(c: &mut Composition, mut node: Node, name: &str) {
    let Some(i) = c.index(name) else {
        return;
    };
    let old = c.nodes.remove(i);
    node.place = old.place;
    node.input = old.input.filter(|_| node.tool.takes_input());
    node.effect_mask = old.effect_mask.filter(|_| node.tool.takes_mask());
    let picture = node.tool.flows();
    let mask = node.tool.is_mask();
    let patterns = node.tool.tracker().map_or(0, |t| t.patterns.len());
    for n in &mut c.nodes {
        if n.input.as_deref() == Some(name) {
            n.input = picture.then(|| node.name.clone());
        }
        if n.effect_mask.as_deref() == Some(name) {
            n.effect_mask = mask.then(|| node.name.clone());
        }
        let Some(m) = n.tool.mask_mut() else {
            continue;
        };
        if let Some(k) = m.connect.as_mut().filter(|k| k.tracker == name) {
            match (k.pattern as usize) <= patterns {
                true => k.tracker = node.name.clone(),
                false => m.connect = None,
            }
        }
    }
    c.nodes.insert(i, node);
}

pub fn paste(p: &mut Project, id: &str, nodes: Vec<Node>, to: Landing) -> Result<Value> {
    let nodes = nodes.iter().map(node_checked).collect::<Result<Vec<_>>>()?;
    let ids: Vec<String> = p
        .clip(id)
        .map(|c| c.motion.iter().map(|m| m.id.clone()).collect())
        .unwrap_or_default();
    let c = comp_of(p, id)?;
    let spot = match to {
        Landing::After(a) => {
            let a = &c.nodes[found(c, id, a)?];
            [a.place[0] + 1.0, a.place[1]]
        }
        Landing::Over(r) => {
            if nodes.len() != 1 {
                return Err(Error::usage("comp.paste with `replace` takes one node"));
            }
            c.nodes[found(c, id, r)?].place
        }
        Landing::At(at) if at.iter().all(|v| v.is_finite()) => at,
        Landing::At(_) => {
            return Err(Error::invalid("a node's place is two finite numbers"));
        }
        Landing::Apart => apart(c),
    };
    let gone: Vec<String> = match to {
        Landing::Over(r) => (c.node(r).and_then(|n| n.tool.tracker()).into_iter())
            .flat_map(|t| t.patterns.iter().filter_map(|x| x.path.clone()))
            .collect(),
        _ => Vec::new(),
    };
    let mut d = c.clone();
    let start = d.nodes.len();
    for n in &nodes {
        one_of(&d, &n.tool, id)?;
        let mut node = n.clone();
        if d.node(&node.name).is_some() {
            node.name = d.copy_name(&node.name);
        }
        d.nodes.push(node);
    }
    let names: Vec<String> = d.nodes[start..].iter().map(|n| n.name.clone()).collect();
    let Some(last) = names.last().cloned() else {
        return Err(Error::usage("comp.paste has no node to paste"));
    };
    let copied = |old: &str| {
        let i = nodes.iter().position(|n| n.name == old)?;
        names.get(i).cloned()
    };
    for k in start..d.nodes.len() {
        let mut node = d.nodes[k].clone();
        kept_paths(&mut node, &ids);
        let tracker = node.tool.mask().and_then(|m| m.connect.as_ref());
        let to = tracker.and_then(|t| copied(&t.tracker));
        kept_connect(&d, &mut node, to);
        d.nodes[k] = node;
    }
    if let [_] = names.as_slice()
        && let Some(mut node) = d.nodes.pop()
    {
        node.input = None;
        node.effect_mask = None;
        match to {
            Landing::After(a) => insert(&mut d, node, Some(a), id)?,
            Landing::Over(r) => replaced(&mut d, node, r),
            Landing::At(_) => {
                node.place = spot;
                d.nodes.push(node);
            }
            Landing::Apart => insert(&mut d, node, None, id)?,
        }
    } else {
        let renamed = |old: &Option<String>| {
            let i = nodes.iter().position(|n| Some(&n.name) == old.as_ref())?;
            names.get(i).cloned()
        };
        let corner = nodes.iter().fold([f64::INFINITY; 2], |a, n| {
            [a[0].min(n.place[0]), a[1].min(n.place[1])]
        });
        for (k, n) in nodes.iter().enumerate() {
            let m = &mut d.nodes[start + k];
            m.input = renamed(&n.input);
            m.effect_mask = renamed(&n.effect_mask);
            m.place = [
                n.place[0] - corner[0] + spot[0],
                n.place[1] - corner[1] + spot[1],
            ];
        }
    }
    *c = d;
    tidied(p, id)?;
    super::motion::unused(p, id, &gone);
    Ok(json!({ "clip": id, "node": last, "nodes": names }))
}

pub fn connect(
    p: &mut Project,
    id: &str,
    name: &str,
    input: NodeInput,
    from: Option<&str>,
) -> Result<()> {
    let c = kept(p, id)?;
    let i = found(c, id, name)?;
    let tool = c.nodes[i].tool.clone();
    let takes = match input {
        NodeInput::Input => tool.takes_input(),
        NodeInput::EffectMask => tool.takes_mask(),
    };
    if !takes {
        return Err(Error::invalid(format!(
            "{name} is a {}, which has no {} input",
            tool.label(),
            match input {
                NodeInput::Input => "picture",
                NodeInput::EffectMask => "effect mask",
            }
        )));
    }
    if let Some(from) = from {
        let source = &c.nodes[found(c, id, from)?].tool;
        let fits = match input {
            NodeInput::Input => source.works() || *source == Tool::MediaIn,
            NodeInput::EffectMask => source.is_mask(),
        };
        if !fits {
            return Err(Error::invalid(format!(
                "{from} is a {}: an input takes a tool or Media In, and an effect mask takes a mask",
                source.label()
            )));
        }
    }
    let to = from.map(str::to_string);
    match input {
        NodeInput::Input => c.nodes[i].input = to,
        NodeInput::EffectMask => c.nodes[i].effect_mask = to,
    }
    if let Some(at) = c.loops() {
        return Err(Error::invalid(format!(
            "that join makes a loop through {at}"
        )));
    }
    Ok(())
}

pub fn rename(p: &mut Project, id: &str, name: &str, to: &str) -> Result<Value> {
    let c = kept(p, id)?;
    let i = found(c, id, name)?;
    let next = cleaned(to);
    if next.is_empty() {
        return Err(Error::invalid(format!(
            "{to:?} has no letters: a name keeps letters, digits and _, with no digit first"
        )));
    }
    if next != name && c.node(&next).is_some() {
        return Err(Error::invalid(format!(
            "{id}'s composition has a node {next} already"
        )));
    }
    for n in &mut c.nodes {
        for join in [&mut n.input, &mut n.effect_mask] {
            if join.as_deref() == Some(name) {
                *join = Some(next.clone());
            }
        }
        let connect = n.tool.mask_mut().and_then(|m| m.connect.as_mut());
        if let Some(k) = connect.filter(|k| k.tracker == name) {
            k.tracker = next.clone();
        }
    }
    c.nodes[i].name = next.clone();
    Ok(json!({ "clip": id, "node": next }))
}

pub fn remove(p: &mut Project, id: &str, name: &str) -> Result<()> {
    let c = kept(p, id)?;
    let gone = c.nodes.remove(found(c, id, name)?);
    let paths: Vec<String> = (gone.tool.tracker().into_iter())
        .flat_map(|t| t.patterns.iter().filter_map(|x| x.path.clone()))
        .collect();
    let passes = gone.tool.flows();
    for n in &mut c.nodes {
        if let Some(m) = n.tool.mask_mut()
            && m.connect.as_ref().is_some_and(|k| k.tracker == name)
        {
            m.connect = None;
        }
        if n.input.as_deref() == Some(name) {
            n.input = gone.input.clone().filter(|_| passes);
        }
        if n.effect_mask.as_deref() == Some(name) {
            n.effect_mask = gone.effect_mask.clone().filter(|_| gone.tool.is_mask());
        }
    }
    tidied(p, id)?;
    super::motion::unused(p, id, &paths);
    Ok(())
}

fn taken<T>(from: &mut Option<T>, to: &mut T) {
    if let Some(v) = from.take() {
        *to = v;
    }
}

fn given<T>(from: &mut Option<T>, to: &mut Option<T>) {
    if from.is_some() {
        *to = from.take();
    }
}

fn applied(n: &Node, change: &NodeChange) -> Result<Node> {
    let mut next = n.clone();
    let mut left = change.clone();
    taken(&mut left.pass_through, &mut next.pass_through);
    taken(&mut left.place, &mut next.place);
    if next.tool.works() {
        given(&mut left.blend, &mut next.blend);
    }
    if matches!(
        &next.tool,
        Tool::Effect(crate::model::EffectKind::GaussianBlur(_))
    ) {
        taken(&mut left.values.blur_alpha, &mut next.blur_alpha);
    }
    let polygon = matches!(next.tool, Tool::Polygon(_));
    let rectangle = matches!(next.tool, Tool::Rectangle(_));
    match &mut next.tool {
        Tool::Blur(b) => {
            given(&mut left.filter, &mut b.filter);
            given(&mut left.values.size, &mut b.size);
            taken(&mut left.lock_xy, &mut b.lock_xy);
            given(&mut left.y_size, &mut b.y_size);
            given(&mut left.passes, &mut b.passes);
            given(&mut left.clipping, &mut b.clipping);
            given(&mut left.channels, &mut b.channels);
        }
        Tool::Effect(kind) => kind_applied(kind, &mut left.values)?,
        Tool::Ellipse(m) | Tool::Rectangle(m) | Tool::Polygon(m) => {
            given(&mut left.level, &mut m.level);
            given(&mut left.soft_edge, &mut m.soft_edge);
            given(&mut left.filter, &mut m.filter);
            given(&mut left.passes, &mut m.passes);
            given(&mut left.border_width, &mut m.border_width);
            taken(&mut left.values.invert, &mut m.invert);
            taken(&mut left.solid, &mut m.solid);
            taken(&mut left.show_view_controls, &mut m.show_view_controls);
            if let Some(connect) = left.connect.take() {
                m.connect = connect;
                if m.connect.is_some() {
                    m.center = None;
                }
            }
            if let Some(c) = left.values.center.take() {
                let [x, y] = c[..] else {
                    return Err(Error::invalid("a mask's center is two numbers, [x, y]"));
                };
                m.center = Some([x, y]);
            }
            if polygon {
                taken(&mut left.points, &mut m.points);
                given(&mut left.fill_method, &mut m.fill_method);
                given(&mut left.values.size, &mut m.size);
                given(&mut left.z_rotation, &mut m.z_rotation);
                given(&mut left.x_rotation, &mut m.x_rotation);
                given(&mut left.y_rotation, &mut m.y_rotation);
            } else {
                given(&mut left.width, &mut m.width);
                given(&mut left.height, &mut m.height);
                given(&mut left.angle, &mut m.angle);
            }
            if rectangle {
                given(&mut left.corner_radius, &mut m.corner_radius);
            }
        }
        Tool::Tracker(t) => {
            let k = left.pattern.take().unwrap_or(1);
            let touched = left.pattern_name.is_some()
                || left.pattern_enabled.is_some()
                || left.pattern_center.is_some()
                || left.pattern_width.is_some()
                || left.pattern_height.is_some()
                || left.pattern_offset.is_some()
                || left.remove_pattern;
            if touched {
                let i = (k as usize)
                    .checked_sub(1)
                    .filter(|i| *i < t.patterns.len());
                let Some(i) = i else {
                    return Err(super::motion::no_pattern(&n.name, k, t.patterns.len()));
                };
                let pattern = &mut t.patterns[i];
                taken(&mut left.pattern_name, &mut pattern.name);
                taken(&mut left.pattern_enabled, &mut pattern.enabled);
                given(&mut left.pattern_center, &mut pattern.center);
                given(&mut left.pattern_width, &mut pattern.width);
                given(&mut left.pattern_height, &mut pattern.height);
                given(&mut left.pattern_offset, &mut pattern.offset);
                if left.remove_pattern {
                    t.patterns.remove(i);
                    left.remove_pattern = false;
                }
            }
            if left.add_pattern {
                t.patterns.push(Pattern::new(t.free_name()));
                left.add_pattern = false;
            }
        }
        Tool::MediaIn | Tool::MediaOut => {}
    }
    if left != NodeChange::default() {
        return Err(Error::invalid(format!(
            "comp.set has a value that {} ({}) doesn't have",
            n.name,
            n.tool.label()
        )));
    }
    Ok(next)
}

fn named_values(n: &Node, change: &NodeChange) -> Result<Vec<EffectValue>> {
    let values = n.values();
    let mut probe = n.clone();
    for v in &values {
        probe.write(*v, f64::NAN);
    }
    let probe = applied(&probe, change)?;
    Ok(values
        .into_iter()
        .filter(|v| probe.takes(*v) && !probe.plain(*v).is_nan())
        .collect())
}

pub fn set(
    p: &mut Project,
    id: &str,
    name: &str,
    change: &NodeChange,
    frame: Option<i64>,
) -> Result<()> {
    if *change == NodeChange::default() {
        return Err(Error::usage("comp.set needs a value to change"));
    }
    let mut change = change.clone();
    trimmed(p, id, name, &mut change, frame)?;
    moved_path(p, id, name, &mut change, frame)?;
    let pattern = change.pattern.unwrap_or(1);
    let change = &change;
    let c = kept(p, id)?;
    let i = found(c, id, name)?;
    let n = &mut c.nodes[i];
    if matches!(change.connect, Some(Some(_))) && !n.keys_of(EffectValue::Center).is_empty() {
        return Err(Error::invalid(format!(
            "{name}'s center has keyframes: first remove them (comp.key), then connect it"
        )));
    }
    let gone = match &n.tool {
        Tool::Tracker(t) if change.remove_pattern => {
            let k = (pattern as usize).checked_sub(1);
            k.and_then(|k| t.patterns.get(k)?.path.clone())
        }
        _ => None,
    };
    let mut next = applied(n, change)?;
    let named = named_values(n, change)?;
    for value in n.keys.iter().map(|k| k.value).filter(|v| named.contains(v)) {
        let Some(frame) = frame else {
            return Err(Error::usage(format!(
                "{name}'s {} has keyframes: give `frame` for the keyframe",
                value.name().replace('_', " ")
            )));
        };
        let to = next.point(value);
        if to != n.point_at(value, frame as f64) {
            next.put_point(value, frame, to);
        }
        next.write_point(value, n.point(value));
    }
    *n = node_checked(&next)?;
    if change.remove_pattern {
        unhooked(c, name, pattern);
    }
    tidied(p, id)?;
    super::motion::unused(p, id, gone.as_slice());
    Ok(())
}

fn unhooked(c: &mut Composition, tracker: &str, gone: u32) {
    for m in c.nodes.iter_mut().filter_map(|n| n.tool.mask_mut()) {
        let Some(k) = m.connect.as_mut().filter(|k| k.tracker == tracker) else {
            continue;
        };
        match k.pattern.cmp(&gone) {
            std::cmp::Ordering::Equal => m.connect = None,
            std::cmp::Ordering::Greater => k.pattern -= 1,
            std::cmp::Ordering::Less => {}
        }
    }
}

fn trimmed(
    p: &mut Project,
    id: &str,
    name: &str,
    change: &mut NodeChange,
    frame: Option<i64>,
) -> Result<()> {
    let Some(trim) = change.pattern_trim.take() else {
        return Ok(());
    };
    let k = change.pattern.unwrap_or(1);
    let c = p
        .clip(id)
        .ok_or_else(|| Error::not_found(format!("no clip {id}")))?;
    let comp = c.picture.as_ref().and_then(|x| x.composition.as_ref());
    let t = comp.and_then(|x| x.node(name)?.tool.tracker());
    let t = t.ok_or_else(|| Error::invalid(format!("{name} is not a tracker")))?;
    let i = (k as usize)
        .checked_sub(1)
        .filter(|i| *i < t.patterns.len());
    let i = i.ok_or_else(|| super::motion::no_pattern(name, k, t.patterns.len()))?;
    let Some(path) = t.patterns[i].path.clone() else {
        return Ok(());
    };
    let Some(m) = c.motion.iter().find(|m| m.id == path) else {
        return Ok(());
    };
    let samples = m.samples.clone();
    let at = match (trim, frame) {
        (PatternTrim::All, _) => None,
        (_, Some(frame)) => Some(Frames::of(p, c).file(frame)),
        (_, None) => {
            return Err(Error::usage(
                "pattern_trim start and end take off the samples before or after `frame`",
            ));
        }
    };
    let clear = match (trim, at) {
        (PatternTrim::Start, Some(at)) => samples.first().map(|s| [s.frame, at - 1]),
        (PatternTrim::End, Some(at)) => samples.last().map(|s| [at + 1, s.frame]),
        _ => None,
    };
    let clear = clear.filter(|[a, b]| a <= b);
    let left = |[a, b]: [i64; 2]| samples.iter().any(|s| s.frame < a || s.frame > b);
    let round = |v: f64| (v * 1e5).round() / 1e5;
    let edge = at.and_then(|at| m.base_at(at as f64).map(|a| (at, a)));
    let edge = edge.map(|(frame, a)| PathSample {
        frame,
        center: a.center.map(round),
        size: a.size.map(round),
        rotation: a.rotation,
    });
    match clear {
        Some(span) if left(span) => {
            let edge = edge.as_slice();
            let to = super::motion::PathChange {
                clear: Some(span),
                samples: Some(edge).filter(|e| !e.is_empty()),
                ..Default::default()
            };
            super::motion::set(p, id, &path, to)?;
        }
        None if trim != PatternTrim::All => {}
        _ => {
            let c = kept(p, id)?;
            let n = found(c, id, name)?;
            if let Tool::Tracker(t) = &mut c.nodes[n].tool {
                t.patterns[i].path = None;
            }
            super::motion::unused(p, id, &[path]);
        }
    }
    Ok(())
}

fn moved_path(
    p: &mut Project,
    id: &str,
    name: &str,
    change: &mut NodeChange,
    frame: Option<i64>,
) -> Result<()> {
    let boxed = change.pattern_center.is_some()
        || change.pattern_width.is_some()
        || change.pattern_height.is_some();
    let Some(frame) = frame.filter(|_| boxed) else {
        return Ok(());
    };
    let Some(c) = p.clip(id) else {
        return Ok(());
    };
    let comp = c.picture.as_ref().and_then(|x| x.composition.as_ref());
    let tracker = comp.and_then(|k| k.node(name)?.tool.tracker());
    let k = change.pattern.unwrap_or(1) as usize;
    let pattern = tracker.and_then(|t| t.patterns.get(k.checked_sub(1)?));
    let Some(path) = pattern.and_then(|x| x.path.clone()) else {
        return Ok(());
    };
    let source = Frames::of(p, c).file(frame);
    let now = c.motion.iter().find(|m| m.id == path);
    let Some(now) = now.and_then(|m| m.base_at(source as f64)) else {
        return Ok(());
    };
    let (lo, hi) = Pattern::SIDES;
    let sides = [change.pattern_width, change.pattern_height];
    if sides.iter().flatten().any(|v| !(lo..=hi).contains(v)) {
        return Err(Error::invalid(
            "a pattern's width and height are from 0.001 to 0.5",
        ));
    }
    let sample = PathSample {
        frame: source,
        center: change
            .pattern_center
            .map_or(now.center, |[x, y]| [x, 1.0 - y]),
        size: [
            change.pattern_width.unwrap_or(now.size[0]),
            change.pattern_height.unwrap_or(now.size[1]),
        ],
        rotation: now.rotation,
    };
    let to = super::motion::PathChange {
        samples: Some(&[sample]),
        ..Default::default()
    };
    super::motion::set(p, id, &path, to)?;
    change.pattern_center = None;
    change.pattern_width = None;
    change.pattern_height = None;
    Ok(())
}

pub fn key(
    p: &mut Project,
    id: &str,
    name: &str,
    frame: i64,
    (value, ease): (Option<EffectValue>, Option<Ease>),
) -> Result<Value> {
    let c = kept(p, id)?;
    let i = found(c, id, name)?;
    let n = &mut c.nodes[i];
    if n.values().is_empty() {
        return Err(Error::invalid(format!(
            "{name} is a {}, which has no values to key",
            n.tool.label()
        )));
    }
    if let Some(v) = value
        && !n.values().contains(&v)
    {
        return Err(Error::invalid(format!(
            "{name} is a {}, which has no {}",
            n.tool.label(),
            v.name().replace('_', " ")
        )));
    }
    let on = match value {
        Some(v) => n.keys_of(v).iter().any(|k| k.frame == frame),
        None => n.has_key_at(frame),
    };
    let keyed = match (ease, value) {
        (Some(ease), Some(v)) if on => {
            n.ease_value_key(v, frame, ease);
            true
        }
        (Some(ease), None) if on => {
            n.ease_keys(frame, ease);
            true
        }
        (Some(_), _) => {
            return Err(Error::invalid(format!(
                "{name} of {id} has no keyframes on frame {frame}"
            )));
        }
        (None, Some(v)) => n.toggle_value_key(v, frame),
        (None, None) => n.toggle_keys(frame),
    };
    *n = node_checked(n)?;
    Ok(json!({ "clip": id, "node": name, "frame": frame, "keyed": keyed }))
}

pub fn looping(p: &mut Project, id: &str, on: Option<bool>, ping_pong: Option<bool>) -> Result<()> {
    let c = comp_of(p, id)?;
    c.loop_play = match (on, ping_pong) {
        (None, None) => !c.loop_play,
        (on, _) => on.unwrap_or(c.loop_play),
    };
    c.ping_pong = ping_pong.unwrap_or(c.ping_pong);
    Ok(())
}

pub fn reset(p: &mut Project, id: &str, name: &str) -> Result<()> {
    let c = kept(p, id)?;
    let i = found(c, id, name)?;
    let n = &mut c.nodes[i];
    let connect = n.tool.mask().and_then(|m| m.connect.clone());
    let mut tool = n.tool.made();
    if let Some(m) = tool.mask_mut() {
        m.connect = connect;
    }
    *n = Node {
        name: n.name.clone(),
        tool,
        input: n.input.take(),
        effect_mask: n.effect_mask.take(),
        pass_through: n.pass_through,
        ..Node::new(&n.name, Tool::MediaIn, n.place)
    };
    Ok(())
}
