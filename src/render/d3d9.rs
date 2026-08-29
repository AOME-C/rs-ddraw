//! Direct3D 9 presentation backend.
//!
//! Uploads the game's primary surface into an `A8R8G8B8` texture each frame and
//! draws a textured quad to the swap chain, honouring the same viewport /
//! aspect-ratio / letterbox rules the GDI and OpenGL backends use. D3D9 is the
//! most robust backend on modern Windows (it does not depend on a legacy GL
//! context like the OpenGL path does).

use std::sync::atomic::Ordering;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D9::*;
use windows::Win32::Graphics::Direct3D9::IDirect3DBaseTexture9;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

use crate::state::{state, SurfaceBuffers};

pub(crate) struct D3D9State {
    d3d: IDirect3D9,
    device: IDirect3DDevice9,
    hwnd: HWND,
    tex: Option<IDirect3DTexture9>,
    tex_w: i32,
    tex_h: i32,
    surf_bpp: i32,
    client_w: i32,
    client_h: i32,
    vsync: bool,
}

impl D3D9State {
    pub(crate) fn new(hwnd: HWND, _width: i32, _height: i32) -> Option<D3D9State> {
        unsafe {
            let d3d = match Direct3DCreate9(D3D_SDK_VERSION) {
                Some(d) => d,
                None => {
                    crate::dd_log!("d3d9: Direct3DCreate9 returned null");
                    return None;
                }
            };

            let mut rc = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            if !hwnd.is_invalid() {
                GetClientRect(hwnd, &mut rc);
            }
            let cw = (rc.right - rc.left).max(1);
            let ch = (rc.bottom - rc.top).max(1);

            let vsync = { state().lock().unwrap().swap_interval != 0 };

            let mut pp = D3DPRESENT_PARAMETERS {
                BackBufferWidth: cw as u32,
                BackBufferHeight: ch as u32,
                BackBufferFormat: D3DFMT_UNKNOWN,
                BackBufferCount: 1,
                MultiSampleType: D3DMULTISAMPLE_NONE,
                MultiSampleQuality: 0,
                SwapEffect: D3DSWAPEFFECT_DISCARD,
                hDeviceWindow: hwnd,
                Windowed: TRUE,
                EnableAutoDepthStencil: FALSE,
                AutoDepthStencilFormat: D3DFMT_UNKNOWN,
                Flags: 0,
                FullScreen_RefreshRateInHz: 0,
                PresentationInterval: if vsync {
                    D3DPRESENT_INTERVAL_ONE as u32
                } else {
                    D3DPRESENT_INTERVAL_IMMEDIATE as u32
                },
            };

            let mut device: Option<IDirect3DDevice9> = None;
            let hr = d3d.CreateDevice(
                D3DADAPTER_DEFAULT,
                D3DDEVTYPE_HAL,
                hwnd,
                D3DCREATE_SOFTWARE_VERTEXPROCESSING as u32,
                &mut pp,
                &mut device,
            );
            let device = match device {
                Some(d) => d,
                None => {
                    crate::dd_log!("d3d9: CreateDevice failed: {:?}", hr);
                    return None;
                }
            };

            let st = D3D9State {
                d3d,
                device,
                hwnd,
                tex: None,
                tex_w: 0,
                tex_h: 0,
                surf_bpp: 0,
                client_w: cw,
                client_h: ch,
                vsync,
            };
            st.apply_states();
            Some(st)
        }
    }

    /// (Re)apply the fixed-function pipeline state needed for a textured quad.
    fn apply_states(&self) {
        unsafe {
            let _ = self.device.SetRenderState(D3DRS_ZENABLE, 0);
            let _ = self.device.SetRenderState(D3DRS_LIGHTING, 0);
            let _ = self
                .device
                .SetTextureStageState(0, D3DTSS_COLOROP, D3DTOP_SELECTARG1.0 as u32);
            let _ = self
                .device
                .SetTextureStageState(0, D3DTSS_COLORARG1, D3DTA_TEXTURE);
            let _ = self
                .device
                .SetSamplerState(0, D3DSAMP_MINFILTER, D3DTEXF_POINT.0 as u32);
            let _ = self
                .device
                .SetSamplerState(0, D3DSAMP_MAGFILTER, D3DTEXF_POINT.0 as u32);
            let _ = self
                .device
                .SetSamplerState(0, D3DSAMP_ADDRESSU, D3DTADDRESS_CLAMP.0 as u32);
            let _ = self
                .device
                .SetSamplerState(0, D3DSAMP_ADDRESSV, D3DTADDRESS_CLAMP.0 as u32);
        }
    }

