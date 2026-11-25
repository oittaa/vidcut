use crate::error::{Error, Result};
use crate::model::{
    CurvePoint, DynamicsProfile, Grade, GradeNode, GradeWindow, Project, WindowKind, WindowShape,
};
use crate::window_key::{
    CIRCLE_RADIUS, CURVE_HANDLE, PAN_STEP, aspect_shrink, circle_scale, linear_scale, linear_scales,
};

use super::grade::{
    GradeOf, auto_keyed, changed_node, edited, frame_of, no_node, stored, unlocked,
};

pub const POLYGON_LEAST: usize = 3;
pub const POLYGON_MOST: usize = 128;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct WindowChange {
    pub on: Option<bool>,
    pub name: Option<String>,
    pub invert: Option<bool>,
    pub mask: Option<bool>,
    pub size: Option<f64>,
    pub aspect: Option<f64>,
    pub pan: Option<f64>,
    pub tilt: Option<f64>,
    pub rotate: Option<f64>,
    pub opacity: Option<f64>,
    pub soft: Option<Vec<f64>>,
    pub inside: Option<f64>,
    pub outside: Option<f64>,
    pub points: Option<Vec<[f64; 2]>>,
    pub inner: Option<Vec<[f64; 2]>>,
    pub outer: Option<Vec<[f64; 2]>>,
}

fn corrector(g: &mut Grade, n: u32) -> Result<&mut GradeNode> {
    let d = unlocked(g, n)?;
    if d.tool.is_mixer() {
        return Err(Error::invalid(format!(
            "node {n} is a mixer, which has no windows"
        )));
    }
    Ok(d)
}

fn window(d: &mut GradeNode, n: u32, i: usize) -> Result<&mut GradeWindow> {
    let count = d.windows.len();
    d.windows.get_mut(i).ok_or_else(|| {
        Error::not_found(format!(
            "node {n} has {count} windows: there is no window {i}"
        ))
    })
}

pub fn add(p: &mut Project, of: &GradeOf, n: u32, kind: WindowKind) -> Result<usize> {
    let frame = p.frame_size();
    let mut made = 0;
    edited(p, of, |g| {
        let d = corrector(g, n)?;
        for list in d.window_lists_mut() {
            list.push(GradeWindow::new(kind, frame));
        }
        made = d.windows.len() - 1;
        Ok(())
    })?;
    Ok(made)
}

fn in_range(name: &str, v: f64, (lo, hi): (f64, f64)) -> Result<f64> {
    match v.is_finite() && v >= lo && v <= hi {
        true => Ok(v),
        false => Err(Error::invalid(format!(
            "`{name}` is {v}: give a value from {lo} to {hi}"
        ))),
    }
}

