//! The theme shown (`crate::themes`, deviation X22): egui's style and
//! fonts, the panels' frames and what is painted behind them, and the
//! colors everything else paints with.

use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Shape, Stroke, TextStyle, Visuals,
    pos2, vec2,
};

use crate::themes::{Buttons, Colors, Fonts, Style, Theme};

thread_local! {
    /// The theme shown. The GUI paints on one thread.
    static CURRENT: RefCell<Rc<Theme>> =
        RefCell::new(Rc::new(crate::themes::default_theme().clone()));
    /// The fonts in egui's font atlas, so a theme with the same ones
    /// doesn't build it again.
    static FONTS: RefCell<Option<Fonts>> = const { RefCell::new(None) };
}

fn current() -> Rc<Theme> {
    CURRENT.with(|t| t.borrow().clone())
}

/// The colors of the theme shown.
pub fn colors() -> Colors {
    CURRENT.with(|t| t.borrow().colors)
}

/// The corners of the timeline's clips.
pub fn clip_corners() -> CornerRadius {
    CornerRadius::same(current().shape.clips)
}

/// Shows `theme` from the next frame on.
pub fn apply(ctx: &egui::Context, theme: &Theme) {
    CURRENT.with(|t| *t.borrow_mut() = Rc::new(theme.clone()));
    if FONTS.with(|f| f.borrow().as_ref() != Some(&theme.fonts)) {
        ctx.set_fonts(font_definitions(&theme.fonts));
        FONTS.with(|f| *f.borrow_mut() = Some(theme.fonts.clone()));
    }
    // Both of egui's styles: the computer's light or dark setting doesn't
    // pick one.
    let style = Arc::new(style(theme));
    ctx.set_style_of(egui::Theme::Dark, style.clone());
    ctx.set_style_of(egui::Theme::Light, style);
}

fn style(t: &Theme) -> egui::Style {
    let (s, f) = (&t.shape, &t.fonts);
    let mut style = egui::Style {
        visuals: visuals(t),
        ..Default::default()
    };
    style.spacing.item_spacing = vec2(s.spacing[0], s.spacing[1]);
    style.spacing.button_padding = vec2(s.button_padding[0], s.button_padding[1]);
    style.spacing.interact_size.y = s.button_height;
    style.text_styles = [
        (TextStyle::Small, FontId::proportional(f.small)),
        (TextStyle::Body, FontId::proportional(f.size)),
        (TextStyle::Button, FontId::proportional(f.size)),
        (TextStyle::Heading, FontId::proportional(f.heading)),
        (TextStyle::Monospace, FontId::monospace(f.mono_size)),
    ]
    .into();
    style
}

fn visuals(t: &Theme) -> Visuals {
    let (c, s) = (&t.colors, &t.shape);
    let mut v = if t.light {
        Visuals::light()
    } else {
        Visuals::dark()
    };
    v.panel_fill = c.panel;
    v.window_fill = c.panel;
    v.extreme_bg_color = c.field;
    v.code_bg_color = c.field;
    v.faint_bg_color = c.faint;
    v.override_text_color = None;
    v.selection.bg_fill = c.selection;
    v.selection.stroke = Stroke::new(1.0, c.selected_text);
    v.hyperlink_color = c.accent;
    v.window_stroke = Stroke::new(1.0, c.line);
    v.window_corner_radius = CornerRadius::same(s.windows);
    v.menu_corner_radius = CornerRadius::same(s.menus);
    let i8 = |n: u8| i8::try_from(n).unwrap_or(i8::MAX);
    let [x, y, blur] = s.shadow;
    v.window_shadow = egui::Shadow {
        offset: [i8(x), i8(y)],
        blur,
        spread: 0,
        color: c.shadow,
    };
    v.popup_shadow = egui::Shadow {
        offset: [i8(x / 2), i8(y / 2)],
        blur: blur / 2,
        spread: 0,
        color: c.shadow,
    };
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = c.panel;
    w.noninteractive.weak_bg_fill = c.panel;
    w.noninteractive.bg_stroke = Stroke::new(1.0, c.line);
    w.noninteractive.fg_stroke = Stroke::new(1.0, c.text);
    w.noninteractive.corner_radius = CornerRadius::same(s.corners);
    for (state, fill) in [
        (&mut w.inactive, c.button),
        (&mut w.hovered, c.button_hovered),
        (&mut w.active, c.button_pressed),
        (&mut w.open, c.button_pressed),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::new(1.0, c.button_line);
        state.fg_stroke = Stroke::new(1.0, c.text);
        state.corner_radius = CornerRadius::same(s.corners);
    }
    // Check boxes keep `bg_fill`: they show a box in every style.
    match s.buttons {
        Buttons::Filled => w.inactive.bg_stroke = Stroke::NONE,
        Buttons::Outlined => w.inactive.weak_bg_fill = Color32::TRANSPARENT,
        Buttons::Flat => {
            w.inactive.weak_bg_fill = Color32::TRANSPARENT;
            w.inactive.bg_stroke = Stroke::NONE;
        }
    }
    v
}

