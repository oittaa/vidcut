use crate::error::{Error, Result};
use crate::model::{Clip, Grade, GradeNode, Project, SharedNode, Track};
use crate::request::GradeAdd;
use crate::shared::SharedVec;

use super::grade::{self, GradeOf};

fn found(p: &Project, number: u64) -> Result<&SharedNode> {
    (p.shared_nodes.iter())
        .find(|s| s.number == number)
        .ok_or_else(|| Error::not_found(format!("no shared node {number}")))
}

fn clip_only(of: &GradeOf) -> Result<()> {
    match of {
        GradeOf::Timeline => Err(Error::invalid(
            "a shared node is in clip grades, not in the timeline grade",
        )),
        GradeOf::Clip(..) => Ok(()),
    }
}

pub fn share(p: &mut Project, of: &GradeOf, n: u32) -> Result<u64> {
    clip_only(of)?;
    let g = grade::stored(p, of)?;
    let d = g.node(n).ok_or_else(|| grade::no_node(n))?;
    if d.tool.is_mixer() {
        return Err(Error::invalid(format!("node {n} is a mixer")));
    }
    if let Some(s) = d.shared {
        return Err(Error::invalid(format!(
            "node {n} is already shared node {s}"
        )));
    }
    let number = p.next_id.shared;
    p.next_id.shared += 1;
    let node = GradeNode {
        locked: true,
        ..d.shared_part()
    };
    p.shared_nodes.push(SharedNode { number, node });
    grade::edited(p, of, |g| {
        let d = grade::node(g, n)?;
        d.shared = Some(number);
        d.locked = true;
        Ok(())
    })?;
    Ok(number)
}

pub fn unshare(p: &mut Project, of: &GradeOf, n: u32) -> Result<()> {
    grade::edited(p, of, |g| {
        let d = grade::node(g, n)?;
        if d.shared.take().is_none() {
            return Err(Error::invalid(format!("node {n} is not a shared node")));
        }
        d.locked = false;
        Ok(())
    })
}

pub fn add(p: &mut Project, of: &GradeOf, after: u32, number: u64) -> Result<u32> {
    clip_only(of)?;
    let part = found(p, number)?.node.clone();
    let added = grade::add(p, of, GradeAdd::Serial, (Some(after), None), None)?;
    let made = (added["node"].as_u64())
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| Error::internal("the new node has no number"))?;
    grade::edited(p, of, |g| {
        let d = grade::node(g, made)?;
        d.take_shared(&part);
        d.shared = Some(number);
        Ok(())
    })?;
    Ok(made)
}

pub fn remove(p: &mut Project, number: u64) -> Result<()> {
    found(p, number)?;
    p.shared_nodes.retain(|s| s.number != number);
    let frame = p.frame_size();
    each_copy(
        p,
        |d| d.shared == Some(number),
        |d| {
            let mut fresh = GradeNode::corrector(None, d.place, frame);
            fresh.inputs = std::mem::take(&mut d.inputs);
            fresh.key_from = d.key_from;
            fresh.label = std::mem::take(&mut d.label);
            *d = fresh;
        },
    );
    Ok(())
}

pub(super) fn took(p: &mut Project, was: &Grade, now: &mut Grade) {
    let had = |n: u64| was.every_node().any(|d| d.shared == Some(n));
    let changes: Vec<(usize, GradeNode)> = (now.every_node())
        .filter_map(|d| {
            let n = d.shared.filter(|&n| had(n))?;
            let i = p.shared_nodes.iter().position(|s| s.number == n)?;
            let part = d.shared_part();
            (p.shared_nodes[i].node != part).then_some((i, part))
        })
        .collect();
    if changes.is_empty() {
        return;
    }
    for (i, part) in changes {
        p.shared_nodes[i].node = part;
    }
    synced(p);
    current(p, now);
}

pub(super) fn current(p: &Project, g: &mut Grade) {
    for d in g.every_node_mut().filter(|d| d.shared.is_some()) {
        let shared = (p.shared_nodes.iter()).find(|s| Some(s.number) == d.shared);
        match shared {
            Some(s) if s.node != d.shared_part() => d.take_shared(&s.node),
            Some(_) => {}
            None => d.shared = None,
        }
    }
}

pub fn synced(p: &mut Project) {
    let registry = std::mem::take(&mut p.shared_nodes);
    let part = |n: u64| (registry.iter()).find(|s| s.number == n).map(|s| &s.node);
    let stale =
        |d: &GradeNode| (d.shared).is_some_and(|n| part(n).is_none_or(|s| *s != d.shared_part()));
    each_copy(p, stale, |d| match d.shared.and_then(part) {
        Some(s) => d.take_shared(s),
        None => d.shared = None,
    });
    p.shared_nodes = registry;
}

fn has(c: &Clip, pick: &impl Fn(&GradeNode) -> bool) -> bool {
    let g = c.picture.as_ref().and_then(|x| x.grade.as_ref());
    g.is_some_and(|g| g.every_node().any(pick))
}

fn fixed(
    tracks: &mut SharedVec<Track>,
    pick: &impl Fn(&GradeNode) -> bool,
    fix: &mut impl FnMut(&mut GradeNode),
) {
    for i in 0..tracks.len() {
        if !tracks[i].clips.iter().any(|c| has(c, pick)) {
            continue;
        }
        for c in tracks[i].clips.iter_mut().filter(|c| has(c, pick)) {
            let g = c.picture.as_mut().and_then(|x| x.grade.as_mut());
            g.into_iter()
                .flat_map(Grade::every_node_mut)
                .filter(|d| pick(d))
                .for_each(&mut *fix);
        }
    }
}

fn each_copy(
    p: &mut Project,
    pick: impl Fn(&GradeNode) -> bool,
    mut fix: impl FnMut(&mut GradeNode),
) {
    let any = |tracks: &[Track]| {
        (tracks.iter())
            .flat_map(|t| t.clips.iter())
            .any(|c| has(c, &pick))
    };
    if any(&p.tracks) {
        fixed(&mut p.tracks, &pick, &mut fix);
    }
    for i in 0..p.timelines.len() {
        if any(&p.timelines[i].tracks) {
            fixed(&mut p.timelines[i].tracks, &pick, &mut fix);
        }
    }
    for i in 0..p.compounds.len() {
        if any(&p.compounds[i].tracks) {
            fixed(&mut p.compounds[i].tracks, &pick, &mut fix);
        }
    }
}
