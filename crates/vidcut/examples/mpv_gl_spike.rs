//! M0b spike: libmpv's OpenGL renderer inside eframe/egui.
//!
//! Renders each player into its own FBO-backed texture (registered once with
//! egui and resized in place), shows the active one, and prints playback
//! statistics. With barcode fixtures it also verifies the picture orientation by
//! reading the FBO back and decoding the frame index.
//!
//! ```text
//! cargo run -p vidcut --example mpv_gl_spike -- FILE_A [FILE_B] [--auto-exit SECS] [--hwdec MODE]
//! ```
//! Keys: Space play/pause, Q switch player, ←/→ frame step, Home restart.
//!
//! The UI thread is mpv's render thread, so it only uses the client functions
//! libmpv declares render-thread safe: async commands, property observation and
//! `wait_event(0)`. Synchronous calls here deadlock against the video output.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use eframe::glow::{self, HasContext};
use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
use std::collections::HashMap;

use vidcut_mpv::{Display, Event, Format, GlRenderer, Mpv, Value, ffi::MPV_RENDER_UPDATE_FRAME};

const OBSERVED: &[(&str, Format)] = &[
    ("time-pos", Format::Double),
    ("pause", Format::Flag),
    ("hwdec-current", Format::String),
    ("estimated-vf-fps", Format::Double),
    ("frame-drop-count", Format::Int),
    ("decoder-frame-drop-count", Format::Int),
    ("vo-delayed-frame-count", Format::Int),
    ("video-params/dw", Format::Int),
    ("video-params/dh", Format::Int),
    ("video-params/pixelformat", Format::String),
    ("container-fps", Format::Double),
];

struct Target {
    texture: glow::Texture,
    fbo: glow::Framebuffer,
    size: [i32; 2],
}

struct Player {
    name: String,
    mpv: Mpv,
    renderer: Option<GlRenderer>,
    target: Option<Target>,
    texture_id: Option<egui::TextureId>,
    frames_rendered: u64,
    /// Orientation check result once the first barcode frame has been read back.
    orientation: Option<String>,
    checking_orientation: bool,
    /// Seek for the orientation check completed; read back after the next render.
    await_render: bool,
    /// Latest values of the observed properties.
    props: HashMap<String, Value>,
}

impl Player {
    fn num(&self, name: &str) -> Option<f64> {
        match self.props.get(name)? {
            Value::Double(v) => Some(*v),
            Value::Int(v) => Some(*v as f64),
            _ => None,
        }
    }

    fn text(&self, name: &str) -> String {
        match self.props.get(name) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Double(v)) => format!("{v:.3}"),
            Some(Value::Int(v)) => v.to_string(),
            Some(Value::Flag(v)) => v.to_string(),
            _ => "-".into(),
        }
    }

    fn command(&self, args: &[&str]) {
        let _ = self.mpv.command_async(0, args);
    }
}

struct Spike {
    gl: Arc<glow::Context>,
    players: Vec<Player>,
    active: usize,
    flip_y: bool,
    auto_exit: Option<Duration>,
    started: Instant,
    report_done: bool,
    last_progress: Instant,
}

fn options(hwdec: &str) -> Vec<(&'static str, String)> {
    [
        ("vo", "libmpv"),
        ("config", "no"),
        ("load-scripts", "no"),
        ("ytdl", "no"),
        ("terminal", "no"),
        ("input-default-bindings", "no"),
        ("osc", "no"),
        ("hr-seek", "yes"),
        ("hr-seek-framedrop", "no"),
        ("keep-open", "always"),
        ("idle", "yes"),
        ("pause", "yes"),
        ("video-timing-offset", "0"),
    ]
    .into_iter()
    .map(|(k, v)| (k, v.to_string()))
    .chain([("hwdec", hwdec.to_string())])
    .collect()
}