    fn present_params(&self, w: i32, h: i32) -> D3DPRESENT_PARAMETERS {
        D3DPRESENT_PARAMETERS {
            BackBufferWidth: w.max(1) as u32,
            BackBufferHeight: h.max(1) as u32,
            BackBufferFormat: D3DFMT_UNKNOWN,
            BackBufferCount: 1,
            MultiSampleType: D3DMULTISAMPLE_NONE,
            MultiSampleQuality: 0,
            SwapEffect: D3DSWAPEFFECT_DISCARD,
            hDeviceWindow: self.hwnd,
            Windowed: TRUE,
            EnableAutoDepthStencil: FALSE,
            AutoDepthStencilFormat: D3DFMT_UNKNOWN,
            Flags: 0,
            FullScreen_RefreshRateInHz: 0,
            PresentationInterval: if self.vsync {
                D3DPRESENT_INTERVAL_ONE as u32
            } else {
                D3DPRESENT_INTERVAL_IMMEDIATE as u32
            },
        }
    }

    /// Recreate the swap chain if the window client area changed size.
    fn ensure_size(&mut self) {
        let mut rc = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if !self.hwnd.is_invalid() {
            unsafe {
                GetClientRect(self.hwnd, &mut rc);
            }
        }
        let cw = (rc.right - rc.left).max(1);
        let ch = (rc.bottom - rc.top).max(1);
        if cw == self.client_w && ch == self.client_h {
            return;
        }
        self.client_w = cw;
        self.client_h = ch;
        // Back buffer size change requires a Reset; drop the texture (recreated
        // lazily on the next upload) so it isn't tied to the old size.
        self.tex = None;
        let mut pp = self.present_params(cw, ch);
        unsafe {
            if self.device.Reset(&mut pp).is_ok() {
                self.apply_states();
            }
        }
    }

    fn ensure_texture(&mut self, w: i32, h: i32, bpp: i32) {
        if self.tex.is_some() && w == self.tex_w && h == self.tex_h && bpp == self.surf_bpp {
            return;
        }
        unsafe {
            self.tex = None;
            let mut tex: Option<IDirect3DTexture9> = None;
            let hr = self.device.CreateTexture(
                w as u32,
                h as u32,
                1,
                0,
                D3DFMT_A8R8G8B8,
                D3DPOOL_MANAGED,
                &mut tex,
                std::ptr::null_mut(),
            );
            match tex {
                Some(t) => {
                    self.tex = Some(t);
                    self.tex_w = w;
                    self.tex_h = h;
                    self.surf_bpp = bpp;
                }
                None => {
                    crate::dd_log!("d3d9: CreateTexture failed: {:?}", hr);
                }
            }
        }
    }

    /// Copy the game surface into the D3D texture, converting to A8R8G8B8.
    fn upload(&self, buffers: &SurfaceBuffers) {
        let tex = match self.tex.as_ref() {
            Some(t) => t,
            None => return,
        };
        unsafe {
            let mut lr = D3DLOCKED_RECT {
                Pitch: 0,
                pBits: std::ptr::null_mut(),
            };
            if tex.LockRect(0, &mut lr, std::ptr::null(), 0).is_err() {
                return;
            }
            let rgb555 = crate::state::RGB555.load(Ordering::Relaxed);
            let bpp = buffers.bpp;
            let src = buffers.surface;
            let src_pitch = buffers.pitch as usize;
            let dst = lr.pBits as *mut u32;
            let dst_pitch = (lr.Pitch as usize) / 4;
            let width = buffers.width as usize;
            let height = buffers.height as usize;

            for y in 0..height {
                let srow = src.add(y * src_pitch);
                let drow = dst.add(y * dst_pitch);
                for x in 0..width {
                    let v = match bpp {
                        32 => {
                            let p = srow.add(x * 4);
                            let b = *p;
                            let g = *p.add(1);
                            let r = *p.add(2);
                            (b as u32)
                                | ((g as u32) << 8)
                                | ((r as u32) << 16)
                                | 0xFF00_0000
                        }
                        16 => {
                            let p = srow.add(x * 2) as *const u16;
                            let v = *p;
                            let (r, g, b) = if rgb555 {
                                (
                                    ((v >> 10) & 0x1F) as u32,
                                    ((v >> 5) & 0x1F) as u32,
                                    (v & 0x1F) as u32,
                                )
                            } else {
                                (
                                    ((v >> 11) & 0x1F) as u32,
                                    ((v >> 5) & 0x3F) as u32,
                                    (v & 0x1F) as u32,
                                )
                            };
                            let r8 = (r * 255 / 31) as u32;
                            let g8 = if rgb555 {
                                (g * 255 / 31) as u32
                            } else {
                                (g * 255 / 63) as u32
                            };
                            let b8 = (b * 255 / 31) as u32;
                            b8 | (g8 << 8) | (r8 << 16) | 0xFF00_0000
                        }
                        _ => 0xFF00_0000,
                    };
                    *drow.add(x) = v;
                }
            }
            let _ = tex.UnlockRect(0);
        }
    }

