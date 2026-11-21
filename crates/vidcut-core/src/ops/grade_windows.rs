use crate::error::{Error, Result};
use crate::model::{Grade, GradeNode, GradeWindow, Project, WindowKind, WindowShape};

use super::grade::{GradeOf, edited, unlocked};

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
        d.windows.push(GradeWindow::new(kind, frame));
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
        (WindowShape::Circle { soft } | WindowShape::Gradient { soft }, Some(given)) => {
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
                if given.len() < 3 {
                    return Err(Error::invalid("a polygon takes three `points` or more"));
                }
                points.clone_from(given);
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
        ..WindowChange::default()
    };
    changed(&mut w.clone(), &all)
}

pub fn set(p: &mut Project, of: &GradeOf, n: u32, i: usize, c: &WindowChange) -> Result<()> {
    if *c == WindowChange::default() {
        return Err(Error::usage("give a value to change"));
    }
    edited(p, of, |g| changed(window(corrector(g, n)?, n, i)?, c))
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
        d.windows.remove(i);
        Ok(())
    })
}