/// The computer's own UI fonts, looked for in this order.
const SYSTEM_UI: &[&str] = if cfg!(windows) {
    &["Segoe UI Variable", "Segoe UI"]
} else if cfg!(target_os = "macos") {
    &[
        "SF Pro",
        "SF Pro Text",
        "System Font",
        ".SF NS",
        "Helvetica Neue",
    ]
} else {
    &[
        "Ubuntu Sans",
        "Adwaita Sans",
        "Cantarell",
        "Noto Sans",
        "DejaVu Sans",
    ]
};

/// The computer's own monospace fonts, looked for in this order.
const SYSTEM_MONO: &[&str] = if cfg!(windows) {
    &["Cascadia Mono", "Consolas"]
} else if cfg!(target_os = "macos") {
    &["SF Mono", "Menlo"]
} else {
    &[
        "Ubuntu Sans Mono",
        "Ubuntu Mono",
        "Adwaita Mono",
        "Noto Sans Mono",
        "DejaVu Sans Mono",
    ]
};

/// egui's fonts with the theme's in front, and Phosphor's icons mixed into
/// ordinary text. A theme that names fonts gets Geist or Geist Mono where
/// none of them is there. One that names none keeps egui's.
fn font_definitions(f: &Fonts) -> egui::FontDefinitions {
    let mut defs = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut defs, egui_phosphor::Variant::Regular);
    for (family, wanted, system, fallback) in [
        (FontFamily::Proportional, &f.ui, SYSTEM_UI, "Geist"),
        (FontFamily::Monospace, &f.mono, SYSTEM_MONO, "Geist Mono"),
    ] {
        if wanted.is_empty() {
            continue;
        }
        let names = wanted.iter().flat_map(|w| match w.as_str() {
            "system" => system.to_vec(),
            name => vec![name],
        });
        if let Some((name, data)) = names
            .chain([fallback])
            .find_map(|name| font(name).map(|data| (name, data)))
        {
            let key = format!("theme {name}");
            defs.font_data.insert(key.clone(), Arc::new(data));
            defs.families.entry(family).or_default().insert(0, key);
        }
    }
    defs
}

/// The font family `name`: one built into vidcut, else an installed one.
/// Its capitals sit in the middle of its lines, and a variable font shows
/// its regular weight.
fn font(name: &str) -> Option<egui::FontData> {
    let (data, index) = match built_in_font(name) {
        Some(data) => (Cow::Borrowed(data), 0),
        None => installed_font(name)?,
    };
    let tweak = egui::FontTweak {
        y_offset_factor: centering(&data, index),
        coords: egui::epaint::text::VariationCoords::new([(b"wght", 400.0)]),
        ..Default::default()
    };
    Some(egui::FontData {
        font: data,
        index,
        tweak,
    })
}

/// The fonts vidcut carries (OFL-1.1): the same on every computer.
fn built_in_font(name: &str) -> Option<&'static [u8]> {
    let files = match name {
        "Geist" => &shadcn_egui_font_geist::FONT,
        "Geist Mono" => &shadcn_egui_font_geist_mono::FONT,
        "Space Grotesk" => &shadcn_egui_font_space_grotesk::FONT,
        _ => return None,
    };
    Some(files.face(400).data)
}

/// An installed font family's file and the face's index in it.
fn installed_font(name: &str) -> Option<(Cow<'static, [u8]>, u32)> {
    static DATABASE: OnceLock<fontdb::Database> = OnceLock::new();
    let db = DATABASE.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        db
    });
    let id = db.query(&fontdb::Query {
        families: &[fontdb::Family::Name(name)],
        ..Default::default()
    })?;
    db.with_face_data(id, |data, index| (Cow::Owned(data.to_vec()), index))
}

