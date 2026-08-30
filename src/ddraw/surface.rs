//! DirectDraw surface implementation (ports `IDirectDrawSurface.c`).
//!
//! Each surface owns a GDI DIB section so the game can write pixels directly
//! via `Lock`/`GetDC`, and the renderer thread can read the same memory to
//! present it to the screen.
//!
//! The surface is exposed through every DirectDraw version (1..7) because
//! games `QueryInterface` for the version they were built against (RA2 uses
//! the v3/v4 interfaces) and `CreateSurface` itself `cast`s the v1 object up
//! to v4/v7.

use std::sync::Arc;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::DirectDraw::*;
use windows::Win32::Graphics::Gdi::{
    BI_BITFIELDS, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, GetDC, HDC,
    HGDIOBJ, ReleaseDC, SelectObject,
};
use windows::core::*;

use crate::state::SurfaceBuffers;

#[implement(IDirectDrawSurface7, IDirectDrawSurface4, IDirectDrawSurface3, IDirectDrawSurface2, IDirectDrawSurface)]
pub struct SurfaceImpl {
    pub width: i32,
    pub height: i32,
    pub bpp: i32,
    pub pitch: i32,
    pub caps: u32,
    pub is_primary: bool,
    pub buffers: Arc<SurfaceBuffers>,
    pub backbuffer: Option<Arc<SurfaceBuffers>>,
}

unsafe impl Send for SurfaceImpl {}
unsafe impl Sync for SurfaceImpl {}

fn pixel_type(bpp: i32) -> u32 {
    match bpp {
        8 => DDPF_PALETTEINDEXED8 as u32 | DDPF_RGB as u32,
        _ => DDPF_RGB as u32,
    }
}

fn fill_pixel_format(pf: &mut DDPIXELFORMAT, bpp: i32) {
    pf.dwSize = std::mem::size_of::<DDPIXELFORMAT>() as u32;
    pf.dwFlags = pixel_type(bpp);
    pf.Anonymous1.dwRGBBitCount = bpp as u32;
    if bpp == 16 {
        if crate::state::RGB555.load(std::sync::atomic::Ordering::Relaxed) {
            pf.Anonymous2.dwRBitMask = 0x7C00;
            pf.Anonymous3.dwGBitMask = 0x03E0;
            pf.Anonymous4.dwBBitMask = 0x001F;
        } else {
            pf.Anonymous2.dwRBitMask = 0xF800;
            pf.Anonymous3.dwGBitMask = 0x07E0;
            pf.Anonymous4.dwBBitMask = 0x001F;
        }
    }
}

/// Extra DIB scanlines allocated below the surface so buggy games that write
/// past the bottom of the framebuffer (e.g. Tiberian Sun) don't corrupt the
/// heap. Mirrors ts-ddraw's `guardLines = 200`.
const GUARD_LINES: i32 = 200;

/// Build a DIB section backing store for a surface.
unsafe fn make_buffers(hdc: HDC, width: i32, height: i32, bpp: i32) -> SurfaceBuffers {
    let bytes_pp = (bpp / 8).max(1);
    let pitch = width * bytes_pp;
    let header_size = std::mem::size_of::<BITMAPINFOHEADER>();
    let mut buf = vec![0u8; header_size + 12];

    let dev_height = height + GUARD_LINES;
    let bmh = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width,
        biHeight: -dev_height,
        biPlanes: 1,
        biBitCount: bpp as u16,
        biCompression: BI_BITFIELDS.0,
        biSizeImage: 0,
        biXPelsPerMeter: 0,
        biYPelsPerMeter: 0,
        biClrUsed: 0,
        biClrImportant: 0,
    };
    let bmh_ptr = buf.as_mut_ptr() as *mut BITMAPINFOHEADER;
    *bmh_ptr = bmh;

    let masks: [u32; 3] = if bpp == 32 {
        // 32-bit BGRX (matches GL_BGRA upload order).
        [0x00FF0000, 0x0000FF00, 0x000000FF]
    } else if crate::state::RGB555.load(std::sync::atomic::Ordering::Relaxed) {
        [0x7C00, 0x03E0, 0x001F]
    } else {
        [0xF800, 0x07E0, 0x001F]
    };
    let mask_ptr = buf.as_mut_ptr().add(header_size) as *mut u32;
    std::ptr::copy_nonoverlapping(masks.as_ptr(), mask_ptr, 3);

    let base = if hdc.is_invalid() { GetDC(None) } else { hdc };
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let bitmap =
        CreateDIBSection(Some(base), buf.as_ptr() as *const BITMAPINFO, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
    let memdc = CreateCompatibleDC(Some(base));
    let default_bm = SelectObject(memdc, HGDIOBJ(bitmap.0));
    if hdc.is_invalid() {
        let _ = ReleaseDC(None, base);
    }

    SurfaceBuffers {
        hdc: memdc,
        bitmap,
        default_bm,
        surface: bits as *mut u8,
        width,
        height,
        pitch,
        bpp,
        using_pbo: false,
        lock: crate::state::ReentrantLock::new(),
    }
}

impl SurfaceImpl {
    pub fn new(hdc: HDC, width: i32, height: i32, bpp: i32, is_primary: bool) -> Self {
        let buffers = unsafe { Arc::new(make_buffers(hdc, width, height, bpp)) };
        let caps = if is_primary {
            DDSCAPS_PRIMARYSURFACE as u32 | DDSCAPS_VIDEOMEMORY as u32
        } else {
            DDSCAPS_OFFSCREENPLAIN as u32 | DDSCAPS_SYSTEMMEMORY as u32
        };
        Self { width, height, bpp, pitch: buffers.pitch, caps, is_primary, buffers, backbuffer: None }
    }

    /// Attach a back buffer (shares a separate DIB with the primary).
    pub fn with_backbuffer(mut self, hdc: HDC, width: i32, height: i32, bpp: i32) -> Self {
        let bb = unsafe { Arc::new(make_buffers(hdc, width, height, bpp)) };
        self.backbuffer = Some(bb);
        self
    }

