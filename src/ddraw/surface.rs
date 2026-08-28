use std::cell::RefCell;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::DirectDraw::*;
use windows::Win32::Graphics::Gdi::HDC;

#[implement(IDirectDrawSurface)]
pub struct SurfaceImpl {
    pub width: u32,
    pub height: u32,
    pub bpp: u32,
    pub pitch: u32,
    pub pixels: RefCell<Vec<u8>>,
    pub is_primary: bool,
    pub caps: u32,
    pub backbuffer_pixels: Option<RefCell<Vec<u8>>>,
}

impl SurfaceImpl {
    pub fn new(width: u32, height: u32, bpp: u32, is_primary: bool) -> Self {
        let bytes_pp = (bpp / 8).max(1);
        let pitch = ((width * bytes_pp + 63) & !63) as u32;
        let size = (pitch * height) as usize;
        Self {
            width,
            height,
            bpp,
            pitch,
            pixels: RefCell::new(vec![0; size]),
            is_primary,
            caps: if is_primary {
                DDSCAPS_PRIMARYSURFACE as u32 | DDSCAPS_VIDEOMEMORY as u32
            } else {
                DDSCAPS_OFFSCREENPLAIN as u32 | DDSCAPS_SYSTEMMEMORY as u32
            },
            backbuffer_pixels: None,
        }
    }

    fn fill_surface_desc(&self, desc: &mut DDSURFACEDESC, pixels_ptr: *const u8) {
        desc.dwSize = std::mem::size_of::<DDSURFACEDESC>() as u32;
        desc.dwFlags = (DDSD_CAPS | DDSD_WIDTH | DDSD_HEIGHT | DDSD_PITCH | DDSD_PIXELFORMAT) as u32;
        desc.dwWidth = self.width;
        desc.dwHeight = self.height;
        desc.Anonymous1.lPitch = self.pitch as i32;
        desc.lpSurface = pixels_ptr as *mut core::ffi::c_void;
        desc.ddsCaps.dwCaps = self.caps;
        fill_pixel_format(&mut desc.ddpfPixelFormat, self.bpp);
    }
}

fn fill_pixel_format(pf: &mut DDPIXELFORMAT, bpp: u32) {
    pf.dwSize = std::mem::size_of::<DDPIXELFORMAT>() as u32;
    pf.dwFlags = DDPF_RGB as u32;
    pf.Anonymous1.dwRGBBitCount = bpp;
    match bpp {
        8 => { pf.dwFlags = (DDPF_PALETTEINDEXED8 | DDPF_RGB) as u32; }
        16 => {
            pf.Anonymous2.dwRBitMask = 0xF800;
            pf.Anonymous3.dwGBitMask = 0x07E0;
            pf.Anonymous4.dwBBitMask = 0x001F;
        }
        24 | 32 => {
            pf.Anonymous2.dwRBitMask = 0xFF0000;
            pf.Anonymous3.dwGBitMask = 0x00FF00;
            pf.Anonymous4.dwBBitMask = 0x0000FF;
        }
        _ => {}
    }
}

fn dderr(code: u32) -> Error {
    Error::from(HRESULT(code as i32))
}

