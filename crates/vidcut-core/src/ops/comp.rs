use serde_json::{Value, json};

use crate::composition::cleaned;
use crate::effect_keys::Keyed;
use crate::error::{Error, Result};
use crate::model::{Composition, Ease, EffectValue, Node, Project, Subject, Tool, ToolType};
use crate::request::{NodeChange, NodeInput};

use super::effect::{kind_applied, source_frame};
use super::locate;
use super::picture::{node_checked, takes};

fn comp_of<'a>(p: &'a mut Project, id: &str) -> Result<&'a mut Composition> {
    let (ti, ci) = locate(p, id)?;
    takes(p, ti, &p.tracks[ti].clips[ci])?;
    let c = &mut p.tracks[ti].clips[ci];
    if c.generator.is_some() {
        return Err(Error::invalid(format!(
            "{id} is a generator, which takes no composition"
        )));
    }
    let picture = c.picture.get_or_insert_with(Default::default);
    Ok(picture.composition.get_or_insert_with(Composition::plain))
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
    let picture = |a: &Node| a.tool.works() || a.tool == Tool::MediaIn;
    match after {
        Some(a) if node.tool.works() && picture(&a) => {
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

fn follow_checked(p: &Project, id: &str, named: &str) -> Result<()> {
    let has = p
        .clip(id)
        .is_some_and(|c| c.motion.iter().any(|m| m.id == named));
    match has || Subject::named(named).is_some() {
        true => Ok(()),
        false => Err(Error::not_found(format!(
            "{id} has no motion path {named} for a mask to follow"
        ))),
    }
}

pub fn copied(p: &mut Project, id: &str, name: &str) -> Result<Node> {
    let c = kept(p, id)?;
    Ok(c.nodes[found(c, id, name)?].clone())
}

pub fn paste(p: &mut Project, id: &str, mut node: Node, after: Option<&str>) -> Result<Value> {
    node = node_checked(&node)?;
    if let Some(named) = node.tool.mask().and_then(|m| m.follows.as_deref()) {
        follow_checked(p, id, named)?;
    }
    let c = comp_of(p, id)?;
    one_of(c, &node.tool, id)?;
    if c.node(&node.name).is_some() {
        node.name = c.copy_name(&node.name);
    }
    node.input = None;
    node.effect_mask = None;
    let pasted = node.name.clone();
    insert(c, node, after, id)?;
    tidied(p, id)?;
    Ok(json!({ "clip": id, "node": pasted }))
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
    }
    c.nodes[i].name = next.clone();
    Ok(json!({ "clip": id, "node": next }))
}

pub fn remove(p: &mut Project, id: &str, name: &str) -> Result<()> {
    let c = kept(p, id)?;
    let gone = c.nodes.remove(found(c, id, name)?);
    let passes = gone.tool.works();
    for n in &mut c.nodes {
        if n.input.as_deref() == Some(name) {
            n.input = gone.input.clone().filter(|_| passes);
        }
        if n.effect_mask.as_deref() == Some(name) {
            n.effect_mask = gone.effect_mask.clone().filter(|_| gone.tool.is_mask());
        }
    }
    tidied(p, id)
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
        taken(&mut left.blur_alpha, &mut next.blur_alpha);
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
            taken(&mut left.invert, &mut m.invert);
            taken(&mut left.solid, &mut m.solid);
            if let Some(follows) = left.follows.take() {
                m.follows = follows;
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
        Tool::MediaIn | Tool::MediaOut => {}
    }
    if left != NodeChange::default() {
        return Err(Error::invalid(format!(
            "comp.set has a value that a {} doesn't have",
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
    at: Option<i64>,
) -> Result<()> {
    if *change == NodeChange::default() {
        return Err(Error::usage("comp.set needs a value to change"));
    }
    if let Some(Some(named)) = &change.follows {
        follow_checked(p, id, named)?;
    }
    let frame = at.map(|t| source_frame(p, id, t)).transpose()?;
    let c = kept(p, id)?;
    let i = found(c, id, name)?;
    let n = &mut c.nodes[i];
    let mut next = applied(n, change)?;
    let named = named_values(n, change)?;
    for value in n.keys.iter().map(|k| k.value).filter(|v| named.contains(v)) {
        let Some(frame) = frame else {
            return Err(Error::usage(format!(
                "{name}'s {} has keyframes: give `at` for the keyframe",
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
    Ok(())
}

pub fn key(
    p: &mut Project,
    id: &str,
    name: &str,
    at: i64,
    (value, ease): (Option<EffectValue>, Option<Ease>),
) -> Result<Value> {
    let frame = source_frame(p, id, at)?;
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
                "{name} of {id} has no keyframes on source frame {frame}"
            )));
        }
        (None, Some(v)) => n.toggle_value_key(v, frame),
        (None, None) => n.toggle_keys(frame),
    };
    *n = node_checked(n)?;
    Ok(json!({ "clip": id, "node": name, "frame": frame, "keyed": keyed }))
}

pub fn reset(p: &mut Project, id: &str, name: &str) -> Result<()> {
    let c = kept(p, id)?;
    let i = found(c, id, name)?;
    let n = &mut c.nodes[i];
    let follows = n.tool.mask().and_then(|m| m.follows.clone());
    let mut tool = n.tool.made();
    if let Some(m) = tool.mask_mut() {
        m.follows = follows;
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