fn changed(w: &mut GradeWindow, c: &WindowChange) -> Result<()> {
    let soft_range = (0.0, f64::MAX);
    if let Some(v) = c.size {
        w.size = in_range("size", v, (0.0, f64::MAX))?;
    }
    if let Some(v) = c.aspect {
        w.aspect = in_range("aspect", v, (0.0, 100.0))?;
    }
    if let Some(v) = c.opacity {
        w.opacity = in_range("opacity", v, (0.0, 100.0))?;
    }
    for (name, v, to) in [
        ("pan", c.pan, &mut w.pan),
        ("tilt", c.tilt, &mut w.tilt),
        ("rotate", c.rotate, &mut w.rotate),
    ] {
        if let Some(v) = v {
            *to = in_range(name, v, (f64::MIN, f64::MAX))?;
        }
    }
    w.on = c.on.unwrap_or(w.on);
    w.invert = c.invert.unwrap_or(w.invert);
    w.mask = c.mask.unwrap_or(w.mask);
    if let Some(name) = &c.name {
        w.name.clone_from(name);
    }
    let wrong = |field: &str| Error::invalid(format!("this window has no `{field}`"));
    match (&mut w.shape, &c.soft) {
        (_, None) => {}
        (WindowShape::Linear { soft, .. }, Some(given)) => {
            let given: [f64; 4] = given.as_slice().try_into().map_err(|_| {
                Error::invalid("a linear window takes four `soft` values: left, top, right, bottom")
            })?;
            for (i, v) in given.into_iter().enumerate() {
                soft[i] = in_range("soft", v, soft_range)?;
            }
        }
        (
            WindowShape::Circle { soft }
            | WindowShape::Curve { soft, .. }
            | WindowShape::Gradient { soft },
            Some(given),
        ) => {
            let [v] = given.as_slice() else {
                return Err(Error::invalid("this window takes one `soft` value"));
            };
            *soft = in_range("soft", *v, soft_range)?;
        }
        (WindowShape::Polygon { .. }, Some(_)) => return Err(wrong("soft")),
    }
    if c.points.iter().flatten().flatten().any(|v| !v.is_finite()) {
        return Err(Error::invalid("each point takes two numbers"));
    }
    let curve = matches!(w.shape, WindowShape::Curve { .. });
    for (field, given) in [("inner", c.inner.is_some()), ("outer", c.outer.is_some())] {
        if given && !curve {
            return Err(wrong(field));
        }
    }
    match &mut w.shape {
        WindowShape::Polygon {
            points,
            inside,
            outside,
        } => {
            if let Some(v) = c.inside {
                *inside = in_range("inside", v, soft_range)?;
            }
            if let Some(v) = c.outside {
                *outside = in_range("outside", v, soft_range)?;
            }
            if let Some(given) = &c.points {
                if !(POLYGON_LEAST..=POLYGON_MOST).contains(&given.len()) {
                    return Err(Error::invalid(format!(
                        "a polygon takes {POLYGON_LEAST} to {POLYGON_MOST} `points`"
                    )));
                }
                points.clone_from(given);
            }
        }
        WindowShape::Curve {
            points,
            inside,
            outside,
            inner,
            outer,
            ..
        } => {
            if let Some(v) = c.inside {
                *inside = in_range("inside", v, soft_range)?;
                *inner = None;
            }
            if let Some(v) = c.outside {
                *outside = in_range("outside", v, soft_range)?;
                *outer = None;
            }
            if let Some(given) = &c.points {
                if given.len() % 3 != 0 {
                    return Err(Error::invalid(
                        "a curve takes three `points` for each point: the in handle, the point, the out handle",
                    ));
                }
                *points = given.as_chunks::<3>().0.to_vec();
            }
            for (name, given, to) in [("inner", &c.inner, inner), ("outer", &c.outer, outer)] {
                if let Some(given) = given {
                    if given.iter().flatten().any(|v| !v.is_finite()) {
                        return Err(Error::invalid(format!(
                            "each `{name}` move takes two numbers"
                        )));
                    }
                    *to = Some(given.clone());
                }
                if to.as_ref().is_some_and(|m| m.len() != points.len()) {
                    match given {
                        Some(_) => {
                            return Err(Error::invalid(format!(
                                "`{name}` takes one move for each point of the curve"
                            )));
                        }
                        None => *to = None,
                    }
                }
            }
        }
        WindowShape::Linear { points, .. } => {
            if let Some(given) = &c.points {
                *points = given.as_slice().try_into().map_err(|_| {
                    Error::invalid("a linear window takes four `points`: bottom left, top left, top right, bottom right")
                })?;
            }
            no_inside_outside(c, &wrong)?;
        }
        _ => {
            if c.points.is_some() {
                return Err(wrong("points"));
            }
            no_inside_outside(c, &wrong)?;
        }
    }
    Ok(())
}

fn no_inside_outside(c: &WindowChange, wrong: &dyn Fn(&str) -> Error) -> Result<()> {
    for (field, given) in [
        ("inside", c.inside.is_some()),
        ("outside", c.outside.is_some()),
    ] {
        if given {
            return Err(wrong(field));
        }
    }
    Ok(())
}