impl IDirectDrawSurface_Impl for SurfaceImpl_Impl {
    fn AddAttachedSurface(&self, _s: Ref<'_, IDirectDrawSurface>) -> Result<()> { Ok(()) }
    fn AddOverlayDirtyRect(&self, _r: *mut RECT) -> Result<()> { Ok(()) }
    fn Blt(&self, dr: *mut RECT, _s: Ref<'_, IDirectDrawSurface>, sr: *mut RECT, flags: u32, _fx: *mut DDBLTFX) -> Result<()> {
        crate::dd_log!("Surface::Blt(dst={:p}, src={:p}, flags={:#x})", dr, sr, flags);
        Ok(())
    }
    fn BltBatch(&self, _b: *mut DDBLTBATCH, _c: u32, _f: u32) -> Result<()> { Ok(()) }
    fn BltFast(&self, _x: u32, _y: u32, _s: Ref<'_, IDirectDrawSurface>, _r: *mut RECT, _t: u32) -> Result<()> { Ok(()) }
    fn DeleteAttachedSurface(&self, _f: u32, _s: Ref<'_, IDirectDrawSurface>) -> Result<()> { Ok(()) }
    fn EnumAttachedSurfaces(&self, _c: *mut core::ffi::c_void, _cb: LPDDENUMSURFACESCALLBACK) -> Result<()> { Ok(()) }
    fn EnumOverlayZOrders(&self, _f: u32, _c: *mut core::ffi::c_void, _cb: LPDDENUMSURFACESCALLBACK) -> Result<()> { Ok(()) }

    fn Flip(&self, _target: Ref<'_, IDirectDrawSurface>, flags: u32) -> Result<()> {
        crate::dd_log!("Surface::Flip(flags={:#x})", flags);
        if let Some(ref back) = self.backbuffer_pixels {
            let mut front = self.pixels.borrow_mut();
            let back_buf = back.borrow();
            front.copy_from_slice(&back_buf);
            crate::dd_log!("  -> DD_OK (swapped front/back)");
        } else {
            crate::dd_log!("  -> DD_OK (no backbuffer)");
        }
        Ok(())
    }

    fn GetAttachedSurface(&self, caps: *mut DDSCAPS, out: OutRef<'_, IDirectDrawSurface>) -> Result<()> {
        crate::dd_log!("Surface::GetAttachedSurface(caps={:p})", caps);
        if caps.is_null() { return Err(E_INVALIDARG.into()); }
        let requested = unsafe { (*caps).dwCaps };
        crate::dd_log!("  requested caps={:#x}, has_backbuffer={}", requested, self.backbuffer_pixels.is_some());
        if (requested & DDSCAPS_BACKBUFFER as u32) != 0 && self.backbuffer_pixels.is_some() {
            let mut bb = SurfaceImpl::new(self.width, self.height, self.bpp, false);
            bb.caps = (DDSCAPS_BACKBUFFER | DDSCAPS_OFFSCREENPLAIN | DDSCAPS_SYSTEMMEMORY) as u32;
            if let Some(ref bp) = self.backbuffer_pixels {
                bb.pixels = RefCell::new(bp.borrow().clone());
            }
            let surface: IDirectDrawSurface = bb.into();
            let _ = out.write(Some(surface));
            crate::dd_log!("  -> DD_OK (backbuffer returned)");
            return Ok(());
        }
        crate::dd_log!("  -> DXERR_GENERIC");
        Err(dderr(DXERR_GENERIC))
    }

    fn GetBltStatus(&self, _f: u32) -> Result<()> { Ok(()) }
    fn GetCaps(&self, caps: *mut DDSCAPS) -> Result<()> {
        if !caps.is_null() { unsafe { (*caps).dwCaps = self.caps }; }
        Ok(())
    }
    fn GetClipper(&self) -> Result<IDirectDrawClipper> { Err(dderr(DXERR_GENERIC)) }
    fn GetColorKey(&self, _f: u32, _k: *mut DDCOLORKEY) -> Result<()> { Ok(()) }

    fn GetDC(&self, hdc: *mut HDC) -> Result<()> {
        crate::dd_log!("Surface::GetDC(hdc={:p})", hdc);
        if hdc.is_null() { return Err(E_INVALIDARG.into()); }
        unsafe { *hdc = HDC::default() };
        crate::dd_log!("  -> DD_OK");
        Ok(())
    }

    fn GetFlipStatus(&self, _f: u32) -> Result<()> { Ok(()) }
    fn GetOverlayPosition(&self, _x: *mut i32, _y: *mut i32) -> Result<()> { Err(dderr(DXERR_GENERIC)) }
    fn GetPalette(&self) -> Result<IDirectDrawPalette> { Err(dderr(DXERR_GENERIC)) }

    fn GetPixelFormat(&self, pf: *mut DDPIXELFORMAT) -> Result<()> {
        if pf.is_null() { return Err(E_INVALIDARG.into()); }
        unsafe { fill_pixel_format(&mut *pf, self.bpp) };
        Ok(())
    }

    fn GetSurfaceDesc(&self, desc: *mut DDSURFACEDESC) -> Result<()> {
        if desc.is_null() { return Err(E_INVALIDARG.into()); }
        let pixels = self.pixels.borrow();
        unsafe { self.fill_surface_desc(&mut *desc, pixels.as_ptr()) };
        Ok(())
    }

    fn Initialize(&self, _dd: Ref<'_, IDirectDraw>, _desc: *mut DDSURFACEDESC) -> Result<()> { Ok(()) }
    fn IsLost(&self) -> Result<()> { Ok(()) }

    fn Lock(&self, rect: *mut RECT, desc: *mut DDSURFACEDESC, flags: u32, _event: HANDLE) -> Result<()> {
        crate::dd_log!("Surface::Lock(rect={:p}, desc={:p}, flags={:#x})", rect, desc, flags);
        if desc.is_null() { return Err(E_INVALIDARG.into()); }
        let pixels = self.pixels.borrow();
        unsafe {
            self.fill_surface_desc(&mut *desc, pixels.as_ptr());
            if !rect.is_null() {
                let r = &*rect;
                let bytes_pp = (self.bpp / 8).max(1) as u32;
                let offset = (r.top as u32 * self.pitch) + (r.left as u32 * bytes_pp);
                (*desc).lpSurface = pixels.as_ptr().add(offset as usize) as *mut core::ffi::c_void;
                (*desc).dwWidth = (r.right - r.left) as u32;
                (*desc).dwHeight = (r.bottom - r.top) as u32;
            }
        }
        crate::dd_log!("  -> DD_OK ({}x{}@{}bpp)", self.width, self.height, self.bpp);
        Ok(())
    }

    fn ReleaseDC(&self, _hdc: HDC) -> Result<()> { Ok(()) }
    fn Restore(&self) -> Result<()> { Ok(()) }
    fn SetClipper(&self, _c: Ref<'_, IDirectDrawClipper>) -> Result<()> { Ok(()) }
    fn SetColorKey(&self, _f: u32, _k: *mut DDCOLORKEY) -> Result<()> { Ok(()) }
    fn SetOverlayPosition(&self, _x: i32, _y: i32) -> Result<()> { Err(dderr(DXERR_GENERIC)) }
    fn SetPalette(&self, _p: Ref<'_, IDirectDrawPalette>) -> Result<()> { Ok(()) }
    fn Unlock(&self, _r: *mut core::ffi::c_void) -> Result<()> {
        crate::dd_log!("Surface::Unlock -> DD_OK");
        Ok(())
    }
    fn UpdateOverlay(&self, _sr: *mut RECT, _s: Ref<'_, IDirectDrawSurface>, _dr: *mut RECT, _f: u32, _fx: *mut DDOVERLAYFX) -> Result<()> { Err(dderr(DXERR_GENERIC)) }
    fn UpdateOverlayDisplay(&self, _f: u32) -> Result<()> { Err(dderr(DXERR_GENERIC)) }
    fn UpdateOverlayZOrder(&self, _f: u32, _s: Ref<'_, IDirectDrawSurface>) -> Result<()> { Err(dderr(DXERR_GENERIC)) }
}