    fn from_buffers(buffers: Arc<SurfaceBuffers>, width: i32, height: i32, bpp: i32, is_primary: bool) -> Self {
        let caps = if is_primary {
            DDSCAPS_PRIMARYSURFACE as u32 | DDSCAPS_VIDEOMEMORY as u32
        } else {
            DDSCAPS_OFFSCREENPLAIN as u32 | DDSCAPS_SYSTEMMEMORY as u32
        };
        Self { width, height, bpp, pitch: buffers.pitch, caps, is_primary, buffers, backbuffer: None }
    }

    // ---- shared implementation helpers (use the v7/DDSURFACEDESC2 types) ----

    unsafe fn fill_desc2(&self, desc: &mut DDSURFACEDESC2, pixels: *mut u8) {
        desc.dwSize = std::mem::size_of::<DDSURFACEDESC2>() as u32;
        desc.dwFlags = (DDSD_CAPS | DDSD_WIDTH | DDSD_HEIGHT | DDSD_PITCH | DDSD_PIXELFORMAT | DDSD_LPSURFACE) as u32;
        desc.dwWidth = self.width as u32;
        desc.dwHeight = self.height as u32;
        desc.Anonymous1.lPitch = self.pitch;
        desc.lpSurface = pixels as *mut core::ffi::c_void;
        desc.ddsCaps.dwCaps = self.caps;
        fill_pixel_format(&mut desc.Anonymous5.ddpfPixelFormat, self.bpp);
    }

    fn lock_impl2(&self, rect: *mut RECT, desc: *mut DDSURFACEDESC2, _flags: u32, _event: HANDLE) -> Result<()> {
        if desc.is_null() {
            return Err(E_INVALIDARG.into());
        }
        // Acquire the surface lock and hold it until Unlock, matching
        // cnc-ddraw's `lock_surfaces`. While held, the game writes pixels into
        // the surface; the renderer thread also takes this lock during upload,
        // so it can only ever read a complete frame (no torn/blank captures).
        self.buffers.lock.acquire();
        if self.is_primary {
            static C: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            if C.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 24 {
                crate::dd_log!("DIAG PLOCK tid={}", self.buffers.lock.owner());
            }
        }
        unsafe {
            self.fill_desc2(&mut *desc, self.buffers.surface);
            if !rect.is_null() {
                let r = &*rect;
                let bytes_pp = (self.bpp / 8).max(1);
                let offset = (r.top * self.pitch) + (r.left * bytes_pp);
                (*desc).lpSurface = self.buffers.surface.add(offset as usize) as *mut core::ffi::c_void;
                (*desc).dwWidth = (r.right - r.left) as u32;
                (*desc).dwHeight = (r.bottom - r.top) as u32;
            }
        }
        Ok(())
    }

    fn lock_impl1(&self, rect: *mut RECT, desc: *mut DDSURFACEDESC, _flags: u32, _event: HANDLE) -> Result<()> {
        let mut d2 = DDSURFACEDESC2::default();
        self.lock_impl2(rect, &mut d2, 0, HANDLE(std::ptr::null_mut()))?;
        unsafe {
            (*desc).dwSize = std::mem::size_of::<DDSURFACEDESC>() as u32;
            (*desc).dwFlags = d2.dwFlags;
            (*desc).dwHeight = d2.dwHeight;
            (*desc).dwWidth = d2.dwWidth;
            (*desc).Anonymous1.lPitch = d2.Anonymous1.lPitch;
            (*desc).lpSurface = d2.lpSurface;
            (*desc).ddsCaps.dwCaps = d2.ddsCaps.dwCaps;
            fill_pixel_format(&mut (*desc).ddpfPixelFormat, self.bpp);
        }
        Ok(())
    }

    fn get_surface_desc_impl2(&self, desc: *mut DDSURFACEDESC2) -> Result<()> {
        if desc.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let _g = self.buffers.lock.lock();
        unsafe { self.fill_desc2(&mut *desc, self.buffers.surface) };
        Ok(())
    }

    fn get_surface_desc_impl1(&self, desc: *mut DDSURFACEDESC) -> Result<()> {
        if desc.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let _g = self.buffers.lock.lock();
        let mut d2 = DDSURFACEDESC2::default();
        unsafe { self.fill_desc2(&mut d2, self.buffers.surface) };
        unsafe {
            (*desc).dwSize = std::mem::size_of::<DDSURFACEDESC>() as u32;
            (*desc).dwFlags = d2.dwFlags;
            (*desc).dwHeight = d2.dwHeight;
            (*desc).dwWidth = d2.dwWidth;
            (*desc).Anonymous1.lPitch = d2.Anonymous1.lPitch;
            (*desc).lpSurface = d2.lpSurface;
            (*desc).ddsCaps.dwCaps = d2.ddsCaps.dwCaps;
            fill_pixel_format(&mut (*desc).ddpfPixelFormat, self.bpp);
        }
        Ok(())
    }

    fn get_caps_impl2(&self, caps: *mut DDSCAPS2) -> Result<()> {
        if !caps.is_null() {
            unsafe { (*caps).dwCaps = self.caps };
        }
        Ok(())
    }

    fn get_caps_impl1(&self, caps: *mut DDSCAPS) -> Result<()> {
        if !caps.is_null() {
            unsafe { (*caps).dwCaps = self.caps };
        }
        Ok(())
    }

    fn get_attached_impl2(&self, caps: *mut DDSCAPS2) -> Result<IDirectDrawSurface7> {
        if caps.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let requested = unsafe { (*caps).dwCaps };
        if (requested & DDSCAPS_BACKBUFFER as u32) != 0
            && let Some(ref bb) = self.backbuffer {
                let mut s = SurfaceImpl::from_buffers(bb.clone(), self.width, self.height, self.bpp, false);
                s.caps = (DDSCAPS_BACKBUFFER | DDSCAPS_OFFSCREENPLAIN | DDSCAPS_SYSTEMMEMORY) as u32;
                let surface: IDirectDrawSurface = s.into();
                return surface.cast::<IDirectDrawSurface7>();
            }
        Err(dderr(DXERR_GENERIC))
    }