/// How far down, in ems, a font's text moves for its capitals to sit in
/// the middle of its line: check boxes and buttons center the line, and
/// fonts place their capitals in it differently. Zero for a font that
/// doesn't say how tall its capitals are.
fn centering(data: &[u8], index: u32) -> f32 {
    use skrifa::MetadataProvider;
    let Ok(font) = skrifa::FontRef::from_index(data, index) else {
        return 0.0;
    };
    let m = font.metrics(
        skrifa::instance::Size::unscaled(),
        skrifa::instance::LocationRef::default(),
    );
    match m.cap_height {
        Some(cap) => capitals_down(
            m.ascent,
            m.descent,
            m.leading,
            cap,
            f32::from(m.units_per_em),
        ),
        None => 0.0,
    }
}

/// The line holds `ascent` above the baseline, `descent` (negative) and the
/// gap below it. Capitals stand `cap` tall on the baseline.
fn capitals_down(ascent: f32, descent: f32, gap: f32, cap: f32, em: f32) -> f32 {
    let line_middle = (ascent - descent + gap) / 2.0;
    let capitals_middle = ascent - cap / 2.0;
    (line_middle - capitals_middle) / em
}

/// The top and status bars' frame.
pub fn bar() -> egui::Frame {
    let t = current();
    let [x, y] = t.shape.bar_padding;
    egui::Frame::new()
        .fill(t.colors.background)
        .inner_margin(egui::Margin::symmetric(x, y))
}

/// A panel's frame, filled with `fill`. Cut corners paint their own.
pub fn card(fill: Color32) -> egui::Frame {
    let t = current();
    let s = &t.shape;
    let fill = match t.decoration.style {
        Style::Hud | Style::Steel => Color32::TRANSPARENT,
        Style::None | Style::Glow => fill,
    };
    egui::Frame::new()
        .fill(fill)
        .inner_margin(s.padding)
        .outer_margin(s.gap)
        .corner_radius(s.panels)
        .stroke(if s.panel_border {
            Stroke::new(1.0, t.colors.line)
        } else {
            Stroke::NONE
        })
}

/// What a decoration is painted behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deco {
    /// The top bar.
    Bar,
    Status,
    /// A panel in a [`card`] frame.
    Card,
    /// The viewer's panel.
    Viewer,
}

/// Shows `add` in `ui`, with the theme's decoration painted behind it.
pub fn decorated<R>(ui: &mut egui::Ui, deco: Deco, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let t = current();
    if t.decoration.style == Style::None {
        return add(ui);
    }
    // A place under what `add` paints, filled once its size is known.
    let slot = ui.painter().add(Shape::Noop);
    let max = ui.max_rect();
    let r = add(ui);
    let rect = match deco {
        Deco::Bar | Deco::Status => {
            let [x, y] = t.shape.bar_padding;
            Rect::from_x_y_ranges(max.x_range(), ui.min_rect().y_range())
                .expand2(vec2(f32::from(x), f32::from(y)))
        }
        Deco::Card | Deco::Viewer => max.expand(f32::from(t.shape.padding)),
    };
    ui.painter()
        .set(slot, Shape::Vec(decoration(&t, rect, deco)));
    r
}

/// Paints a glow in `color` along the playhead's `line`, where the theme
/// has one.
pub fn playhead_glow(painter: &egui::Painter, line: [Pos2; 2], color: Color32) {
    if current().decoration.style == Style::None {
        return;
    }
    for (width, alpha) in [(9.0, 22), (5.0, 50)] {
        painter.line_segment(line, Stroke::new(width, with_alpha(color, alpha)));
    }
}