pub(super) fn checked(w: &GradeWindow) -> Result<()> {
    let (soft, inside, outside, points) = match &w.shape {
        WindowShape::Linear { soft, points } => {
            (Some(soft.to_vec()), None, None, Some(points.to_vec()))
        }
        WindowShape::Circle { soft } | WindowShape::Gradient { soft } => {
            (Some(vec![*soft]), None, None, None)
        }
        WindowShape::Polygon {
            points,
            inside,
            outside,
        } => (None, Some(*inside), Some(*outside), Some(points.clone())),
        WindowShape::Curve {
            points,
            soft,
            inside,
            outside,
            ..
        } => (
            Some(vec![*soft]),
            Some(*inside),
            Some(*outside),
            Some(points.iter().flatten().copied().collect()),
        ),
    };
    let (inner, outer) = match &w.shape {
        WindowShape::Curve { inner, outer, .. } => (inner.clone(), outer.clone()),
        _ => (None, None),
    };
    let all = WindowChange {
        size: Some(w.size),
        aspect: Some(w.aspect),
        pan: Some(w.pan),
        tilt: Some(w.tilt),
        rotate: Some(w.rotate),
        opacity: Some(w.opacity),
        soft,
        inside,
        outside,
        points,
        inner,
        outer,
        ..WindowChange::default()
    };
    changed(&mut w.clone(), &all)
}

fn keyed(
    g: &mut Grade,
    (n, i): (u32, usize),
    (frame, profile): (Option<i64>, DynamicsProfile),
    change: impl FnOnce(&mut GradeWindow) -> Result<()>,
) -> Result<()> {
    window(corrector(g, n)?, n, i)?;
    auto_keyed(g, n, frame, profile);
    let (d, frame) = changed_node(g, n, frame)?;
    let mut now = d.windows_changed_at(frame)[i].clone();
    change(&mut now)?;
    d.change_window_at(frame, i, |_| now)
        .ok_or_else(|| no_node(n))
}

pub fn set(
    p: &mut Project,
    of: &GradeOf,
    (n, i): (u32, usize),
    c: &WindowChange,
    at: Option<i64>,
) -> Result<()> {
    if *c == WindowChange::default() {
        return Err(Error::usage("give a value to change"));
    }
    let frame = at.map(|t| frame_of(p, of, t)).transpose()?;
    let profile = p.settings.dynamics_profile;
    edited(p, of, |g| {
        keyed(g, (n, i), (frame, profile), |w| changed(w, c))
    })
}

pub fn copy(
    p: &Project,
    of: &GradeOf,
    (n, i): (u32, usize),
    at: Option<i64>,
) -> Result<GradeWindow> {
    let frame = at.map(|t| frame_of(p, of, t)).transpose()?;
    let g = stored(p, of)?;
    let d = g.node(n).ok_or_else(|| no_node(n))?;
    if d.tool.is_mixer() {
        return Err(Error::invalid(format!(
            "node {n} is a mixer, which has no windows"
        )));
    }
    let count = d.windows.len();
    let shown = frame.map_or(&d.windows[..], |f| d.windows_changed_at(f));
    shown.get(i).cloned().ok_or_else(|| {
        Error::not_found(format!(
            "node {n} has {count} windows: there is no window {i}"
        ))
    })
}

pub fn paste(
    p: &mut Project,
    of: &GradeOf,
    n: u32,
    (to, append): (Option<usize>, bool),
    copied: &GradeWindow,
    at: Option<i64>,
) -> Result<usize> {
    checked(copied)?;
    let frame = at.map(|t| frame_of(p, of, t)).transpose()?;
    let profile = p.settings.dynamics_profile;
    let mut made = 0;
    edited(p, of, |g| {
        let d = corrector(g, n)?;
        let onto = match (append, to) {
            (true, _) => None,
            (false, Some(i)) => Some(i),
            (false, None) => {
                return Err(Error::usage(
                    "give `window`, the window that takes the copy, or `append`",
                ));
            }
        };
        if let Some(i) = onto
            && window(d, n, i)?.shape.kind() == copied.shape.kind()
        {
            made = i;
            return keyed(g, (n, i), (frame, profile), |w| {
                *w = GradeWindow {
                    on: w.on,
                    name: std::mem::take(&mut w.name),
                    ..copied.clone()
                };
                Ok(())
            });
        }
        for list in d.window_lists_mut() {
            list.push(GradeWindow {
                on: true,
                name: String::new(),
                ..copied.clone()
            });
        }
        made = d.windows.len() - 1;
        Ok(())
    })?;
    Ok(made)
}