impl Spike {
    fn new(
        cc: &eframe::CreationContext<'_>,
        files: Vec<String>,
        auto_exit: Option<Duration>,
        hwdec: String,
    ) -> Self {
        let gl = cc.gl.clone().expect("glow backend");
        let proc_address = cc.get_proc_address.clone().expect("get_proc_address");
        let display = match cc.display_handle().map(|h| h.as_raw()) {
            Ok(RawDisplayHandle::Wayland(h)) => Display::Wayland(h.display.as_ptr()),
            Ok(RawDisplayHandle::Xlib(h)) => {
                Display::X11(h.display.map_or(std::ptr::null_mut(), |d| d.as_ptr()))
            }
            _ => Display::None,
        };
        eprintln!("display: {display:?}");
        let opts = options(&hwdec);
        let opts: Vec<(&str, &str)> = opts.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let players = files
            .iter()
            .map(|file| {
                let mpv = Mpv::new(&opts).expect("mpv");
                mpv.request_log_messages("warn").unwrap();
                for (id, (name, format)) in OBSERVED.iter().enumerate() {
                    mpv.observe_property(id as u64, name, *format).unwrap();
                }
                let lookup = proc_address.clone();
                let renderer = GlRenderer::new(&mpv, Box::new(move |name| lookup(name)), display)
                    .expect("GL renderer");
                let ctx = cc.egui_ctx.clone();
                renderer.set_update_callback(move || ctx.request_repaint());
                let ctx = cc.egui_ctx.clone();
                mpv.set_wakeup_callback(move || ctx.request_repaint());
                mpv.command_async(0, &["loadfile", file, "replace"])
                    .expect("loadfile");
                Player {
                    name: file.rsplit('/').next().unwrap_or(file).to_string(),
                    mpv,
                    renderer: Some(renderer),
                    target: None,
                    texture_id: None,
                    frames_rendered: 0,
                    orientation: None,
                    checking_orientation: false,
                    await_render: false,
                    props: HashMap::new(),
                }
            })
            .collect();
        Self {
            gl,
            players,
            active: 0,
            flip_y: false,
            auto_exit,
            started: Instant::now(),
            report_done: false,
            last_progress: Instant::now(),
        }
    }

    /// (Re)creates `player`'s render target at `size`, keeping its texture name
    /// (egui has no way to unregister a native texture).
    fn ensure_target(
        gl: &glow::Context,
        frame: &mut eframe::Frame,
        player: &mut Player,
        size: [i32; 2],
    ) {
        if player.target.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        unsafe {
            let texture = match &player.target {
                Some(t) => t.texture,
                None => gl.create_texture().expect("texture"),
            };
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                size[0],
                size[1],
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_S,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_T,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.bind_texture(glow::TEXTURE_2D, None);
            let fbo = match &player.target {
                Some(t) => t.fbo,
                None => gl.create_framebuffer().expect("fbo"),
            };
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            assert_eq!(status, glow::FRAMEBUFFER_COMPLETE, "FBO incomplete");
            if player.texture_id.is_none() {
                player.texture_id = Some(frame.register_native_glow_texture(texture));
            }
            player.target = Some(Target { texture, fbo, size });
        }
    }