fn with_alpha(c: Color32, a: u8) -> Color32 {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

fn decoration(t: &Theme, r: Rect, deco: Deco) -> Vec<Shape> {
    let d = &t.decoration;
    let mut out = Vec::new();
    match (d.style, deco) {
        (Style::None, _) => {}
        // Neon: glowing panels, a gradient under the top bar, a grid
        // behind the viewer's controls.
        (Style::Glow, Deco::Card) => glow(r, f32::from(t.shape.panels), d.color, &mut out),
        (Style::Glow, Deco::Viewer) => {
            grid(r.shrink(2.0), d.color, d.color2, &mut out);
            glow(r, f32::from(t.shape.panels), d.color2, &mut out);
        }
        (Style::Glow, Deco::Bar) => {
            let y = r.bottom() - 2.0;
            let (a, b) = (r.left(), r.right());
            out.push(band(
                y,
                a,
                b,
                7.0,
                with_alpha(d.color, 45),
                with_alpha(d.color2, 45),
            ));
            out.push(band(y, a, b, 2.0, d.color, d.color2));
        }
        (Style::Glow, Deco::Status) => out.push(band(
            r.top() + 1.0,
            r.left(),
            r.right(),
            1.0,
            with_alpha(d.color2, 140),
            with_alpha(d.color, 140),
        )),
        // Starship: glass panels with two corners cut and brackets on the
        // others, marks behind the picture, a ruler under the top bar.
        (Style::Hud, Deco::Card | Deco::Viewer) => {
            let viewer = deco == Deco::Viewer;
            let fill = if viewer {
                t.colors.panel_dark
            } else {
                t.colors.panel
            };
            let points = chamfer(r, d.cut, [true, false, true, false]);
            out.push(Shape::convex_polygon(
                points,
                fill,
                Stroke::new(1.0, with_alpha(d.color, 90)),
            ));
            brackets(
                r.shrink(3.0),
                16.0,
                d.color,
                [false, true, false, true],
                &mut out,
            );
            if viewer {
                marks(
                    r.shrink(f32::from(t.shape.padding)),
                    with_alpha(d.color, 26),
                    &mut out,
                );
                let s = Stroke::new(2.0, d.color2);
                out.push(Shape::line_segment(
                    [
                        pos2(r.left(), r.top() + d.cut),
                        pos2(r.left() + d.cut, r.top()),
                    ],
                    s,
                ));
                out.push(Shape::line_segment(
                    [
                        pos2(r.right(), r.bottom() - d.cut),
                        pos2(r.right() - d.cut, r.bottom()),
                    ],
                    s,
                ));
            }
        }
        (Style::Hud, Deco::Bar) => {
            let y = r.bottom() - 1.0;
            out.push(Shape::line_segment(
                [pos2(r.left(), y), pos2(r.right(), y)],
                Stroke::new(1.0, with_alpha(d.color, 110)),
            ));
            let ruler = r.left() + 220.0;
            out.push(Shape::line_segment(
                [pos2(r.left(), y), pos2(ruler, y)],
                Stroke::new(3.0, d.color),
            ));
            let mut x = ruler + 8.0;
            while x < ruler + 120.0 {
                out.push(Shape::line_segment(
                    [pos2(x, y - 3.0), pos2(x, y)],
                    Stroke::new(1.0, with_alpha(d.color, 150)),
                ));
                x += 8.0;
            }
            out.push(Shape::line_segment(
                [pos2(r.right() - 80.0, y), pos2(r.right(), y)],
                Stroke::new(3.0, d.color2),
            ));
        }
        (Style::Hud, Deco::Status) => out.push(Shape::line_segment(
            [r.left_top(), r.right_top()],
            Stroke::new(1.0, with_alpha(d.color, 80)),
        )),
        // Cybertruck: brushed steel slabs under a light bar.
        (Style::Steel, Deco::Card) => {
            let points = chamfer(r, d.cut, [true; 4]);
            out.push(shaded(&points, d.top, d.bottom));
            brushed(r, d.cut, &mut out);
            out.push(Shape::closed_line(points, Stroke::new(1.0, d.edge)));
        }
        (Style::Steel, Deco::Viewer) => out.push(Shape::convex_polygon(
            chamfer(r, d.cut, [true; 4]),
            t.colors.panel_dark,
            Stroke::new(1.0, d.edge),
        )),
        (Style::Steel, Deco::Bar) => {
            let y = r.bottom() - 3.0;
            let line = [pos2(r.left(), y), pos2(r.right(), y)];
            for (width, alpha) in [(16.0, 14), (8.0, 40), (4.0, 120)] {
                out.push(Shape::line_segment(
                    line,
                    Stroke::new(width, with_alpha(d.color, alpha)),
                ));
            }
            out.push(Shape::line_segment(line, Stroke::new(1.5, d.color)));
        }
        (Style::Steel, Deco::Status) => {}
    }
    out
}

/// `r` with its corners cut at 45°: top left, top right, bottom right,
/// bottom left.
fn chamfer(r: Rect, cut: f32, corners: [bool; 4]) -> Vec<Pos2> {
    let (l, t, rr, b) = (r.left(), r.top(), r.right(), r.bottom());
    let mut p = Vec::with_capacity(8);
    let mut corner = |on: bool, at: Pos2, before: Pos2, after: Pos2| {
        if on {
            p.extend([before, after]);
        } else {
            p.push(at);
        }
    };
    corner(corners[0], pos2(l, t), pos2(l, t + cut), pos2(l + cut, t));
    corner(
        corners[1],
        pos2(rr, t),
        pos2(rr - cut, t),
        pos2(rr, t + cut),
    );
    corner(
        corners[2],
        pos2(rr, b),
        pos2(rr, b - cut),
        pos2(rr - cut, b),
    );
    corner(corners[3], pos2(l, b), pos2(l + cut, b), pos2(l, b - cut));
    p
}

/// A convex polygon shaded from `top` at its top to `bottom` at its bottom.
fn shaded(points: &[Pos2], top: Color32, bottom: Color32) -> Shape {
    let mut mesh = egui::Mesh::default();
    let (y0, y1) = points
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.y), b.max(p.y)));
    let at = |y: f32| top.lerp_to_gamma(bottom, ((y - y0) / (y1 - y0).max(1.0)).clamp(0.0, 1.0));
    let center = points.iter().fold(egui::Vec2::ZERO, |s, p| s + p.to_vec2()) / points.len() as f32;
    mesh.colored_vertex(center.to_pos2(), at(center.y));
    for p in points {
        mesh.colored_vertex(*p, at(p.y));
    }
    let n = points.len() as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    Shape::mesh(mesh)
}

