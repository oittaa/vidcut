use serde_json::{Value, json};

use crate::composition::{Frames, cleaned};
use crate::effect_keys::Keyed;
use crate::error::{Error, Result};
use crate::model::{
    Composition, Connect, Ease, EffectValue, MotionPath, Node, PathSample, Pattern, Project,
    ShadingElement, Tool, ToolType,
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
    if c.generates() {
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
    let once = tool.is_media();
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
    match after {
        Some(a) if node.tool.flows() && a.tool.gives_picture() => {
            let spot = [a.place[0] + 1.0, a.place[1]];
            shifted(c, spot);
            node.place = spot;
            let inputs = NodeInput::ALL.into_iter().filter(|i| i.carries_picture());
            for input in inputs {
                for n in &mut c.nodes {
                    if n.join(input) == Some(a.name.as_str()) {
                        *n.join_mut(input) = Some(node.name.clone());
                    }
                }
            }
            node.input = Some(a.name);
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
        Some(a) if node.tool.is_mask() && a.tool.takes(NodeInput::EffectMask) => {
            node.place = above(c, a.place);
            node.effect_mask = a.effect_mask.clone();
            if let Some(i) = c.index(&a.name) {
                c.nodes[i].effect_mask = Some(node.name.clone());
            }
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
    let behind = match after {
        Some(a) => Some(&c.nodes[found(c, id, a)?]),
        None => None,
    };
    let merged = made.makes() && behind.is_some_and(|a| !a.tool.is_mask());
    let mut node = Node::new(&name, made, [0.0, 0.0]);
    let answer = match merged {
        false => {
            insert(c, node, after, id)?;
            json!({ "clip": id, "node": name })
        }
        true => {
            let merge = c.free_name(ToolType::Merge.made().base());
            let mut joined = Node::new(&merge, ToolType::Merge.made(), [0.0, 0.0]);
            joined.foreground = Some(name.clone());
            insert(c, joined, after, id)?;
            let place = c.node(&merge).map_or([0.0, 0.0], |m| m.place);
            node.place = above(c, place);
            c.nodes.push(node);
            json!({ "clip": id, "node": name, "merge": merge, "nodes": [merge] })
        }
    };
    tidied(p, id)?;
    Ok(answer)
}

pub fn copied(p: &mut Project, id: &str, name: &str) -> Result<Node> {
    let c = kept(p, id)?;
    Ok(c.nodes[found(c, id, name)?].clone())
}

fn kept_paths(n: &mut Node, takes: bool) {
    if let Some(t) = n.tool.tracker_mut().filter(|_| !takes) {
        for pattern in &mut t.patterns {
            pattern.path = None;
        }
    }
}

fn kept_connect(d: &Composition, n: &mut Node, to: Option<String>) {
    let tracker = |name: &str| d.node(name).and_then(|t| t.tool.tracker()).is_some();
    n.connect = n.connect.take().and_then(|k| {
        let name = to.unwrap_or(k.tracker);
        tracker(&name).then_some(Connect { tracker: name, ..k })
    });
    if n.connect.is_some() {
        n.center_to_usual();
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
    for input in NodeInput::ALL {
        let kept = old.join(input).filter(|_| node.tool.takes(input));
        *node.join_mut(input) = kept.map(str::to_string);
    }
    let patterns = node.tool.tracker().map_or(0, |t| t.patterns.len());
    for n in &mut c.nodes {
        for input in NodeInput::ALL {
            if n.join(input) == Some(name) {
                *n.join_mut(input) = node.tool.feeds(input).then(|| node.name.clone());
            }
        }
        if let Some(k) = n.connect.as_mut().filter(|k| k.tracker == name) {
            match (k.pattern as usize) <= patterns {
                true => k.tracker = node.name.clone(),
                false => n.connect = None,
            }
        }
    }
    c.nodes.insert(i, node);
}

pub fn paste(p: &mut Project, id: &str, nodes: Vec<Node>, to: Landing) -> Result<Value> {
    let nodes = nodes.iter().map(node_checked).collect::<Result<Vec<_>>>()?;
    let takes = p.clip(id).is_some_and(super::motion::takes);
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
        kept_paths(&mut node, takes);
        let to = node.connect.as_ref().and_then(|t| copied(&t.tracker));
        kept_connect(&d, &mut node, to);
        d.nodes[k] = node;
    }
    if let [_] = names.as_slice()
        && let Some(mut node) = d.nodes.pop()
    {
        for input in NodeInput::ALL {
            *node.join_mut(input) = None;
        }
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
        let renamed = |old: Option<&str>| {
            let i = nodes.iter().position(|n| Some(n.name.as_str()) == old)?;
            names.get(i).cloned()
        };
        let corner = nodes.iter().fold([f64::INFINITY; 2], |a, n| {
            [a[0].min(n.place[0]), a[1].min(n.place[1])]
        });
        for (k, n) in nodes.iter().enumerate() {
            let m = &mut d.nodes[start + k];
            for input in NodeInput::ALL {
                *m.join_mut(input) = renamed(n.join(input));
            }
            m.place = [
                n.place[0] - corner[0] + spot[0],
                n.place[1] - corner[1] + spot[1],
            ];
        }
    }
    *c = d;
    tidied(p, id)?;
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
    if !tool.takes(input) {
        return Err(Error::invalid(format!(
            "{name} ({}) has no {}",
            tool.label(),
            input.label()
        )));
    }
    if let Some(from) = from {
        let source = &c.nodes[found(c, id, from)?].tool;
        if !source.feeds(input) {
            return Err(Error::invalid(format!(
                "{from} ({}) does not fit: a picture input takes a node that gives a picture, and an effect mask takes a mask",
                source.label()
            )));
        }
    }
    *c.nodes[i].join_mut(input) = from.map(str::to_string);
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
        for input in NodeInput::ALL {
            if n.join(input) == Some(name) {
                *n.join_mut(input) = Some(next.clone());
            }
        }
        if let Some(k) = n.connect.as_mut().filter(|k| k.tracker == name) {
            k.tracker = next.clone();
        }
    }
    c.nodes[i].name = next.clone();
    Ok(json!({ "clip": id, "node": next }))
}

pub fn remove(p: &mut Project, id: &str, name: &str) -> Result<()> {
    let c = kept(p, id)?;
    let gone = c.nodes.remove(found(c, id, name)?);
    for n in &mut c.nodes {
        if n.connect.as_ref().is_some_and(|k| k.tracker == name) {
            n.connect = None;
        }
        for input in NodeInput::ALL {
            if n.join(input) == Some(name) {
                let through = match input.carries_picture() {
                    true => NodeInput::Input,
                    false => NodeInput::EffectMask,
                };
                let passes = gone.tool.takes(through) && gone.tool.feeds(input);
                *n.join_mut(input) = gone.join(through).filter(|_| passes).map(str::to_string);
            }
        }
    }
    tidied(p, id)?;
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
    if let Some(connect) = left.connect.take() {
        next.connect = connect;
        if next.connect.is_some() {
            next.center_to_usual();
        }
    }
    if next.tool.works() {
        given(&mut left.blend, &mut next.blend);
    }
    if next.tool.multiplies() {
        taken(&mut left.multiply_by_mask, &mut next.multiply_by_mask);
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
                    t.pivot_tracker = match t.pivot_pattern().cmp(&k) {
                        std::cmp::Ordering::Equal => None,
                        std::cmp::Ordering::Greater => {
                            Some(t.pivot_pattern() - 1).filter(|p| *p != 1)
                        }
                        std::cmp::Ordering::Less => t.pivot_tracker,
                    };
                    left.remove_pattern = false;
                }
            }
            if left.add_pattern {
                t.patterns.push(Pattern::new(t.free_name()));
                left.add_pattern = false;
            }
            given(&mut left.operation, &mut t.operation);
            given(&mut left.merge, &mut t.merge);
            given(&mut left.edges, &mut t.edges);
            given(&mut left.apply_mode, &mut t.apply_mode);
            given(&mut left.operator, &mut t.operator);
            given(&mut left.subtractive_additive, &mut t.subtractive_additive);
            taken(&mut left.match_position, &mut t.match_position);
            taken(&mut left.match_rotation, &mut t.match_rotation);
            taken(&mut left.match_scaling, &mut t.match_scaling);
            given(&mut left.pivot_type, &mut t.pivot_type);
            given(&mut left.pivot_tracker, &mut t.pivot_tracker);
            given(&mut left.pivot, &mut t.pivot);
            given(&mut left.reference_time, &mut t.reference_time);
            given(&mut left.custom_time, &mut t.custom_time);
            given(&mut left.intermediate_points, &mut t.intermediate_points);
            given(&mut left.fade_frames, &mut t.fade_frames);
        }
        Tool::Merge(m) => {
            if let Some(c) = left.values.center.take() {
                let [x, y] = c[..] else {
                    return Err(Error::invalid("a merge's center is two numbers, [x, y]"));
                };
                m.center = Some([x, y]);
            }
            given(&mut left.values.size, &mut m.size);
            given(&mut left.angle, &mut m.angle);
            taken(&mut left.flip_horizontal, &mut m.flip_horizontal);
            taken(&mut left.flip_vertical, &mut m.flip_vertical);
            taken(&mut left.invert_transform, &mut m.invert_transform);
            given(&mut left.edges, &mut m.edges);
            given(&mut left.apply_mode, &mut m.apply_mode);
            given(&mut left.operator, &mut m.operator);
            given(&mut left.subtractive_additive, &mut m.subtractive_additive);
            given(&mut left.alpha_gain, &mut m.alpha_gain);
            given(&mut left.burn_in, &mut m.burn_in);
            taken(&mut left.depth_merge, &mut m.depth_merge);
            given(&mut left.foreground_z_offset, &mut m.foreground_z_offset);
            given(
                &mut left.background_subtractive_additive,
                &mut m.background_subtractive_additive,
            );
        }
        Tool::TextPlus(t) => {
            if let Some(c) = left.values.center.take() {
                let [x, y] = c[..] else {
                    return Err(Error::invalid("a text's center is two numbers, [x, y]"));
                };
                t.center = Some([x, y]);
            }
            taken(&mut left.text, &mut t.text);
            given(&mut left.font, &mut t.font);
            given(&mut left.style, &mut t.style);
            given(&mut left.values.size, &mut t.size);
            given(&mut left.tracking, &mut t.tracking);
            given(&mut left.line_spacing, &mut t.line_spacing);
            given(&mut left.vertical_anchor, &mut t.vertical_anchor);
            given(&mut left.horizontal_anchor, &mut t.horizontal_anchor);
            given(&mut left.layout_size, &mut t.layout_size);
            given(&mut left.angle, &mut t.angle);
            shading_applied(t.element_mut(element_of(&mut left)?), &mut left)?;
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

fn element_of(change: &mut NodeChange) -> Result<u8> {
    let k = change.element.take().unwrap_or(1);
    match (1..=ShadingElement::ELEMENTS).contains(&k) {
        true => Ok(k),
        false => Err(Error::invalid("a text's shading elements are 1 to 8")),
    }
}

fn shading_applied(s: &mut ShadingElement, left: &mut NodeChange) -> Result<()> {
    if let Some(c) = left.values.color.take() {
        let [r, g, b] = c[..] else {
            return Err(Error::invalid("a text's color is three numbers, [r, g, b]"));
        };
        s.color = Some([r, g, b]);
    }
    given(&mut left.values.opacity, &mut s.opacity);
    given(&mut left.enabled, &mut s.enabled);
    given(&mut left.element_name, &mut s.name);
    given(&mut left.appearance, &mut s.appearance);
    given(&mut left.thickness, &mut s.thickness);
    taken(&mut left.outside_only, &mut s.outside_only);
    given(&mut left.join_style, &mut s.join_style);
    given(&mut left.border_level, &mut s.border_level);
    given(&mut left.extend_horizontal, &mut s.extend_horizontal);
    given(&mut left.extend_vertical, &mut s.extend_vertical);
    given(&mut left.round, &mut s.round);
    given(&mut left.softness_x, &mut s.softness_x);
    given(&mut left.softness_y, &mut s.softness_y);
    given(&mut left.shading_offset, &mut s.offset);
    given(&mut left.priority, &mut s.priority);
    Ok(())
}

fn probed<K: Keyed>(k: &mut K) -> Vec<EffectValue> {
    let values: Vec<EffectValue> = (EffectValue::ALL.into_iter())
        .filter(|v| k.takes(*v))
        .collect();
    for v in &values {
        k.write(*v, f64::NAN);
    }
    values
}

fn named<K: Keyed>(values: Vec<EffectValue>, probe: &K) -> Vec<EffectValue> {
    (values.into_iter())
        .filter(|v| probe.takes(*v) && !probe.plain(*v).is_nan())
        .collect()
}

fn named_values(n: &Node, change: &NodeChange) -> Result<(Vec<EffectValue>, Vec<EffectValue>)> {
    let k = element_of(&mut change.clone())?;
    let mut probe = n.clone();
    let values = probed(&mut probe);
    let parts = match &mut probe.tool {
        Tool::TextPlus(t) => probed(t.element_mut(k)),
        _ => Vec::new(),
    };
    let probe = applied(&probe, change)?;
    let shaded = match &probe.tool {
        Tool::TextPlus(t) => named(parts, &t.element(k)),
        _ => Vec::new(),
    };
    Ok((named(values, &probe), shaded))
}

fn keyed_into<K: Keyed>(
    was: &K,
    next: &mut K,
    named: &[EffectValue],
    (name, frame): (&str, Option<i64>),
) -> Result<()> {
    let keyed: Vec<EffectValue> = was.key_list().iter().map(|k| k.value).collect();
    for value in keyed.into_iter().filter(|v| named.contains(v)) {
        let Some(frame) = frame else {
            return Err(Error::usage(format!(
                "{name}'s {} has keyframes: give `frame` for the keyframe",
                value.name().replace('_', " ")
            )));
        };
        let to = next.point(value);
        if to != was.point_at(value, frame as f64) {
            next.put_point(value, frame, to);
        }
        next.write_point(value, was.point(value));
    }
    Ok(())
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
    left_point(p, id, name, &mut change, frame)?;
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
    let mut next = applied(n, change)?;
    let (named, shaded) = named_values(n, change)?;
    keyed_into(&*n, &mut next, &named, (name, frame))?;
    if let (Tool::TextPlus(was), Tool::TextPlus(now)) = (&n.tool, &mut next.tool) {
        let k = change.element.unwrap_or(1);
        keyed_into(&was.element(k), now.element_mut(k), &shaded, (name, frame))?;
    }
    *n = node_checked(&next)?;
    if change.remove_pattern {
        unhooked(c, name, pattern);
    }
    tidied(p, id)?;
    Ok(())
}

fn left_point(
    p: &Project,
    id: &str,
    name: &str,
    change: &mut NodeChange,
    frame: Option<i64>,
) -> Result<()> {
    if change.connect != Some(None) || change.values.center.is_some() {
        return Ok(());
    }
    let Some(c) = p.clip(id) else {
        return Ok(());
    };
    let comp = c.picture.as_ref().and_then(|x| x.composition.as_ref());
    let Some(k) = comp
        .and_then(|x| x.node(name))
        .and_then(|n| n.connect.as_ref())
    else {
        return Ok(());
    };
    let Some(frame) = frame else {
        return Err(Error::usage(format!(
            "{name} stops following {}: give `frame`, whose point its center keeps",
            k.tracker
        )));
    };
    let at = Frames::of(p, c).file(frame) as f64;
    let to = comp
        .and_then(|x| x.pattern(k))
        .and_then(|t| t.offset_position_at(at));
    if let Some([x, y]) = to {
        change.values.center = Some(vec![x, y]);
    }
    Ok(())
}

fn unhooked(c: &mut Composition, tracker: &str, gone: u32) {
    for n in &mut c.nodes {
        let Some(k) = n.connect.as_mut().filter(|k| k.tracker == tracker) else {
            continue;
        };
        match k.pattern.cmp(&gone) {
            std::cmp::Ordering::Equal => n.connect = None,
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
    let Some(m) = path_of(p, id, name, k)? else {
        return Ok(());
    };
    let samples = m.samples.clone();
    let at = match (trim, frame) {
        (PatternTrim::All, _) => None,
        (_, Some(frame)) => p.clip(id).map(|c| Frames::of(p, c).file(frame)),
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
    let edge = at.and_then(|at| m.base_at(at as f64).map(|a| sample_of(at, &a)));
    match clear {
        Some(span) if left(span) => {
            let edge = edge.as_slice();
            let to = super::motion::PathChange {
                clear: Some(span),
                samples: Some(edge).filter(|e| !e.is_empty()),
            };
            super::motion::set(p, id, (name, k), to)?;
        }
        None if trim != PatternTrim::All => {}
        _ => super::motion::remove(p, id, name, Some(&[k]))?,
    }
    Ok(())
}

fn path_of(p: &Project, id: &str, name: &str, k: u32) -> Result<Option<MotionPath>> {
    let c = p
        .clip(id)
        .ok_or_else(|| Error::not_found(format!("no clip {id}")))?;
    Ok(super::motion::pattern_of(c, name, k)?.path.clone())
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
    let k = change.pattern.unwrap_or(1);
    let Some(m) = path_of(p, id, name, k).ok().flatten() else {
        return Ok(());
    };
    let Some((first, last)) = m.span() else {
        return Ok(());
    };
    let Some(source) = p.clip(id).map(|c| Frames::of(p, c).file(frame)) else {
        return Ok(());
    };
    let source = if m.tracked {
        source.clamp(first, last)
    } else {
        source
    };
    let Some(now) = m.base_at(source as f64) else {
        return Ok(());
    };
    let near = [source - 1, source + 1]
        .into_iter()
        .filter(|f| (first..=last).contains(f) && !m.samples.iter().any(|s| s.frame == *f));
    let mut samples: Vec<PathSample> = near
        .filter_map(|f| Some(sample_of(f, &m.area_at(f as f64)?)))
        .collect();
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
    samples.push(sample);
    samples.sort_by_key(|s| s.frame);
    let to = super::motion::PathChange {
        samples: Some(&samples),
        ..Default::default()
    };
    super::motion::set(p, id, (name, k), to)?;
    change.pattern_center = None;
    change.pattern_width = None;
    change.pattern_height = None;
    Ok(())
}

pub(super) fn sample_of(frame: i64, a: &crate::motion::Area) -> PathSample {
    let round = |v: f64| (v * 1e5).round() / 1e5;
    PathSample {
        frame,
        center: a.center.map(round),
        size: a.size.map(round),
        rotation: a.rotation,
    }
}

pub fn key(
    p: &mut Project,
    id: &str,
    (name, element): (&str, Option<u8>),
    frame: i64,
    (value, ease): (Option<EffectValue>, Option<Ease>),
) -> Result<Value> {
    let c = kept(p, id)?;
    let i = found(c, id, name)?;
    let n = &mut c.nodes[i];
    let label = n.tool.label().to_string();
    let keyed = match element {
        None => toggled(n, (name, &label), frame, (value, ease))?,
        Some(k) => {
            let Tool::TextPlus(t) = &mut n.tool else {
                return Err(Error::invalid(format!(
                    "{name} ({label}) has no shading elements"
                )));
            };
            if !(1..=ShadingElement::ELEMENTS).contains(&k) {
                return Err(Error::invalid("a text's shading elements are 1 to 8"));
            }
            let what = format!("{name}'s shading element {k}");
            toggled(t.element_mut(k), (&what, &label), frame, (value, ease))?
        }
    };
    *n = node_checked(n)?;
    let mut answer = json!({ "clip": id, "node": name, "frame": frame, "keyed": keyed });
    if let Some(k) = element {
        answer["element"] = json!(k);
    }
    Ok(answer)
}

fn toggled<K: Keyed>(
    n: &mut K,
    (name, label): (&str, &str),
    frame: i64,
    (value, ease): (Option<EffectValue>, Option<Ease>),
) -> Result<bool> {
    if n.values().is_empty() {
        return Err(Error::invalid(format!(
            "{name} ({label}) has no values to key"
        )));
    }
    if let Some(v) = value
        && !n.values().contains(&v)
    {
        return Err(Error::invalid(format!(
            "{name} ({label}) has no {}",
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
                "{name} has no keyframes on frame {frame}"
            )));
        }
        (None, Some(v)) => n.toggle_value_key(v, frame),
        (None, None) => n.toggle_keys(frame),
    };
    Ok(keyed)
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
    *n = Node {
        name: n.name.clone(),
        tool: n.tool.made(),
        input: n.input.take(),
        foreground: n.foreground.take(),
        effect_mask: n.effect_mask.take(),
        pass_through: n.pass_through,
        connect: n.connect.take(),
        ..Node::new(&n.name, Tool::MediaIn, n.place)
    };
    Ok(())
}