    /// Renders a new frame into the player's FBO if mpv has one.
    fn render(gl: &glow::Context, player: &mut Player, flip_y: bool) {
        let (Some(renderer), Some(target)) = (player.renderer.as_ref(), player.target.as_ref())
        else {
            return;
        };
        if renderer.update() & MPV_RENDER_UPDATE_FRAME == 0 {
            return;
        }
        renderer
            .render(target.fbo.0.get(), target.size[0], target.size[1], flip_y)
            .expect("render");
        player.frames_rendered += 1;
        unsafe {
            // mpv leaves GL state behind; egui's painter resets most of it but
            // never rebinds the default framebuffer.
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, None);
            gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, 0);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.bind_vertex_array(None);
            gl.use_program(None);
        }
    }

    /// Reads the FBO back and decodes the barcode as egui will display it
    /// (texture row 0 at the top).
    fn read_back_barcode(gl: &glow::Context, target: &Target) -> Option<u32> {
        let [w, h] = target.size;
        let mut buf = vec![0u8; (w * h * 4) as usize];
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(target.fbo));
            gl.read_pixels(
                0,
                0,
                w,
                h,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut buf)),
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        vidcut_av::barcode::read_packed(&buf, w as usize, h as usize, (w * 4) as usize, 4, 1)
    }

    fn report(&self) {
        for p in &self.players {
            let fields: Vec<String> = OBSERVED
                .iter()
                .map(|(n, _)| format!("{n}={}", p.text(n)))
                .collect();
            println!(
                "[{}] {} rendered={} orientation={}",
                p.name,
                fields.join(" "),
                p.frames_rendered,
                p.orientation.as_deref().unwrap_or("unchecked"),
            );
        }
    }
}

impl eframe::App for Spike {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Drain mpv events; start playback once the file is ready.
        for (i, p) in self.players.iter_mut().enumerate() {
            while let Some(ev) = p.mpv.wait_event(0.0) {
                match ev {
                    Event::FileLoaded => {
                        // Seek to 1.5 s for the orientation check, then play.
                        p.checking_orientation = true;
                        p.command(&["seek", "1.49", "absolute+exact"]);
                    }
                    Event::PlaybackRestart if p.checking_orientation => p.await_render = true,
                    Event::PropertyChange { name, value, .. } => {
                        p.props.insert(name, value);
                    }
                    Event::LogMessage {
                        prefix,
                        level,
                        text,
                    } => eprint!("[{i}:{prefix}:{level}] {text}"),
                    Event::EndFile { reason: 4, error } => {
                        eprintln!("[{i}] playback error {error}")
                    }
                    _ => {}
                }
            }
        }

        let input = ctx.input(|i| i.clone());
        if input.key_pressed(egui::Key::Q) && self.players.len() > 1 {
            self.players[self.active].command(&["set", "pause", "yes"]);
            self.active = (self.active + 1) % self.players.len();
        }
        let active = &self.players[self.active];
        if input.key_pressed(egui::Key::Space) {
            active.command(&["cycle", "pause"]);
        }
        if input.key_pressed(egui::Key::ArrowRight) {
            active.command(&["frame-step"]);
        }
        if input.key_pressed(egui::Key::ArrowLeft) {
            active.command(&["frame-back-step"]);
        }
        if input.key_pressed(egui::Key::Home) {
            active.command(&["seek", "0", "absolute+exact"]);
        }

        let ppp = ctx.pixels_per_point();
        let avail = ui.available_rect_before_wrap();
        let gl = self.gl.clone();
        for (i, p) in self.players.iter_mut().enumerate() {
            // Fit the video into the panel at physical-pixel resolution.
            let w = p.num("video-params/dw").unwrap_or(16.0);
            let h = p.num("video-params/dh").unwrap_or(9.0);
            let scale = (f64::from(avail.width()) / w)
                .min(f64::from(avail.height() - 60.0) / h)
                .max(0.01);
            let size_pts = egui::vec2((w * scale) as f32, (h * scale) as f32);
            let size_px = [
                ((size_pts.x * ppp).round() as i32).max(16),
                ((size_pts.y * ppp).round() as i32).max(16),
            ];
            Self::ensure_target(&gl, frame, p, size_px);
            let before = p.frames_rendered;
            Self::render(&gl, p, self.flip_y);
            let rendered = p.frames_rendered > before;

            // Orientation check on the frame rendered after the check's seek.
            if p.await_render && rendered {
                p.await_render = false;
                let target = p.target.as_ref().unwrap();
                let code = Self::read_back_barcode(&gl, target);
                // MKV has no container-fps; the decoder's estimate works everywhere.
                let fps = p
                    .num("container-fps")
                    .or_else(|| p.num("estimated-vf-fps"))
                    .unwrap_or(30.0);
                let want = (p.num("time-pos").unwrap_or(0.0) * fps).round() as u32;
                let verdict = match code {
                    Some(c) if c == want => "upright".to_string(),
                    Some(c) if c == vidcut_av::barcode::MAX_INDEX - want => "flipped".to_string(),
                    other => format!("unknown ({other:?}, want {want})"),
                };
                println!(
                    "[{}] orientation with flip_y={}: {verdict}",
                    p.name, self.flip_y
                );
                if verdict == "flipped" && !self.flip_y {
                    self.flip_y = true; // re-check with the other setting
                    p.command(&["seek", "1.49", "absolute+exact"]);
                } else {
                    p.orientation = Some(format!("{verdict} (flip_y={})", self.flip_y));
                    p.checking_orientation = false;
                    if i == self.active {
                        p.command(&["set", "pause", "no"]);
                    }
                }
            }
        }