pub fn remove(p: &mut Project, of: &GradeOf, n: u32, i: usize) -> Result<()> {
    edited(p, of, |g| {
        let d = corrector(g, n)?;
        window(d, n, i)?;
        if i < WindowKind::ALL.len() {
            return Err(Error::invalid(format!(
                "window {i} is one of the first windows of node {n}, which stay: turn it off with `on`"
            )));
        }
        for list in d.window_lists_mut() {
            list.remove(i);
        }
        Ok(())
    })
}

fn curve_points(w: &GradeWindow, [fw, fh]: [f64; 2]) -> Result<(Vec<CurvePoint>, [f64; 2])> {
    let k = linear_scale(w.size);
    let sharp = |p: [f64; 2]| [p, p, p];
    match &w.shape {
        WindowShape::Circle { .. } => {
            let shrink = aspect_shrink(w.aspect);
            if shrink.contains(&0.0) {
                return Err(Error::invalid(
                    "a circle at aspect 0 or 100 is a band, which has no outline",
                ));
            }
            let r = CIRCLE_RADIUS * fw * circle_scale(w.size) / k;
            let [a, b] = [r / shrink[0] / fw, r / shrink[1] / fh];
            let [ka, kb] = [CURVE_HANDLE * a, CURVE_HANDLE * b];
            let points = vec![
                [[-ka, b], [0.0, b], [ka, b]],
                [[a, kb], [a, 0.0], [a, -kb]],
                [[ka, -b], [0.0, -b], [-ka, -b]],
                [[-a, -kb], [-a, 0.0], [-a, kb]],
            ];
            Ok((points, [0.0, 0.0]))
        }
        WindowShape::Linear { points, .. } => {
            let n = points.len() as f64;
            let [mx, my] =
                (points.iter()).fold([0.0, 0.0], |[x, y], [px, py]| [x + px / n, y + py / n]);
            let [sx, sy] = linear_scales(w.size, w.aspect);
            let corners = points
                .iter()
                .map(|[px, py]| sharp([(px - mx) * sx / k, (py - my) * sy / k]));
            Ok((corners.collect(), [mx * fw, my * fh]))
        }
        WindowShape::Polygon { points, .. } => {
            Ok((points.iter().map(|p| sharp(*p)).collect(), [0.0, 0.0]))
        }
        WindowShape::Curve { .. } | WindowShape::Gradient { .. } => Err(Error::invalid(
            "only a circle, a linear window or a polygon turns into a curve",
        )),
    }
}

pub fn convert(p: &mut Project, of: &GradeOf, n: u32, i: usize) -> Result<usize> {
    let frame = p.frame_size();
    let mut made = 0;
    edited(p, of, |g| {
        let d = corrector(g, n)?;
        window(d, n, i)?;
        let curve = |w: &GradeWindow| -> Result<GradeWindow> {
            let (points, [dx, dy]) = curve_points(w, frame)?;
            Ok(GradeWindow {
                shape: WindowShape::Curve {
                    points,
                    soft: 0.0,
                    inside: 0.0,
                    outside: 0.0,
                    inner: None,
                    outer: None,
                },
                on: true,
                name: String::new(),
                aspect: GradeWindow::new(WindowKind::Curve, frame).aspect,
                pan: w.pan + dx / PAN_STEP,
                tilt: w.tilt + dy / PAN_STEP,
                ..w.clone()
            })
        };
        let curves: Vec<GradeWindow> = (d.window_lists_mut())
            .map(|list| curve(&list[i]))
            .collect::<Result<_>>()?;
        for (list, curve) in d.window_lists_mut().zip(curves) {
            match i < WindowKind::ALL.len() {
                true => list[i].on = false,
                false => {
                    list.remove(i);
                }
            }
            list.push(curve);
        }
        made = d.windows.len() - 1;
        Ok(())
    })?;
    Ok(made)
}
