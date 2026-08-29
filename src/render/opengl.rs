//! OpenGL presentation backend (ports the OpenGL branch of `render.c`).
//!
//! Uses the fixed-function GL 1.1 pipeline available through `opengl32.dll`,
//! uploading the 16-bit surface as an RGB565 texture each frame.

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::HDC;
use windows::Win32::Graphics::OpenGL::*;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows::core::PCSTR;

use crate::state::{state, SurfaceBuffers};

const GL_UNSIGNED_SHORT_5_6_5: u32 = 33635;
const GL_TEXTURE_MAX_LEVEL: u32 = 33117;
const GL_BGRA: u32 = 0x80E1;

type SwapIntervalFn = unsafe extern "system" fn(i32) -> i32;

pub(crate) struct OglState {
    hdc: HDC,
    ctx: HGLRC,
    tex: u32,
    /// Allocated texture dimensions (next power-of-two of the surface).
    tex_w: i32,
    tex_h: i32,
    /// Surface dimensions the texture was last configured for.
    surf_w: i32,
    surf_h: i32,
    surf_bpp: i32,
    scale_w: f32,
    scale_h: f32,
    swap_fn: Option<SwapIntervalFn>,
}

fn next_pow2(mut v: i32) -> i32 {
    v -= 1;
    v |= v >> 1;
    v |= v >> 2;
    v |= v >> 4;
    v |= v >> 8;
    v |= v >> 16;
    v + 1
}