        egui::CentralPanel::default().show(ui, |ui| {
            let p = &self.players[self.active];
            ui.label(format!(
                "{} | hwdec {} | {} fps est | drops {} | t {} | Space Q ← → Home",
                p.name,
                p.text("hwdec-current"),
                p.text("estimated-vf-fps"),
                p.text("frame-drop-count"),
                p.text("time-pos"),
            ));
            if let (Some(id), Some(t)) = (p.texture_id, p.target.as_ref()) {
                let size = egui::vec2(t.size[0] as f32 / ppp, t.size[1] as f32 / ppp);
                ui.add(egui::Image::new((id, size)));
            }
        });

        if self.last_progress.elapsed() >= Duration::from_secs(1) {
            self.last_progress = Instant::now();
            let p = &self.players[self.active];
            eprintln!(
                "progress t={} drops={} rendered={}",
                p.text("time-pos"),
                p.text("frame-drop-count"),
                p.frames_rendered
            );
        }
        if let Some(limit) = self.auto_exit
            && self.started.elapsed() >= limit
            && !self.report_done
        {
            self.report();
            self.report_done = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if self.auto_exit.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn on_exit(&mut self, gl: Option<&glow::Context>) {
        // Free render contexts while the GL context is still current, then the
        // GL objects; mpv handles go last (when `players` drops).
        for p in &mut self.players {
            p.renderer = None;
            if let (Some(gl), Some(t)) = (gl, p.target.take()) {
                unsafe { gl.delete_framebuffer(t.fbo) };
                // The texture belongs to egui after registration.
            }
        }
        eprintln!("clean shutdown");
    }
}

fn main() -> eframe::Result {
    let mut files = Vec::new();
    let mut auto_exit = None;
    let mut hwdec = "auto-safe".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--auto-exit" => {
                auto_exit = args
                    .next()
                    .and_then(|s| s.parse().ok())
                    .map(Duration::from_secs_f64)
            }
            "--hwdec" => hwdec = args.next().expect("--hwdec MODE"),
            _ => files.push(a),
        }
    }
    assert!(
        !files.is_empty(),
        "usage: mpv_gl_spike FILE_A [FILE_B] [--auto-exit SECS] [--hwdec MODE]"
    );
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        // With vsync, NVIDIA's EGL on Wayland blocks eglSwapBuffers until the
        // compositor sends a frame callback, which GNOME withholds while the
        // window is hidden or occluded: the UI (= mpv render) thread then hangs.
        // Repaints are driven by mpv's frame callbacks anyway.
        glow_options: eframe::egui_glow::GlowConfiguration {
            vsync: false,
            ..Default::default()
        },
        viewport: egui::ViewportBuilder::default()
            .with_title("vidcut mpv GL spike")
            .with_inner_size([1280.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "vidcut-mpv-gl-spike",
        options,
        Box::new(move |cc| Ok(Box::new(Spike::new(cc, files, auto_exit, hwdec)))),
    )
}