/// Faint lines across a slab with four cut corners, lighter and darker by
/// turns: brushed metal.
fn brushed(r: Rect, cut: f32, out: &mut Vec<Shape>) {
    let mut y = r.top() + 2.0;
    let mut i = 0u32;
    while y < r.bottom() - 1.0 {
        let inset = (cut - (y - r.top())).max(cut - (r.bottom() - y)).max(0.0) + 1.0;
        let n = i.wrapping_mul(2_654_435_761) >> 27;
        let alpha = 3 + (n % 7) as u8;
        let color = if n.is_multiple_of(2) {
            Color32::from_white_alpha(alpha)
        } else {
            Color32::from_black_alpha(alpha * 2)
        };
        out.push(Shape::line_segment(
            [pos2(r.left() + inset, y), pos2(r.right() - inset, y)],
            Stroke::new(1.0, color),
        ));
        y += 3.0;
        i += 1;
    }
}

/// A band across from `x0` to `x1` at `y`, `width` tall, shaded from `a`
/// on the left to `b` on the right.
fn band(y: f32, x0: f32, x1: f32, width: f32, a: Color32, b: Color32) -> Shape {
    let mut mesh = egui::Mesh::default();
    let h = width / 2.0;
    mesh.colored_vertex(pos2(x0, y - h), a);
    mesh.colored_vertex(pos2(x1, y - h), b);
    mesh.colored_vertex(pos2(x1, y + h), b);
    mesh.colored_vertex(pos2(x0, y + h), a);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    Shape::mesh(mesh)
}

/// Strokes around `r`, wider and fainter outward.
fn glow(r: Rect, radius: f32, color: Color32, out: &mut Vec<Shape>) {
    for (width, alpha) in [(10.0, 12), (6.0, 30), (3.0, 70), (1.2, 230)] {
        out.push(Shape::rect_stroke(
            r,
            radius,
            Stroke::new(width, with_alpha(color, alpha)),
            egui::StrokeKind::Middle,
        ));
    }
}

/// A floor of lines in perspective in the lower part of `r`, under a
/// horizon shaded from `a` to `b`.
fn grid(r: Rect, a: Color32, b: Color32, out: &mut Vec<Shape>) {
    let horizon = r.top() + r.height() * 0.58;
    for i in 1..=12 {
        let t = (i as f32 / 12.0).powi(2);
        let y = horizon + (r.bottom() - horizon) * t;
        out.push(Shape::line_segment(
            [pos2(r.left(), y), pos2(r.right(), y)],
            Stroke::new(1.0, with_alpha(a, (30.0 + 90.0 * t) as u8)),
        ));
    }
    let vanish = pos2(r.center().x, horizon);
    for i in -14..=14 {
        let x = r.center().x + i as f32 * r.width() / 9.0;
        out.push(Shape::line_segment(
            [vanish, pos2(x, r.bottom())],
            Stroke::new(1.0, with_alpha(a, 70)),
        ));
    }
    out.push(band(
        horizon,
        r.left(),
        r.right(),
        2.0,
        with_alpha(a, 200),
        with_alpha(b, 200),
    ));
}