    fn get_attached_impl1(&self, caps: *mut DDSCAPS) -> Result<IDirectDrawSurface7> {
        if caps.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let requested = unsafe { (*caps).dwCaps };
        if (requested & DDSCAPS_BACKBUFFER as u32) != 0
            && let Some(ref bb) = self.backbuffer {
                let mut s = SurfaceImpl::from_buffers(bb.clone(), self.width, self.height, self.bpp, false);
                s.caps = (DDSCAPS_BACKBUFFER | DDSCAPS_OFFSCREENPLAIN | DDSCAPS_SYSTEMMEMORY) as u32;
                let surface: IDirectDrawSurface = s.into();
                return surface.cast::<IDirectDrawSurface7>();
            }
        Err(dderr(DXERR_GENERIC))
    }

    fn get_dc_impl(&self, hdc: *mut HDC) -> Result<()> {
        if hdc.is_null() {
            return Err(E_INVALIDARG.into());
        }
        unsafe { *hdc = self.buffers.hdc };
        Ok(())
    }

    fn get_pixel_format_impl(&self, pf: *mut DDPIXELFORMAT) -> Result<()> {
        if pf.is_null() {
            return Err(E_INVALIDARG.into());
        }
        unsafe { fill_pixel_format(&mut *pf, self.bpp) };
        Ok(())
    }

    fn flip_impl(&self) -> Result<()> {
        if let Some(ref bb) = self.backbuffer {
            let _g = self.buffers.lock.lock();
            let _bg = bb.lock.lock();
            unsafe {
                let dst = self.buffers.surface;
                let src = bb.surface;
                let len = (self.pitch as usize) * (self.height as usize);
                std::ptr::copy_nonoverlapping(src, dst, len);
            }
        }
        if self.is_primary {
            crate::state::mark_dirty();
            crate::state::mark_frame_ready();
        }
        Ok(())
    }

    fn blt_impl(
        &self,
        dr: *mut RECT,
        src: Option<IDirectDrawSurface7>,
        sr: *mut RECT,
        flags: u32,
        fx: *mut DDBLTFX,
    ) -> Result<()> {
        crate::dd_log!("Surface::Blt(flags={:#x})", flags);

        // Copy the source pixels into a temporary buffer while holding ONLY the
        // source lock. Important: if the source is the same surface as `self`
        // (self-blit), locking `self.buffers.lock` here and then re-locking it
        // via `src_iface.Lock` would deadlock a non-reentrant mutex. Releasing
        // the source lock before acquiring `self`'s lock avoids that.
        let src_copy: Option<(Vec<u8>, usize, usize, usize)> = if let Some(src_iface) = src {
            let mut src_desc = DDSURFACEDESC2::default();
            if unsafe { src_iface.Lock(sr, &mut src_desc, 0, HANDLE(std::ptr::null_mut())).is_ok() } {
                let sp = src_desc.lpSurface as *mut u8;
                let spitch = unsafe { src_desc.Anonymous1.lPitch } as usize;
                let sw = src_desc.dwWidth as usize;
                let sh = src_desc.dwHeight as usize;
                let mut data = vec![0u8; sh * spitch];
                for y in 0..sh {
                    unsafe {
                        std::ptr::copy_nonoverlapping(sp.add(y * spitch), data.as_mut_ptr().add(y * spitch), spitch);
                    }
                }
                let _ = unsafe { src_iface.Unlock(std::ptr::null_mut::<RECT>()) };
                Some((data, sw, sh, spitch))
            } else {
                None
            }
        } else {
            None
        };

        let _guard = self.buffers.lock.lock();

        let dst_ptr = self.buffers.surface;
        let dst_pitch = self.pitch as usize;
        let dst_w = self.width as usize;
        let dst_h = self.height as usize;
        let bytes_pp = (self.bpp / 8).max(1) as usize;

        if (flags & DDBLT_COLORFILL as u32) != 0 && !fx.is_null() {
            let fill = unsafe { (*fx).Anonymous5.dwFillColor } as u16;
            let (x0, y0, x1, y1) = rect_clamp(dr, dst_w, dst_h);
            for y in y0..y1 {
                let row = dst_ptr as usize + y * dst_pitch;
                for x in x0..x1 {
                    unsafe {
                        *(row as *mut u16).add(x) = fill;
                    }
                }
            }
        }

        if let Some((data, src_w, src_h, src_pitch)) = src_copy {
            let (dx0, dy0, dx1, dy1) = rect_clamp(dr, dst_w, dst_h);
            let (sx0, sy0, sx1, sy1) = rect_clamp(sr, src_w, src_h);
            let dw = dx1 - dx0;
            let dh = dy1 - dy0;
            let sw = sx1 - sx0;
            let sh = sy1 - sy0;
            if dw == sw && dh == sh {
                for y in 0..dh {
                    let d = dst_ptr as usize + ((dy0 + y) * dst_pitch) + dx0 * bytes_pp;
                    let s = data.as_ptr() as usize + ((sy0 + y) * src_pitch) + sx0 * bytes_pp;
                    unsafe {
                        std::ptr::copy_nonoverlapping(s as *const u8, d as *mut u8, dw * bytes_pp);
                    }
                }
            } else if dw > 0 && dh > 0 && sw > 0 && sh > 0 {
                for y in 0..dh {
                    let sy = sy0 + (y * sh) / dh.max(1);
                    let d = dst_ptr as usize + ((dy0 + y) * dst_pitch) + dx0 * bytes_pp;
                    let s = data.as_ptr() as usize + (sy * src_pitch) + sx0 * bytes_pp;
                    for x in 0..dw {
                        let sx = sx0 + (x * sw) / dw.max(1);
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                (s + sx * bytes_pp) as *const u8,
                                (d + x * bytes_pp) as *mut u8,
                                bytes_pp,
                            );
                        }
                    }
                }
            }
        }
        if self.is_primary {
            static C: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            if C.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 24 {
                crate::dd_log!("DIAG PUNLOCK tid={}", self.buffers.lock.owner());
            }
            crate::state::mark_dirty();
        }
        Ok(())
    }
}

fn dderr(code: u32) -> Error {
    Error::from(HRESULT(code as i32))
}

