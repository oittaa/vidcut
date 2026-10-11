//! libmpv's OpenGL renderer drawing into textures egui shows
//! (docs/decisions/preview.md §6).
//!
//! Each player renders into its own framebuffer object. Its texture is
//! registered with egui once (egui can't unregister native textures) and
//! re-specified when the viewer size changes. Rendering happens in
//! `App::logic`, which eframe calls even while the window is hidden, so
//! players keep running and the viewer can be captured at any time.

use eframe::egui;
use eframe::glow::{self, HasContext};
use vidcut_mpv::GlRenderer;
use vidcut_mpv::ffi::MPV_RENDER_UPDATE_FRAME;

use crate::live::Image;

struct Target {
    texture: glow::Texture,
    fbo: glow::Framebuffer,
    size: [i32; 2],
}

/// One player's render context and target.
pub struct Video {
    renderer: Option<GlRenderer>,
    target: Option<Target>,
    pub texture_id: Option<egui::TextureId>,
    /// Size the viewer last asked for, in physical pixels.
    pub want: [i32; 2],
    /// The target holds a rendered frame.
    pub has_frame: bool,
}

impl Video {
    pub fn new(renderer: Option<GlRenderer>) -> Self {
        Self {
            renderer,
            target: None,
            texture_id: None,
            want: [640, 360],
            has_frame: false,
        }
    }

    /// Renders a new frame if mpv has one (or the target was resized).
    /// Returns `Some(new)` if it drew, where `new` says whether that was a new
    /// picture rather than a redraw.
    pub fn render(&mut self, gl: &glow::Context, frame: &mut eframe::Frame) -> Option<bool> {
        self.renderer.as_ref()?;
        let resized = self.ensure_target(gl, frame);
        let renderer = self.renderer.as_ref()?;
        // `update` must run whenever mpv asked for it, even if we redraw anyway.
        let update = renderer.update() & MPV_RENDER_UPDATE_FRAME != 0;
        if !(update || resized && self.has_frame) {
            return None;
        }
        let new = update && renderer.next_frame_is_new();
        if crate::live::debug() {
            eprintln!("[gl] render update={update} resized={resized} new={new}");
        }
        let target = self.target.as_ref()?;
        if let Err(e) = renderer.render(target.fbo.0.get(), target.size[0], target.size[1], false) {
            eprintln!("vidcut: mpv render failed: {e}");
            return None;
        }
        self.has_frame = true;
        // SAFETY: plain GL state calls on the thread that owns the context.
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
        Some(new)
    }

    /// (Re)creates the target at `want`; returns whether it changed. The
    /// texture name stays the same, so egui's registration stays valid.
    fn ensure_target(&mut self, gl: &glow::Context, frame: &mut eframe::Frame) -> bool {
        let size = [self.want[0].max(16), self.want[1].max(16)];
        if self.target.as_ref().is_some_and(|t| t.size == size) {
            return false;
        }
        // SAFETY: GL calls on the thread that owns the context; the objects
        // created here are owned by this target (the texture by egui once
        // registered).
        unsafe {
            let texture = match &self.target {
                Some(t) => t.texture,
                None => match gl.create_texture() {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!("vidcut: creating a GL texture: {e}");
                        return false;
                    }
                },
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
            for (param, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, param, value as i32);
            }
            gl.bind_texture(glow::TEXTURE_2D, None);
            let fbo = match &self.target {
                Some(t) => t.fbo,
                None => match gl.create_framebuffer() {
                    Ok(f) => f,
                    Err(e) => {
                        eprintln!("vidcut: creating a GL framebuffer: {e}");
                        return false;
                    }
                },
            };
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            // Black until mpv draws.
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            if self.texture_id.is_none() {
                self.texture_id = Some(frame.register_native_glow_texture(texture));
            }
            self.target = Some(Target { texture, fbo, size });
        }
        true
    }

    /// Reads the picture back as displayed (rows top to bottom).
    pub fn read_back(&self, gl: &glow::Context) -> Option<Image> {
        let t = self.target.as_ref()?;
        let [w, h] = t.size;
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        // SAFETY: reads `w`×`h` RGBA pixels into a buffer of exactly that size.
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(t.fbo));
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            gl.read_pixels(
                0,
                0,
                w,
                h,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut rgba)),
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        // mpv's alpha is undefined outside the picture; the viewer is opaque.
        for px in rgba.chunks_exact_mut(4) {
            px[3] = 255;
        }
        Some(Image {
            width: w as u32,
            height: h as u32,
            rgba,
        })
    }

    /// Frees the render context (GL context current) and the framebuffer.
    pub fn destroy(&mut self, gl: Option<&glow::Context>) {
        self.renderer = None;
        if let (Some(gl), Some(t)) = (gl, self.target.take()) {
            // SAFETY: the framebuffer belongs to us; the texture to egui.
            unsafe { gl.delete_framebuffer(t.fbo) };
        }
    }
}