/// L-shaped marks at the corners of `r` that `which` picks: top left, top
/// right, bottom right, bottom left.
fn brackets(r: Rect, len: f32, color: Color32, which: [bool; 4], out: &mut Vec<Shape>) {
    let s = Stroke::new(2.0, color);
    let corners = [
        (r.left_top(), 1.0, 1.0),
        (r.right_top(), -1.0, 1.0),
        (r.right_bottom(), -1.0, -1.0),
        (r.left_bottom(), 1.0, -1.0),
    ];
    for ((c, dx, dy), on) in corners.into_iter().zip(which) {
        if on {
            out.push(Shape::line(
                vec![c + vec2(0.0, dy * len), c, c + vec2(dx * len, 0.0)],
                s,
            ));
        }
    }
}

/// Small crosses on a grid over `r`.
fn marks(r: Rect, color: Color32, out: &mut Vec<Shape>) {
    let s = Stroke::new(1.0, color);
    let step = 48.0;
    let mut y = r.top() + step / 2.0;
    while y < r.bottom() {
        let mut x = r.left() + step / 2.0;
        while x < r.right() {
            out.push(Shape::line_segment([pos2(x - 3.0, y), pos2(x + 3.0, y)], s));
            out.push(Shape::line_segment([pos2(x, y - 3.0), pos2(x, y + 3.0)], s));
            x += step;
        }
        y += step;
    }
}

/// How Clip Color `name` looks (spec §3); none for a name it doesn't know.
pub fn clip_color(name: &str) -> Option<Color32> {
    vidcut_core::ops::CLIP_COLORS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, [r, g, b])| Color32::from_rgb(*r, *g, *b))
}

/// A Clip Color's name as shown: `Teal`.
pub fn color_title(name: &str) -> String {
    let mut title = name.to_string();
    if let Some(first) = title.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    title
}

/// The color all of `colors` have, if they have the same one.
pub fn same_color<'a>(mut colors: impl Iterator<Item = Option<&'a str>>) -> Option<&'a str> {
    let first = colors.next()??;
    colors.all(|c| c == Some(first)).then_some(first)
}

/// The Clip Color submenu (spec §3): each color, the `current` one
/// selected, then Clear Color. Returns the one picked, empty for Clear
/// Color.
pub fn clip_color_menu(ui: &mut egui::Ui, current: Option<&str>) -> Option<String> {
    let mut picked = None;
    for (name, [r, g, b]) in vidcut_core::ops::CLIP_COLORS {
        let title = color_title(name);
        let clicked = ui
            .horizontal(|ui| {
                let (swatch, _) =
                    ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                ui.painter().rect_filled(
                    swatch,
                    CornerRadius::same(2),
                    Color32::from_rgb(*r, *g, *b),
                );
                ui.add(egui::Button::selectable(current == Some(*name), title))
                    .clicked()
            })
            .inner;
        if clicked {
            picked = Some(name.to_string());
        }
    }
    ui.separator();
    if ui.button("Clear Color").clicked() {
        picked = Some(String::new());
    }
    picked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clips_of_one_color_show_it_in_the_menu() {
        assert_eq!(
            same_color([Some("teal"), Some("teal")].into_iter()),
            Some("teal")
        );
    }

    #[test]
    fn clips_of_mixed_colors_show_none_in_the_menu() {
        assert_eq!(same_color([Some("teal"), None].into_iter()), None);
    }

    #[test]
    fn capitals_low_in_their_line_move_up() {
        // A line 2500 tall with its baseline 2000 down: capitals 1400 tall
        // have their middle at 1300, 50 below the line's.
        assert_eq!(capitals_down(2000.0, -500.0, 0.0, 1400.0, 1000.0), -0.05);
    }

    #[test]
    fn a_cut_corner_takes_two_points() {
        let r = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 50.0));
        assert_eq!(
            chamfer(r, 10.0, [true, false, false, false]),
            vec![
                pos2(0.0, 10.0),
                pos2(10.0, 0.0),
                pos2(100.0, 0.0),
                pos2(100.0, 50.0),
                pos2(0.0, 50.0),
            ]
        );
    }
}