/// Returns clamped (x0, y0, x1, y1) for a rectangle pointer (or whole surface).
fn rect_clamp(r: *mut RECT, w: usize, h: usize) -> (usize, usize, usize, usize) {
    if r.is_null() {
        return (0, 0, w, h);
    }
    unsafe {
        let rc = &*r;
        let x0 = rc.left.max(0) as usize;
        let y0 = rc.top.max(0) as usize;
        let x1 = (rc.right as usize).min(w);
        let y1 = (rc.bottom as usize).min(h);
        (x0, y0, x1.max(x0), y1.max(y0))
    }
}

// ===========================================================================
// IDirectDrawSurface (v1)
// ===========================================================================
impl IDirectDrawSurface_Impl for SurfaceImpl_Impl {
    fn AddAttachedSurface(&self, _s: Ref<'_, IDirectDrawSurface>) -> Result<()> {
        Ok(())
    }
    fn AddOverlayDirtyRect(&self, _r: *mut RECT) -> Result<()> {
        Ok(())
    }
    fn Blt(
        &self,
        dr: *mut RECT,
        src: Ref<'_, IDirectDrawSurface>,
        sr: *mut RECT,
        flags: u32,
        fx: *mut DDBLTFX,
    ) -> Result<()> {
        let s7 = src.as_ref().and_then(|s| s.cast::<IDirectDrawSurface7>().ok());
        self.blt_impl(dr, s7, sr, flags, fx)
    }
    fn BltBatch(&self, _b: *mut DDBLTBATCH, _c: u32, _f: u32) -> Result<()> {
        Ok(())
    }
    fn BltFast(&self, _x: u32, _y: u32, src: Ref<'_, IDirectDrawSurface>, r: *mut RECT, _t: u32) -> Result<()> {
        let s7 = src.as_ref().and_then(|s| s.cast::<IDirectDrawSurface7>().ok());
        self.blt_impl(std::ptr::null_mut(), s7, r, 0, std::ptr::null_mut())
    }
    fn DeleteAttachedSurface(&self, _f: u32, _s: Ref<'_, IDirectDrawSurface>) -> Result<()> {
        Ok(())
    }
    fn EnumAttachedSurfaces(&self, _c: *mut core::ffi::c_void, _cb: LPDDENUMSURFACESCALLBACK) -> Result<()> {
        Ok(())
    }
    fn EnumOverlayZOrders(&self, _f: u32, _c: *mut core::ffi::c_void, _cb: LPDDENUMSURFACESCALLBACK) -> Result<()> {
        Ok(())
    }
    fn Flip(&self, _target: Ref<'_, IDirectDrawSurface>, _flags: u32) -> Result<()> {
        self.flip_impl()
    }
    fn GetAttachedSurface(&self, caps: *mut DDSCAPS, out: OutRef<'_, IDirectDrawSurface>) -> Result<()> {
        let s7 = self.get_attached_impl1(caps)?;
        let s: IDirectDrawSurface = s7.cast()?;
        let _ = out.write(Some(s));
        Ok(())
    }
    fn GetBltStatus(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn GetCaps(&self, caps: *mut DDSCAPS) -> Result<()> {
        self.get_caps_impl1(caps)
    }
    fn GetClipper(&self) -> Result<IDirectDrawClipper> {
        Err(dderr(DXERR_GENERIC))
    }
    fn GetColorKey(&self, _f: u32, _k: *mut DDCOLORKEY) -> Result<()> {
        Ok(())
    }
    fn GetDC(&self, hdc: *mut HDC) -> Result<()> {
        self.get_dc_impl(hdc)
    }
    fn GetFlipStatus(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn GetOverlayPosition(&self, _x: *mut i32, _y: *mut i32) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn GetPalette(&self) -> Result<IDirectDrawPalette> {
        Err(dderr(DXERR_GENERIC))
    }
    fn GetPixelFormat(&self, pf: *mut DDPIXELFORMAT) -> Result<()> {
        self.get_pixel_format_impl(pf)
    }
    fn GetSurfaceDesc(&self, desc: *mut DDSURFACEDESC) -> Result<()> {
        self.get_surface_desc_impl1(desc)
    }
    fn Initialize(&self, _dd: Ref<'_, IDirectDraw>, _desc: *mut DDSURFACEDESC) -> Result<()> {
        Ok(())
    }
    fn IsLost(&self) -> Result<()> {
        Ok(())
    }
    fn Lock(&self, rect: *mut RECT, desc: *mut DDSURFACEDESC, flags: u32, event: HANDLE) -> Result<()> {
        self.lock_impl1(rect, desc, flags, event)
    }
    fn ReleaseDC(&self, _hdc: HDC) -> Result<()> {
        Ok(())
    }
    fn Restore(&self) -> Result<()> {
        Ok(())
    }
    fn SetClipper(&self, _c: Ref<'_, IDirectDrawClipper>) -> Result<()> {
        Ok(())
    }
    fn SetColorKey(&self, _f: u32, _k: *mut DDCOLORKEY) -> Result<()> {
        Ok(())
    }
    fn SetOverlayPosition(&self, _x: i32, _y: i32) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn SetPalette(&self, _p: Ref<'_, IDirectDrawPalette>) -> Result<()> {
        Ok(())
    }
    fn Unlock(&self, _r: *mut core::ffi::c_void) -> Result<()> {
        self.buffers.lock.release();
        if self.is_primary {
            static C: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            if C.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 24 {
                crate::dd_log!("DIAG PUNLOCK tid={}", self.buffers.lock.owner());
            }
            crate::state::mark_dirty();
        }
        Ok(())
    }
    fn UpdateOverlay(
        &self,
        _sr: *mut RECT,
        _s: Ref<'_, IDirectDrawSurface>,
        _dr: *mut RECT,
        _f: u32,
        _fx: *mut DDOVERLAYFX,
    ) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn UpdateOverlayDisplay(&self, _f: u32) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn UpdateOverlayZOrder(&self, _f: u32, _s: Ref<'_, IDirectDrawSurface>) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
}

// ===========================================================================
// IDirectDrawSurface2 (v2) -- same descriptors as v1, extra methods
// ===========================================================================
impl IDirectDrawSurface2_Impl for SurfaceImpl_Impl {
    fn AddAttachedSurface(&self, _s: Ref<'_, IDirectDrawSurface2>) -> Result<()> {
        Ok(())
    }
    fn AddOverlayDirtyRect(&self, _r: *mut RECT) -> Result<()> {
        Ok(())
    }
    fn Blt(
        &self,
        dr: *mut RECT,
        src: Ref<'_, IDirectDrawSurface2>,
        sr: *mut RECT,
        flags: u32,
        fx: *mut DDBLTFX,
    ) -> Result<()> {
        let s7 = src.as_ref().and_then(|s| s.cast::<IDirectDrawSurface7>().ok());
        self.blt_impl(dr, s7, sr, flags, fx)
    }
    fn BltBatch(&self, _b: *mut DDBLTBATCH, _c: u32, _f: u32) -> Result<()> {
        Ok(())
    }
    fn BltFast(&self, _x: u32, _y: u32, src: Ref<'_, IDirectDrawSurface2>, r: *mut RECT, _t: u32) -> Result<()> {
        let s7 = src.as_ref().and_then(|s| s.cast::<IDirectDrawSurface7>().ok());
        self.blt_impl(std::ptr::null_mut(), s7, r, 0, std::ptr::null_mut())
    }
    fn DeleteAttachedSurface(&self, _f: u32, _s: Ref<'_, IDirectDrawSurface2>) -> Result<()> {
        Ok(())
    }
    fn EnumAttachedSurfaces(&self, _c: *mut core::ffi::c_void, _cb: LPDDENUMSURFACESCALLBACK) -> Result<()> {
        Ok(())
    }
    fn EnumOverlayZOrders(&self, _f: u32, _c: *mut core::ffi::c_void, _cb: LPDDENUMSURFACESCALLBACK) -> Result<()> {
        Ok(())
    }
    fn Flip(&self, _target: Ref<'_, IDirectDrawSurface2>, _flags: u32) -> Result<()> {
        self.flip_impl()
    }
    fn GetAttachedSurface(&self, caps: *mut DDSCAPS, out: OutRef<'_, IDirectDrawSurface2>) -> Result<()> {
        let s7 = self.get_attached_impl1(caps)?;
        let s: IDirectDrawSurface2 = s7.cast()?;
        let _ = out.write(Some(s));
        Ok(())
    }
    fn GetBltStatus(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn GetCaps(&self, caps: *mut DDSCAPS) -> Result<()> {
        self.get_caps_impl1(caps)
    }
    fn GetClipper(&self) -> Result<IDirectDrawClipper> {
        Err(dderr(DXERR_GENERIC))
    }
    fn GetColorKey(&self, _f: u32, _k: *mut DDCOLORKEY) -> Result<()> {
        Ok(())
    }
    fn GetDC(&self, hdc: *mut HDC) -> Result<()> {
        self.get_dc_impl(hdc)
    }
    fn GetFlipStatus(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn GetOverlayPosition(&self, _x: *mut i32, _y: *mut i32) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn GetPalette(&self) -> Result<IDirectDrawPalette> {
        Err(dderr(DXERR_GENERIC))
    }
    fn GetPixelFormat(&self, pf: *mut DDPIXELFORMAT) -> Result<()> {
        self.get_pixel_format_impl(pf)
    }
    fn GetSurfaceDesc(&self, desc: *mut DDSURFACEDESC) -> Result<()> {
        self.get_surface_desc_impl1(desc)
    }
    fn Initialize(&self, _dd: Ref<'_, IDirectDraw>, _desc: *mut DDSURFACEDESC) -> Result<()> {
        Ok(())
    }
    fn IsLost(&self) -> Result<()> {
        Ok(())
    }
    fn Lock(&self, rect: *mut RECT, desc: *mut DDSURFACEDESC, flags: u32, event: HANDLE) -> Result<()> {
        self.lock_impl1(rect, desc, flags, event)
    }
    fn ReleaseDC(&self, _hdc: HDC) -> Result<()> {
        Ok(())
    }
    fn Restore(&self) -> Result<()> {
        Ok(())
    }
    fn SetClipper(&self, _c: Ref<'_, IDirectDrawClipper>) -> Result<()> {
        Ok(())
    }
    fn SetColorKey(&self, _f: u32, _k: *mut DDCOLORKEY) -> Result<()> {
        Ok(())
    }
    fn SetOverlayPosition(&self, _x: i32, _y: i32) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn SetPalette(&self, _p: Ref<'_, IDirectDrawPalette>) -> Result<()> {
        Ok(())
    }
    fn Unlock(&self, _r: *mut core::ffi::c_void) -> Result<()> {
        self.buffers.lock.release();
        if self.is_primary {
            static C: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            if C.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 24 {
                crate::dd_log!("DIAG PUNLOCK tid={}", self.buffers.lock.owner());
            }
            crate::state::mark_dirty();
        }
        Ok(())
    }
    fn UpdateOverlay(
        &self,
        _sr: *mut RECT,
        _s: Ref<'_, IDirectDrawSurface2>,
        _dr: *mut RECT,
        _f: u32,
        _fx: *mut DDOVERLAYFX,
    ) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn UpdateOverlayDisplay(&self, _f: u32) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn UpdateOverlayZOrder(&self, _f: u32, _s: Ref<'_, IDirectDrawSurface2>) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn GetDDInterface(&self, _p: *mut *mut core::ffi::c_void) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn PageLock(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn PageUnlock(&self, _f: u32) -> Result<()> {
        Ok(())
    }
}

// ===========================================================================
// IDirectDrawSurface3 (v3) -- DDSURFACEDESC/DDSCAPS, extra methods
// ===========================================================================
impl IDirectDrawSurface3_Impl for SurfaceImpl_Impl {
    fn AddAttachedSurface(&self, _s: Ref<'_, IDirectDrawSurface3>) -> Result<()> {
        Ok(())
    }
    fn AddOverlayDirtyRect(&self, _r: *mut RECT) -> Result<()> {
        Ok(())
    }
    fn Blt(
        &self,
        dr: *mut RECT,
        src: Ref<'_, IDirectDrawSurface3>,
        sr: *mut RECT,
        flags: u32,
        fx: *mut DDBLTFX,
    ) -> Result<()> {
        let s7 = src.as_ref().and_then(|s| s.cast::<IDirectDrawSurface7>().ok());
        self.blt_impl(dr, s7, sr, flags, fx)
    }
    fn BltBatch(&self, _b: *mut DDBLTBATCH, _c: u32, _f: u32) -> Result<()> {
        Ok(())
    }
    fn BltFast(&self, _x: u32, _y: u32, src: Ref<'_, IDirectDrawSurface3>, r: *mut RECT, _t: u32) -> Result<()> {
        let s7 = src.as_ref().and_then(|s| s.cast::<IDirectDrawSurface7>().ok());
        self.blt_impl(std::ptr::null_mut(), s7, r, 0, std::ptr::null_mut())
    }
    fn DeleteAttachedSurface(&self, _f: u32, _s: Ref<'_, IDirectDrawSurface3>) -> Result<()> {
        Ok(())
    }
    fn EnumAttachedSurfaces(&self, _c: *mut core::ffi::c_void, _cb: LPDDENUMSURFACESCALLBACK) -> Result<()> {
        Ok(())
    }
    fn EnumOverlayZOrders(&self, _f: u32, _c: *mut core::ffi::c_void, _cb: LPDDENUMSURFACESCALLBACK) -> Result<()> {
        Ok(())
    }
    fn Flip(&self, _target: Ref<'_, IDirectDrawSurface3>, _flags: u32) -> Result<()> {
        self.flip_impl()
    }
    fn GetAttachedSurface(&self, caps: *mut DDSCAPS, out: OutRef<'_, IDirectDrawSurface3>) -> Result<()> {
        let s7 = self.get_attached_impl1(caps)?;
        let s: IDirectDrawSurface3 = s7.cast()?;
        let _ = out.write(Some(s));
        Ok(())
    }
    fn GetBltStatus(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn GetCaps(&self, caps: *mut DDSCAPS) -> Result<()> {
        self.get_caps_impl1(caps)
    }
    fn GetClipper(&self) -> Result<IDirectDrawClipper> {
        Err(dderr(DXERR_GENERIC))
    }
    fn GetColorKey(&self, _f: u32, _k: *mut DDCOLORKEY) -> Result<()> {
        Ok(())
    }
    fn GetDC(&self, hdc: *mut HDC) -> Result<()> {
        self.get_dc_impl(hdc)
    }
    fn GetFlipStatus(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn GetOverlayPosition(&self, _x: *mut i32, _y: *mut i32) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn GetPalette(&self) -> Result<IDirectDrawPalette> {
        Err(dderr(DXERR_GENERIC))
    }
    fn GetPixelFormat(&self, pf: *mut DDPIXELFORMAT) -> Result<()> {
        self.get_pixel_format_impl(pf)
    }
    fn GetSurfaceDesc(&self, desc: *mut DDSURFACEDESC) -> Result<()> {
        self.get_surface_desc_impl1(desc)
    }
    fn Initialize(&self, _dd: Ref<'_, IDirectDraw>, _desc: *mut DDSURFACEDESC) -> Result<()> {
        Ok(())
    }
    fn IsLost(&self) -> Result<()> {
        Ok(())
    }
    fn Lock(&self, rect: *mut RECT, desc: *mut DDSURFACEDESC, flags: u32, event: HANDLE) -> Result<()> {
        self.lock_impl1(rect, desc, flags, event)
    }
    fn ReleaseDC(&self, _hdc: HDC) -> Result<()> {
        Ok(())
    }
    fn Restore(&self) -> Result<()> {
        Ok(())
    }
    fn SetClipper(&self, _c: Ref<'_, IDirectDrawClipper>) -> Result<()> {
        Ok(())
    }
    fn SetColorKey(&self, _f: u32, _k: *mut DDCOLORKEY) -> Result<()> {
        Ok(())
    }
    fn SetOverlayPosition(&self, _x: i32, _y: i32) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn SetPalette(&self, _p: Ref<'_, IDirectDrawPalette>) -> Result<()> {
        Ok(())
    }
    fn Unlock(&self, _r: *mut core::ffi::c_void) -> Result<()> {
        self.buffers.lock.release();
        if self.is_primary {
            static C: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            if C.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 24 {
                crate::dd_log!("DIAG PUNLOCK tid={}", self.buffers.lock.owner());
            }
            crate::state::mark_dirty();
        }
        Ok(())
    }
    fn UpdateOverlay(
        &self,
        _sr: *mut RECT,
        _s: Ref<'_, IDirectDrawSurface3>,
        _dr: *mut RECT,
        _f: u32,
        _fx: *mut DDOVERLAYFX,
    ) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn UpdateOverlayDisplay(&self, _f: u32) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn UpdateOverlayZOrder(&self, _f: u32, _s: Ref<'_, IDirectDrawSurface3>) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn GetDDInterface(&self, _p: *mut *mut core::ffi::c_void) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn PageLock(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn PageUnlock(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn SetSurfaceDesc(&self, _d: *mut DDSURFACEDESC, _f: u32) -> Result<()> {
        Ok(())
    }
}

// ===========================================================================
// IDirectDrawSurface4 (v4) -- DDSURFACEDESC2/DDSCAPS2
// ===========================================================================
impl IDirectDrawSurface4_Impl for SurfaceImpl_Impl {
    fn AddAttachedSurface(&self, _s: Ref<'_, IDirectDrawSurface4>) -> Result<()> {
        Ok(())
    }
    fn AddOverlayDirtyRect(&self, _r: *mut RECT) -> Result<()> {
        Ok(())
    }
    fn Blt(
        &self,
        dr: *mut RECT,
        src: Ref<'_, IDirectDrawSurface4>,
        sr: *mut RECT,
        flags: u32,
        fx: *mut DDBLTFX,
    ) -> Result<()> {
        let s7 = src.as_ref().and_then(|s| s.cast::<IDirectDrawSurface7>().ok());
        self.blt_impl(dr, s7, sr, flags, fx)
    }
    fn BltBatch(&self, _b: *mut DDBLTBATCH, _c: u32, _f: u32) -> Result<()> {
        Ok(())
    }
    fn BltFast(&self, _x: u32, _y: u32, src: Ref<'_, IDirectDrawSurface4>, r: *mut RECT, _t: u32) -> Result<()> {
        let s7 = src.as_ref().and_then(|s| s.cast::<IDirectDrawSurface7>().ok());
        self.blt_impl(std::ptr::null_mut(), s7, r, 0, std::ptr::null_mut())
    }
    fn DeleteAttachedSurface(&self, _f: u32, _s: Ref<'_, IDirectDrawSurface4>) -> Result<()> {
        Ok(())
    }
    fn EnumAttachedSurfaces(&self, _c: *mut core::ffi::c_void, _cb: LPDDENUMSURFACESCALLBACK2) -> Result<()> {
        Ok(())
    }
    fn EnumOverlayZOrders(&self, _f: u32, _c: *mut core::ffi::c_void, _cb: LPDDENUMSURFACESCALLBACK2) -> Result<()> {
        Ok(())
    }
    fn Flip(&self, _target: Ref<'_, IDirectDrawSurface4>, _flags: u32) -> Result<()> {
        self.flip_impl()
    }
    fn GetAttachedSurface(&self, caps: *mut DDSCAPS2, out: OutRef<'_, IDirectDrawSurface4>) -> Result<()> {
        let s7 = self.get_attached_impl2(caps)?;
        let s: IDirectDrawSurface4 = s7.cast()?;
        let _ = out.write(Some(s));
        Ok(())
    }
    fn GetBltStatus(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn GetCaps(&self, caps: *mut DDSCAPS2) -> Result<()> {
        self.get_caps_impl2(caps)
    }
    fn GetClipper(&self) -> Result<IDirectDrawClipper> {
        Err(dderr(DXERR_GENERIC))
    }
    fn GetColorKey(&self, _f: u32, _k: *mut DDCOLORKEY) -> Result<()> {
        Ok(())
    }
    fn GetDC(&self, hdc: *mut HDC) -> Result<()> {
        self.get_dc_impl(hdc)
    }
    fn GetFlipStatus(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn GetOverlayPosition(&self, _x: *mut i32, _y: *mut i32) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn GetPalette(&self) -> Result<IDirectDrawPalette> {
        Err(dderr(DXERR_GENERIC))
    }
    fn GetPixelFormat(&self, pf: *mut DDPIXELFORMAT) -> Result<()> {
        self.get_pixel_format_impl(pf)
    }
    fn GetSurfaceDesc(&self, desc: *mut DDSURFACEDESC2) -> Result<()> {
        self.get_surface_desc_impl2(desc)
    }
    fn Initialize(&self, _dd: Ref<'_, IDirectDraw>, _desc: *mut DDSURFACEDESC2) -> Result<()> {
        Ok(())
    }
    fn IsLost(&self) -> Result<()> {
        Ok(())
    }
    fn Lock(&self, rect: *mut RECT, desc: *mut DDSURFACEDESC2, flags: u32, event: HANDLE) -> Result<()> {
        self.lock_impl2(rect, desc, flags, event)
    }
    fn ReleaseDC(&self, _hdc: HDC) -> Result<()> {
        Ok(())
    }
    fn Restore(&self) -> Result<()> {
        Ok(())
    }
    fn SetClipper(&self, _c: Ref<'_, IDirectDrawClipper>) -> Result<()> {
        Ok(())
    }
    fn SetColorKey(&self, _f: u32, _k: *mut DDCOLORKEY) -> Result<()> {
        Ok(())
    }
    fn SetOverlayPosition(&self, _x: i32, _y: i32) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn SetPalette(&self, _p: Ref<'_, IDirectDrawPalette>) -> Result<()> {
        Ok(())
    }
    fn Unlock(&self, _r: *mut RECT) -> Result<()> {
        self.buffers.lock.release();
        if self.is_primary {
            static C: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            if C.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 24 {
                crate::dd_log!("DIAG PUNLOCK tid={}", self.buffers.lock.owner());
            }
            crate::state::mark_dirty();
        }
        Ok(())
    }
    fn UpdateOverlay(
        &self,
        _sr: *mut RECT,
        _s: Ref<'_, IDirectDrawSurface4>,
        _dr: *mut RECT,
        _f: u32,
        _fx: *mut DDOVERLAYFX,
    ) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn UpdateOverlayDisplay(&self, _f: u32) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn UpdateOverlayZOrder(&self, _f: u32, _s: Ref<'_, IDirectDrawSurface4>) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn GetDDInterface(&self, _p: *mut *mut core::ffi::c_void) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn PageLock(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn PageUnlock(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn SetSurfaceDesc(&self, _d: *mut DDSURFACEDESC2, _f: u32) -> Result<()> {
        Ok(())
    }
    fn SetPrivateData(&self, _g: *const GUID, _p: *mut core::ffi::c_void, _s: u32, _f: u32) -> Result<()> {
        Ok(())
    }
    fn GetPrivateData(&self, _g: *const GUID, _p: *mut core::ffi::c_void, _s: *mut u32) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn FreePrivateData(&self, _g: *const GUID) -> Result<()> {
        Ok(())
    }
    fn GetUniquenessValue(&self, v: *mut u32) -> Result<()> {
        if !v.is_null() {
            unsafe { *v = 0 }
        }
        Ok(())
    }
    fn ChangeUniquenessValue(&self) -> Result<()> {
        Ok(())
    }
}

// ===========================================================================
// IDirectDrawSurface7 (v7) -- DDSURFACEDESC2/DDSCAPS2
// ===========================================================================
impl IDirectDrawSurface7_Impl for SurfaceImpl_Impl {
    fn AddAttachedSurface(&self, _s: Ref<'_, IDirectDrawSurface7>) -> Result<()> {
        Ok(())
    }
    fn AddOverlayDirtyRect(&self, _r: *mut RECT) -> Result<()> {
        Ok(())
    }
    fn Blt(
        &self,
        dr: *mut RECT,
        src: Ref<'_, IDirectDrawSurface7>,
        sr: *mut RECT,
        flags: u32,
        fx: *mut DDBLTFX,
    ) -> Result<()> {
        let s7 = src.as_ref().and_then(|s| s.cast::<IDirectDrawSurface7>().ok());
        self.blt_impl(dr, s7, sr, flags, fx)
    }
    fn BltBatch(&self, _b: *mut DDBLTBATCH, _c: u32, _f: u32) -> Result<()> {
        Ok(())
    }
    fn BltFast(&self, _x: u32, _y: u32, src: Ref<'_, IDirectDrawSurface7>, r: *mut RECT, _t: u32) -> Result<()> {
        let s7 = src.as_ref().and_then(|s| s.cast::<IDirectDrawSurface7>().ok());
        self.blt_impl(std::ptr::null_mut(), s7, r, 0, std::ptr::null_mut())
    }
    fn DeleteAttachedSurface(&self, _f: u32, _s: Ref<'_, IDirectDrawSurface7>) -> Result<()> {
        Ok(())
    }
    fn EnumAttachedSurfaces(&self, _c: *mut core::ffi::c_void, _cb: LPDDENUMSURFACESCALLBACK7) -> Result<()> {
        Ok(())
    }
    fn EnumOverlayZOrders(&self, _f: u32, _c: *mut core::ffi::c_void, _cb: LPDDENUMSURFACESCALLBACK7) -> Result<()> {
        Ok(())
    }
    fn Flip(&self, _target: Ref<'_, IDirectDrawSurface7>, _flags: u32) -> Result<()> {
        self.flip_impl()
    }
    fn GetAttachedSurface(&self, caps: *mut DDSCAPS2, out: OutRef<'_, IDirectDrawSurface7>) -> Result<()> {
        let s7 = self.get_attached_impl2(caps)?;
        let _ = out.write(Some(s7));
        Ok(())
    }
    fn GetBltStatus(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn GetCaps(&self, caps: *mut DDSCAPS2) -> Result<()> {
        self.get_caps_impl2(caps)
    }
    fn GetClipper(&self) -> Result<IDirectDrawClipper> {
        Err(dderr(DXERR_GENERIC))
    }
    fn GetColorKey(&self, _f: u32, _k: *mut DDCOLORKEY) -> Result<()> {
        Ok(())
    }
    fn GetDC(&self, hdc: *mut HDC) -> Result<()> {
        self.get_dc_impl(hdc)
    }
    fn GetFlipStatus(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn GetOverlayPosition(&self, _x: *mut i32, _y: *mut i32) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn GetPalette(&self) -> Result<IDirectDrawPalette> {
        Err(dderr(DXERR_GENERIC))
    }
    fn GetPixelFormat(&self, pf: *mut DDPIXELFORMAT) -> Result<()> {
        self.get_pixel_format_impl(pf)
    }
    fn GetSurfaceDesc(&self, desc: *mut DDSURFACEDESC2) -> Result<()> {
        self.get_surface_desc_impl2(desc)
    }
    fn Initialize(&self, _dd: Ref<'_, IDirectDraw>, _desc: *mut DDSURFACEDESC2) -> Result<()> {
        Ok(())
    }
    fn IsLost(&self) -> Result<()> {
        Ok(())
    }
    fn Lock(&self, rect: *mut RECT, desc: *mut DDSURFACEDESC2, flags: u32, event: HANDLE) -> Result<()> {
        self.lock_impl2(rect, desc, flags, event)
    }
    fn ReleaseDC(&self, _hdc: HDC) -> Result<()> {
        Ok(())
    }
    fn Restore(&self) -> Result<()> {
        Ok(())
    }
    fn SetClipper(&self, _c: Ref<'_, IDirectDrawClipper>) -> Result<()> {
        Ok(())
    }
    fn SetColorKey(&self, _f: u32, _k: *mut DDCOLORKEY) -> Result<()> {
        Ok(())
    }
    fn SetOverlayPosition(&self, _x: i32, _y: i32) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn SetPalette(&self, _p: Ref<'_, IDirectDrawPalette>) -> Result<()> {
        Ok(())
    }
    fn Unlock(&self, _r: *mut RECT) -> Result<()> {
        self.buffers.lock.release();
        if self.is_primary {
            static C: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            if C.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 24 {
                crate::dd_log!("DIAG PUNLOCK tid={}", self.buffers.lock.owner());
            }
            crate::state::mark_dirty();
        }
        Ok(())
    }
    fn UpdateOverlay(
        &self,
        _sr: *mut RECT,
        _s: Ref<'_, IDirectDrawSurface7>,
        _dr: *mut RECT,
        _f: u32,
        _fx: *mut DDOVERLAYFX,
    ) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn UpdateOverlayDisplay(&self, _f: u32) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn UpdateOverlayZOrder(&self, _f: u32, _s: Ref<'_, IDirectDrawSurface7>) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn GetDDInterface(&self, _p: *mut *mut core::ffi::c_void) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn PageLock(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn PageUnlock(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn SetSurfaceDesc(&self, _d: *mut DDSURFACEDESC2, _f: u32) -> Result<()> {
        Ok(())
    }
    fn SetPrivateData(&self, _g: *const GUID, _p: *mut core::ffi::c_void, _s: u32, _f: u32) -> Result<()> {
        Ok(())
    }
    fn GetPrivateData(&self, _g: *const GUID, _p: *mut core::ffi::c_void, _s: *mut u32) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn FreePrivateData(&self, _g: *const GUID) -> Result<()> {
        Ok(())
    }
    fn GetUniquenessValue(&self, v: *mut u32) -> Result<()> {
        if !v.is_null() {
            unsafe { *v = 0 }
        }
        Ok(())
    }
    fn ChangeUniquenessValue(&self) -> Result<()> {
        Ok(())
    }
    fn SetPriority(&self, _p: u32) -> Result<()> {
        Ok(())
    }
    fn GetPriority(&self, _p: *mut u32) -> Result<()> {
        Ok(())
    }
    fn SetLOD(&self, _l: u32) -> Result<()> {
        Ok(())
    }
    fn GetLOD(&self, _l: *mut u32) -> Result<()> {
        Ok(())
    }
}