    pub(crate) fn present(&mut self, buffers: &SurfaceBuffers, upload: bool) {
        unsafe {
            // Recover from a lost device (e.g. Alt+Tab) before drawing.
            if self.device.TestCooperativeLevel().is_err() {
                self.tex = None;
                let mut pp = self.present_params(self.client_w, self.client_h);
                if self.device.Reset(&mut pp).is_err() {
                    return;
                }
                self.apply_states();
            }

            self.ensure_size();

            if upload {
                self.ensure_texture(buffers.width, buffers.height, buffers.bpp);
                self.upload(buffers);
            }

            let _ = self.device.BeginScene();
            let _ = self
                .device
                .Clear(0, std::ptr::null(), D3DCLEAR_TARGET as u32, 0x0000_0000, 0.0, 0);

            // D3D viewport origin is top-left (Windows coordinates), matching
            // `st.render.viewport`, so no Y flip is needed (unlike OpenGL).
            let (vx, vy, vw, vh) = {
                let st = state().lock().unwrap();
                let vp = st.render.viewport;
                if vp.right > vp.left && vp.bottom > vp.top {
                    (
                        vp.left as u32,
                        vp.top as u32,
                        (vp.right - vp.left) as u32,
                        (vp.bottom - vp.top) as u32,
                    )
                } else {
                    (0, 0, self.client_w as u32, self.client_h as u32)
                }
            };
            let vp = D3DVIEWPORT9 {
                X: vx,
                Y: vy,
                Width: vw,
                Height: vh,
                MinZ: 0.0,
                MaxZ: 1.0,
            };
            let _ = self.device.SetViewport(&vp);

            if let Some(tex) = self.tex.as_ref() {
                let base: &IDirect3DBaseTexture9 = tex;
                let _ = self.device.SetTexture(0, Some(base));
                // Triangle strip quad in NDC, uv 0..1 (texture is exact size).
                #[rustfmt::skip]
                let verts: [f32; 20] = [
                    -1.0,  1.0, 0.0,  0.0, 0.0,
                     1.0,  1.0, 0.0,  1.0, 0.0,
                    -1.0, -1.0, 0.0,  0.0, 1.0,
                     1.0, -1.0, 0.0,  1.0, 1.0,
                ];
                let _ = self.device.SetFVF(D3DFVF_XYZ | D3DFVF_TEX1);
                let _ = self.device.DrawPrimitiveUP(
                    D3DPT_TRIANGLESTRIP,
                    2,
                    verts.as_ptr() as *const core::ffi::c_void,
                    20,
                );
                let _ = self.device.SetTexture(0, None);
            }

            let _ = self.device.EndScene();
            if self
                .device
                .Present(
                    std::ptr::null(),
                    std::ptr::null(),
                    HWND::default(),
                    std::ptr::null(),
                )
                .is_err()
            {
                return;
            }

            // GDI overlays (FPS text + child-window compositing) drawn after
            // present; best effort on top of the D3D content.
            let (hwnd, draw_fps, fps, hdc) = {
                let st = state().lock().unwrap();
                (st.hwnd, st.draw_fps, st.fps, st.hdc)
            };
            if draw_fps {
                crate::render::draw_fps(hdc, fps);
            }
            crate::render::composite_child_windows(hwnd, buffers.hdc);
        }
    }

    pub(crate) fn release(self) {
        // COM pointers are released by their Drop impls when `self` is dropped.
    }
}