impl OglState {
    pub(crate) fn new(hdc: HDC, width: i32, height: i32) -> Option<OglState> {
        unsafe {
            let ctx = wglCreateContext(hdc).ok()?;
            if wglMakeCurrent(hdc, ctx).is_err() {
                let _ = wglDeleteContext(ctx);
                return None;
            }

            let mut tex: u32 = 0;
            glGenTextures(1, &mut tex);
            if tex == 0 {
                let _ = wglMakeCurrent(hdc, HGLRC(std::ptr::null_mut()));
                let _ = wglDeleteContext(ctx);
                return None;
            }

            let tex_w = next_pow2(width);
            let tex_h = next_pow2(height);

            glBindTexture(GL_TEXTURE_2D, tex);
            // NOTE: wglCreateContext yields a legacy GL 1.1 context. Sized
            // internal formats such as GL_RGB565/GL_RGBA8 are invalid there and
            // cause glTexImage2D to allocate no storage, after which the
            // glTexSubImage2D write faults the GPU (TDR / driver reset). Use the
            // legacy-safe internal formats GL_RGB / GL_RGBA.
            glTexImage2D(
                GL_TEXTURE_2D,
                0,
                GL_RGB as i32,
                tex_w,
                tex_h,
                0,
                GL_RGB,
                GL_UNSIGNED_SHORT_5_6_5,
                std::ptr::null(),
            );
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST as i32);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST as i32);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAX_LEVEL, 0);
            glEnable(GL_TEXTURE_2D);

            glMatrixMode(GL_MODELVIEW);
            glLoadIdentity();
            glMatrixMode(GL_PROJECTION);
            glLoadIdentity();

            let _ = wglMakeCurrent(hdc, HGLRC(std::ptr::null_mut()));

            let swap_fn = {
                let name = b"wglSwapIntervalEXT\0";
                let addr: PROC = wglGetProcAddress(PCSTR(name.as_ptr()));
                if addr.is_some() {
                    Some(std::mem::transmute_copy(&addr))
                } else {
                    None
                }
            };

            Some(OglState {
                hdc,
                ctx,
                tex,
                tex_w,
                tex_h,
                surf_w: width,
                surf_h: height,
                surf_bpp: 16,
                scale_w: width as f32 / tex_w as f32,
                scale_h: height as f32 / tex_h as f32,
                swap_fn,
            })
        }
    }

    /// Recreate the GL texture if the surface dimensions / pixel format changed.
    /// The game may recreate the primary at a different size between e.g. the
    /// loading screen and the actual game, and the texture is allocated only
    /// once at construction; without this the upload silently fails and the
    /// screen freezes on the last good frame.
    fn ensure_texture(&mut self, width: i32, height: i32, bpp: i32) {
        if width == self.surf_w && height == self.surf_h && bpp == self.surf_bpp {
            return;
        }
        let tex_w = next_pow2(width.max(1));
        let tex_h = next_pow2(height.max(1));

        unsafe {
            glDeleteTextures(1, &self.tex);
            glGenTextures(1, &mut self.tex);
            glBindTexture(GL_TEXTURE_2D, self.tex);

            let (internal, format, type_) = if bpp == 32 {
                (GL_RGBA as i32, GL_BGRA, GL_UNSIGNED_BYTE)
            } else {
                (GL_RGB as i32, GL_RGB, GL_UNSIGNED_SHORT_5_6_5)
            };
            glTexImage2D(
                GL_TEXTURE_2D,
                0,
                internal,
                tex_w,
                tex_h,
                0,
                format,
                type_,
                std::ptr::null(),
            );
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST as i32);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST as i32);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAX_LEVEL, 0);
        }

        self.tex_w = tex_w;
        self.tex_h = tex_h;
        self.surf_w = width;
        self.surf_h = height;
        self.surf_bpp = bpp;
        self.scale_w = width as f32 / tex_w as f32;
        self.scale_h = height as f32 / tex_h as f32;
        crate::dd_log!(
            "opengl: recreated texture for surface {}x{} bpp={} (tex {}x{})",
            width,
            height,
            bpp,
            tex_w,
            tex_h
        );
    }

    pub(crate) fn present(&mut self, buffers: &SurfaceBuffers, upload: bool) {
        let (hwnd, swap_interval, fps, draw_fps) = {
            let st = state().lock().unwrap();
            (st.hwnd, st.swap_interval, st.fps, st.draw_fps)
        };
        unsafe {
            if wglMakeCurrent(self.hdc, self.ctx).is_err() {
                return;
            }

            if let Some(f) = self.swap_fn {
                f(swap_interval);
            }

            // Recreate the texture if the surface size / format changed (e.g.
            // loading screen -> in-game resolution). Otherwise the fixed-size
            // upload would silently fail and the screen would freeze.
            self.ensure_texture(buffers.width, buffers.height, buffers.bpp);

            let mut rc = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            if !hwnd.is_invalid() {
                GetClientRect(hwnd, &mut rc);
            }
            let cw = rc.right - rc.left;
            let ch = rc.bottom - rc.top;

            let (vl, vt, vr, vb, scale_w, scale_h, stretched) = {
                let st = state().lock().unwrap();
                let vp = st.render.viewport;
                if vp.right > vp.left && vp.bottom > vp.top {
                    (
                        vp.left,
                        vp.top,
                        vp.right,
                        vp.bottom,
                        st.render.scale_w,
                        st.render.scale_h,
                        st.render.stretched,
                    )
                } else if cw > 0 && ch > 0 {
                    (0, 0, cw, ch, 1.0f32, 1.0f32, false)
                } else {
                    (0, 0, 1, 1, 1.0f32, 1.0f32, false)
                }
            };

            if vr > vl && vb > vt {
                glViewport(vl, vt, vr - vl, vb - vt);
            } else if cw > 0 && ch > 0 {
                glViewport(0, 0, cw, ch);
            }

            if stretched {
                glScalef(scale_w, scale_h, 1.0);
            }

            glClearColor(0.0, 0.0, 0.0, 1.0);
            glClear(GL_COLOR_BUFFER_BIT);

            glEnable(GL_TEXTURE_2D);
            glBindTexture(GL_TEXTURE_2D, self.tex);

            glPixelStorei(GL_UNPACK_ROW_LENGTH, buffers.pitch / ((buffers.bpp / 8).max(1)));
            glPixelStorei(GL_UNPACK_ALIGNMENT, 1);

            let (tex_format, tex_type) = if buffers.bpp == 32 {
                (GL_BGRA, GL_UNSIGNED_BYTE)
            } else {
                (GL_RGB, GL_UNSIGNED_SHORT_5_6_5)
            };
            // Upload the surface under its lock so we don't read mid-Blt.
            // Release before drawing/swapping so we never block the game's
            // Blts for a whole vsync interval. Only upload when a new frame
            // was signalled (`upload`); otherwise redraw the last texture so
            // the previous (complete) frame stays on screen.
            if upload {
                let owner_was = buffers.lock.owner();
                let p = buffers.surface as *const u8;
                let mut sum = 0u32;
                let n = (buffers.width as usize * (buffers.bpp as usize / 8)).min(4000);
                for i in 0..n {
                    sum += *p.add(i) as u32;
                }
                let t0 = std::time::Instant::now();
                let _guard = buffers.lock.lock();
                let waited = t0.elapsed();
                static PC: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                let f = PC.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if f < 40 {
                    crate::dd_log!(
                        "DIAG present#{} owner_was={} waited_ms={:?} row0sum={} w={} h={} bpp={}",
                        f,
                        owner_was,
                        waited,
                        sum,
                        buffers.width,
                        buffers.height,
                        buffers.bpp
                    );
                }
                glTexSubImage2D(
                    GL_TEXTURE_2D,
                    0,
                    0,
                    0,
                    buffers.width,
                    buffers.height,
                    tex_format,
                    tex_type,
                    buffers.surface as *const core::ffi::c_void,
                );
            }

            glBegin(GL_TRIANGLE_STRIP);
            glTexCoord2f(0.0, 0.0);
            glVertex2f(-1.0, 1.0);
            glTexCoord2f(self.scale_w, 0.0);
            glVertex2f(1.0, 1.0);
            glTexCoord2f(0.0, self.scale_h);
            glVertex2f(-1.0, -1.0);
            glTexCoord2f(self.scale_w, self.scale_h);
            glVertex2f(1.0, -1.0);
            glEnd();

            let _ = SwapBuffers(self.hdc);

            if draw_fps {
                crate::render::draw_fps(self.hdc, fps);
            }

            crate::render::composite_child_windows(hwnd, buffers.hdc);

            let _ = wglMakeCurrent(self.hdc, HGLRC(std::ptr::null_mut()));
        }
    }

    pub(crate) fn release(self) {
        unsafe {
            let _ = wglMakeCurrent(self.hdc, HGLRC(std::ptr::null_mut()));
            glDeleteTextures(1, &self.tex);
            let _ = wglDeleteContext(self.ctx);
        }
    }
}
