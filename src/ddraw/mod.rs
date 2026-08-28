pub(crate) mod clipper;
pub(crate) mod palette;
pub(crate) mod surface;

use std::cell::UnsafeCell;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::DirectDraw::*;
use windows::Win32::Graphics::Gdi::{HDC, PALETTEENTRY};

use self::clipper::ClipperImpl;
use self::palette::PaletteImpl;
use self::surface::SurfaceImpl;

// Global DirectDraw state
pub(crate) struct GlobalState {
    pub width: u32,
    pub height: u32,
    pub bpp: u32,
    pub hwnd: isize,
    pub cooperative_level: u32,
}

impl Default for GlobalState {
    fn default() -> Self {
        Self { width: 640, height: 480, bpp: 16, hwnd: 0, cooperative_level: 0 }
    }
}

struct GlobalCell(UnsafeCell<GlobalState>);
unsafe impl Sync for GlobalCell {}

static GLOBAL: GlobalCell = GlobalCell(UnsafeCell::new(GlobalState {
    width: 640, height: 480, bpp: 16, hwnd: 0, cooperative_level: 0,
}));

fn with_global<F, R>(f: F) -> R where F: FnOnce(&mut GlobalState) -> R {
    unsafe { f(&mut *GLOBAL.0.get()) }
}

const RESOLUTIONS: &[(u32, u32)] = &[
    (320, 200), (320, 240), (640, 400), (640, 480),
    (800, 600), (1024, 768), (1280, 720), (1280, 1024),
    (1600, 900), (1600, 1200), (1920, 1080),
];

const DDERR_INVALIDMODE: HRESULT = HRESULT(0x8876_0248u32 as i32);

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

fn fill_mode_desc(desc: &mut DDSURFACEDESC, width: u32, height: u32, bpp: u32) {
    desc.dwSize = std::mem::size_of::<DDSURFACEDESC>() as u32;
    desc.dwFlags = (DDSD_WIDTH | DDSD_HEIGHT | DDSD_PITCH | DDSD_PIXELFORMAT | DDSD_REFRESHRATE) as u32;
    desc.dwWidth = width;
    desc.dwHeight = height;
    desc.Anonymous2.dwRefreshRate = 60;
    let bytes_pp = (bpp / 8).max(1);
    desc.Anonymous1.lPitch = ((width * bytes_pp + 63) & !63) as i32;
    fill_pixel_format(&mut desc.ddpfPixelFormat, bpp);
}

fn enum_display_modes(
    filter_desc: *mut DDSURFACEDESC,
    context: *mut core::ffi::c_void,
    callback: LPDDENUMMODESCALLBACK,
) -> Result<()> {
    let callback = match callback { Some(f) => f, None => return Ok(()) };
    let filter_res = if !filter_desc.is_null() {
        let desc = unsafe { &*filter_desc };
        if (desc.dwFlags & (DDSD_WIDTH | DDSD_HEIGHT) as u32) == (DDSD_WIDTH | DDSD_HEIGHT) as u32 {
            Some((desc.dwWidth, desc.dwHeight))
        } else { None }
    } else { None };

    for &(w, h) in RESOLUTIONS {
        if let Some((fw, fh)) = filter_res {
            if w != fw || h != fh { continue; }
        }
        for &bpp in &[8u32, 16, 32] {
            let mut desc = DDSURFACEDESC::default();
            fill_mode_desc(&mut desc, w, h, bpp);
            let hr = unsafe { callback(&mut desc, context) };
            if hr == HRESULT(0) { return Ok(()); }
        }
    }
    Ok(())
}

fn fill_desc_v1(desc: &mut DDSURFACEDESC) {
    with_global(|g| {
        desc.dwSize = std::mem::size_of::<DDSURFACEDESC>() as u32;
        desc.dwFlags = (DDSD_WIDTH | DDSD_HEIGHT | DDSD_PIXELFORMAT) as u32;
        desc.dwWidth = g.width;
        desc.dwHeight = g.height;
        let bytes_pp = (g.bpp / 8).max(1);
        desc.Anonymous1.lPitch = ((g.width * bytes_pp + 63) & !63) as i32;
        fill_pixel_format(&mut desc.ddpfPixelFormat, g.bpp);
    });
}

fn fill_desc_v2(desc: &mut DDSURFACEDESC2) {
    with_global(|g| {
        desc.dwSize = std::mem::size_of::<DDSURFACEDESC2>() as u32;
        desc.dwFlags = (DDSD_WIDTH | DDSD_HEIGHT | DDSD_PIXELFORMAT) as u32;
        desc.dwWidth = g.width;
        desc.dwHeight = g.height;
        let bytes_pp = (g.bpp / 8).max(1);
        desc.Anonymous1.lPitch = ((g.width * bytes_pp + 63) & !63) as i32;
        unsafe { fill_pixel_format(&mut desc.Anonymous5.ddpfPixelFormat, g.bpp) };
    });
}

fn fill_caps(caps: &mut DDCAPS_DX7) {
    caps.dwSize = std::mem::size_of::<DDCAPS_DX7>() as u32;
    caps.dwCaps = (DDCAPS_BLT | DDCAPS_PALETTE | DDCAPS_BLTCOLORFILL | DDCAPS_BLTSTRETCH
        | DDCAPS_CANCLIP | DDCAPS_CANCLIPSTRETCHED | DDCAPS_COLORKEY) as u32;
    caps.dwCaps2 = DDCAPS2_NOPAGELOCKREQUIRED as u32;
    caps.dwPalCaps = (DDPCAPS_8BIT | DDPCAPS_PRIMARYSURFACE) as u32;
    caps.dwVidMemTotal = 16 * 1024 * 1024;
    caps.dwVidMemFree = 16 * 1024 * 1024;
    caps.ddsCaps.dwCaps = (DDSCAPS_BACKBUFFER | DDSCAPS_COMPLEX | DDSCAPS_FLIP
        | DDSCAPS_FRONTBUFFER | DDSCAPS_OFFSCREENPLAIN | DDSCAPS_PRIMARYSURFACE
        | DDSCAPS_VIDEOMEMORY | DDSCAPS_OWNDC | DDSCAPS_LOCALVIDMEM) as u32;
}

fn create_surface_from_desc(desc_caps: u32, desc_width: u32, desc_height: u32, desc_bpp: u32, backbuffer_count: u32) -> Result<IDirectDrawSurface> {
    let is_primary = (desc_caps & DDSCAPS_PRIMARYSURFACE as u32) != 0;
    let has_flip = (desc_caps & DDSCAPS_FLIP as u32) != 0;
    let (width, height, bpp) = if is_primary {
        with_global(|g| (g.width, g.height, g.bpp))
    } else {
        let b = if desc_bpp > 0 { desc_bpp } else { with_global(|g| g.bpp) };
        (desc_width, desc_height, b)
    };
    let mut surface = SurfaceImpl::new(width, height, bpp, is_primary);
    if is_primary && has_flip {
        surface.caps |= DDSCAPS_FRONTBUFFER as u32;
        if backbuffer_count > 0 {
            surface.backbuffer_pixels = Some(std::cell::RefCell::new(vec![0; (surface.pitch * surface.height) as usize]));
        }
    }
    let surface: IDirectDrawSurface = surface.into();
    Ok(surface)
}

fn set_display_mode(width: u32, height: u32, bpp: u32) -> Result<()> {
    crate::dd_log!("  set_display_mode({}, {}, {})", width, height, bpp);
    if bpp != 0 && bpp != 8 && bpp != 16 && bpp != 32 {
        crate::dd_log!("  -> DDERR_INVALIDMODE (bpp={})", bpp);
        return Err(Error::from(DDERR_INVALIDMODE));
    }
    with_global(|g| {
        g.width = if width > 0 { width } else { 640 };
        g.height = if height > 0 { height } else { 480 };
        g.bpp = if bpp > 0 { bpp } else { 16 };
        crate::dd_log!("  -> DD_OK (stored: {}x{}@{}bpp)", g.width, g.height, g.bpp);
    });
    Ok(())
}

fn create_clipper() -> Result<IDirectDrawClipper> {
    Ok(ClipperImpl { hwnd: 0 }.into())
}

fn create_palette(dwflags: u32, lpddcolorarray: *mut PALETTEENTRY) -> Result<IDirectDrawPalette> {
    let mut entries = [[0u8; 4]; 256];
    if !lpddcolorarray.is_null() {
        unsafe {
            for i in 0..256 {
                let pe = *lpddcolorarray.add(i);
                entries[i] = [pe.peBlue, pe.peGreen, pe.peRed, pe.peFlags];
            }
        }
    }
    Ok(PaletteImpl { flags: dwflags, entries: entries.into() }.into())
}

#[implement(IDirectDraw7, IDirectDraw4, IDirectDraw2, IDirectDraw)]
pub struct DirectDrawImpl {}

// === IDirectDraw (v1) ===
impl IDirectDraw_Impl for DirectDrawImpl_Impl {
    fn Compact(&self) -> Result<()> { Ok(()) }
    fn CreateClipper(&self, _dwflags: u32, lplpddclipper: OutRef<'_, IDirectDrawClipper>, _punkouter: Ref<'_, IUnknown>) -> Result<()> {
        let _ = lplpddclipper.write(Some(create_clipper()?)); Ok(())
    }
    fn CreatePalette(&self, dwflags: u32, lpddcolorarray: *mut PALETTEENTRY, lplpddpalette: OutRef<'_, IDirectDrawPalette>, _punkouter: Ref<'_, IUnknown>) -> Result<()> {
        let _ = lplpddpalette.write(Some(create_palette(dwflags, lpddcolorarray)?)); Ok(())
    }
    fn CreateSurface(&self, lpddsd: *mut DDSURFACEDESC, lplpddsurface: OutRef<'_, IDirectDrawSurface>, _punkouter: Ref<'_, IUnknown>) -> Result<()> {
        crate::dd_log!("IDirectDraw::CreateSurface(lpDDSD={:p})", lpddsd);
        if lpddsd.is_null() { return Err(E_INVALIDARG.into()); }
        let desc = unsafe { &*lpddsd };
        crate::dd_log!("  desc: {}x{}, flags={:#x}, caps={:#x}, backbuffer={}", desc.dwWidth, desc.dwHeight, desc.dwFlags, desc.ddsCaps.dwCaps, desc.dwBackBufferCount);
        let bpp = if desc.dwFlags & DDSD_PIXELFORMAT as u32 != 0 {
            unsafe { desc.ddpfPixelFormat.Anonymous1.dwRGBBitCount }
        } else { 0 };
        let bbc = if desc.dwFlags & DDSD_BACKBUFFERCOUNT as u32 != 0 { desc.dwBackBufferCount } else { 0 };
        let result = create_surface_from_desc(desc.ddsCaps.dwCaps, desc.dwWidth, desc.dwHeight, bpp, bbc);
        match &result {
            Ok(_) => crate::dd_log!("  -> DD_OK"),
            Err(e) => crate::dd_log!("  -> Error: {:?}", e),
        }
        let _ = lplpddsurface.write(Some(result?));
        Ok(())
    }
    fn DuplicateSurface(&self, _a: Ref<'_, IDirectDrawSurface>) -> Result<IDirectDrawSurface> { Err(Error::from(HRESULT(DXERR_GENERIC as i32))) }
    fn EnumDisplayModes(&self, _dwflags: u32, lpddsd: *mut DDSURFACEDESC, lpcontext: *mut core::ffi::c_void, callback: LPDDENUMMODESCALLBACK) -> Result<()> {
        enum_display_modes(lpddsd, lpcontext, callback)
    }
    fn EnumSurfaces(&self, _a: u32, _b: *mut DDSURFACEDESC, _c: *mut core::ffi::c_void, _d: LPDDENUMSURFACESCALLBACK) -> Result<()> { Ok(()) }
    fn FlipToGDISurface(&self) -> Result<()> { Ok(()) }
    fn GetCaps(&self, driver: *mut DDCAPS_DX7, emul: *mut DDCAPS_DX7) -> Result<()> {
        if !driver.is_null() { unsafe { fill_caps(&mut *driver) }; }
        if !emul.is_null() { unsafe {
            (*emul).dwSize = std::mem::size_of::<DDCAPS_DX7>() as u32;
            (*emul).dwCaps = DDCAPS_BLTSTRETCH as u32;
        }}
        Ok(())
    }
    fn GetDisplayMode(&self, desc: *mut DDSURFACEDESC) -> Result<()> {
        if desc.is_null() { return Err(E_INVALIDARG.into()); }
        unsafe { fill_desc_v1(&mut *desc) }; Ok(())
    }
    fn GetFourCCCodes(&self, _a: *mut u32, _b: *mut u32) -> Result<()> { Ok(()) }
    fn GetGDISurface(&self) -> Result<IDirectDrawSurface> { Err(Error::from(HRESULT(DXERR_GENERIC as i32))) }
    fn GetMonitorFrequency(&self, f: *mut u32) -> Result<()> { if !f.is_null() { unsafe { *f = 60 }; } Ok(()) }
    fn GetScanLine(&self, _a: *mut u32) -> Result<()> { Err(Error::from(HRESULT(DXERR_UNSUPPORTED as i32))) }
    fn GetVerticalBlankStatus(&self, _a: *mut BOOL) -> Result<()> { Ok(()) }
    fn Initialize(&self, _a: *mut GUID) -> Result<()> { Ok(()) }
    fn RestoreDisplayMode(&self) -> Result<()> { Ok(()) }
    fn SetCooperativeLevel(&self, hwnd: HWND, flags: u32) -> Result<()> {
        crate::dd_log!("IDirectDraw::SetCooperativeLevel(hwnd={:p}, flags={:#x})", hwnd.0, flags);
        with_global(|g| { g.hwnd = hwnd.0 as isize; g.cooperative_level = flags; });
        crate::dd_log!("  -> DD_OK");
        Ok(())
    }
    fn SetDisplayMode(&self, w: u32, h: u32, bpp: u32) -> Result<()> {
        crate::dd_log!("IDirectDraw::SetDisplayMode({}, {}, {})", w, h, bpp);
        set_display_mode(w, h, bpp)
    }
    fn WaitForVerticalBlank(&self, _a: u32, _b: HANDLE) -> Result<()> { Ok(()) }
}

// === IDirectDraw2 ===
impl IDirectDraw2_Impl for DirectDrawImpl_Impl {
    fn Compact(&self) -> Result<()> { Ok(()) }
    fn CreateClipper(&self, a: u32, b: OutRef<'_, IDirectDrawClipper>, c: Ref<'_, IUnknown>) -> Result<()> { IDirectDraw_Impl::CreateClipper(self, a, b, c) }
    fn CreatePalette(&self, a: u32, b: *mut PALETTEENTRY, c: OutRef<'_, IDirectDrawPalette>, d: Ref<'_, IUnknown>) -> Result<()> { IDirectDraw_Impl::CreatePalette(self, a, b, c, d) }
    fn CreateSurface(&self, a: *mut DDSURFACEDESC, b: OutRef<'_, IDirectDrawSurface>, c: Ref<'_, IUnknown>) -> Result<()> { IDirectDraw_Impl::CreateSurface(self, a, b, c) }
    fn DuplicateSurface(&self, a: Ref<'_, IDirectDrawSurface>) -> Result<IDirectDrawSurface> { IDirectDraw_Impl::DuplicateSurface(self, a) }
    fn EnumDisplayModes(&self, a: u32, b: *mut DDSURFACEDESC, c: *mut core::ffi::c_void, d: LPDDENUMMODESCALLBACK) -> Result<()> { IDirectDraw_Impl::EnumDisplayModes(self, a, b, c, d) }
    fn EnumSurfaces(&self, a: u32, b: *mut DDSURFACEDESC, c: *mut core::ffi::c_void, d: LPDDENUMSURFACESCALLBACK) -> Result<()> { IDirectDraw_Impl::EnumSurfaces(self, a, b, c, d) }
    fn FlipToGDISurface(&self) -> Result<()> { Ok(()) }
    fn GetCaps(&self, a: *mut DDCAPS_DX7, b: *mut DDCAPS_DX7) -> Result<()> { IDirectDraw_Impl::GetCaps(self, a, b) }
    fn GetDisplayMode(&self, a: *mut DDSURFACEDESC) -> Result<()> { IDirectDraw_Impl::GetDisplayMode(self, a) }
    fn GetFourCCCodes(&self, a: *mut u32, b: *mut u32) -> Result<()> { IDirectDraw_Impl::GetFourCCCodes(self, a, b) }
    fn GetGDISurface(&self) -> Result<IDirectDrawSurface> { IDirectDraw_Impl::GetGDISurface(self) }
    fn GetMonitorFrequency(&self, a: *mut u32) -> Result<()> { IDirectDraw_Impl::GetMonitorFrequency(self, a) }
    fn GetScanLine(&self, a: *mut u32) -> Result<()> { IDirectDraw_Impl::GetScanLine(self, a) }
    fn GetVerticalBlankStatus(&self, a: *mut BOOL) -> Result<()> { IDirectDraw_Impl::GetVerticalBlankStatus(self, a) }
    fn Initialize(&self, a: *mut GUID) -> Result<()> { IDirectDraw_Impl::Initialize(self, a) }
    fn RestoreDisplayMode(&self) -> Result<()> { Ok(()) }
    fn SetCooperativeLevel(&self, a: HWND, b: u32) -> Result<()> { IDirectDraw_Impl::SetCooperativeLevel(self, a, b) }
    fn SetDisplayMode(&self, w: u32, h: u32, bpp: u32, _r: u32, _f: u32) -> Result<()> { set_display_mode(w, h, bpp) }
    fn WaitForVerticalBlank(&self, a: u32, b: HANDLE) -> Result<()> { IDirectDraw_Impl::WaitForVerticalBlank(self, a, b) }
    fn GetAvailableVidMem(&self, _a: *mut DDSCAPS, _b: *mut u32, _c: *mut u32) -> Result<()> { Ok(()) }
}

// === IDirectDraw4 ===
impl IDirectDraw4_Impl for DirectDrawImpl_Impl {
    fn Compact(&self) -> Result<()> { Ok(()) }
    fn CreateClipper(&self, a: u32, b: OutRef<'_, IDirectDrawClipper>, c: Ref<'_, IUnknown>) -> Result<()> { IDirectDraw_Impl::CreateClipper(self, a, b, c) }
    fn CreatePalette(&self, a: u32, b: *mut PALETTEENTRY, c: OutRef<'_, IDirectDrawPalette>, d: Ref<'_, IUnknown>) -> Result<()> { IDirectDraw_Impl::CreatePalette(self, a, b, c, d) }
    fn CreateSurface(&self, lpddsd: *mut DDSURFACEDESC2, lplpddsurface: OutRef<'_, IDirectDrawSurface4>, _punkouter: Ref<'_, IUnknown>) -> Result<()> {
        if lpddsd.is_null() { return Err(E_INVALIDARG.into()); }
        let desc = unsafe { &*lpddsd };
        let bpp = if desc.dwFlags & DDSD_PIXELFORMAT as u32 != 0 {
            unsafe { desc.Anonymous5.ddpfPixelFormat.Anonymous1.dwRGBBitCount }
        } else { 0 };
        let bbc = if desc.dwFlags & DDSD_BACKBUFFERCOUNT as u32 != 0 {
            unsafe { desc.Anonymous2.dwBackBufferCount }
        } else { 0 };
        let is_primary = (desc.ddsCaps.dwCaps & DDSCAPS_PRIMARYSURFACE as u32) != 0;
        let (w, h, b) = if is_primary {
            with_global(|g| (g.width, g.height, g.bpp))
        } else {
            let bb = if bpp > 0 { bpp } else { with_global(|g| g.bpp) };
            (desc.dwWidth, desc.dwHeight, bb)
        };
        let mut surface = SurfaceImpl::new(w, h, b, is_primary);
        if is_primary && (desc.ddsCaps.dwCaps & DDSCAPS_FLIP as u32) != 0 {
            surface.caps |= DDSCAPS_FRONTBUFFER as u32;
            if bbc > 0 { surface.backbuffer_pixels = Some(std::cell::RefCell::new(vec![0; (surface.pitch * surface.height) as usize])); }
        }
        let surface_v1: IDirectDrawSurface = surface.into();
        let surface_v4: IDirectDrawSurface4 = surface_v1.cast()?;
        let _ = lplpddsurface.write(Some(surface_v4));
        Ok(())
    }
    fn DuplicateSurface(&self, _a: Ref<'_, IDirectDrawSurface4>) -> Result<IDirectDrawSurface4> { Err(Error::from(HRESULT(DXERR_GENERIC as i32))) }
    fn EnumDisplayModes(&self, _a: u32, _b: *mut DDSURFACEDESC2, _c: *mut core::ffi::c_void, _d: LPDDENUMMODESCALLBACK2) -> Result<()> { Ok(()) }
    fn EnumSurfaces(&self, _a: u32, _b: *mut DDSURFACEDESC2, _c: *mut core::ffi::c_void, _d: Option<unsafe extern "system" fn(Ref<'_, IDirectDrawSurface4>, *mut DDSURFACEDESC2, *mut core::ffi::c_void) -> HRESULT>) -> Result<()> { Ok(()) }
    fn FlipToGDISurface(&self) -> Result<()> { Ok(()) }
    fn GetCaps(&self, a: *mut DDCAPS_DX7, b: *mut DDCAPS_DX7) -> Result<()> { IDirectDraw_Impl::GetCaps(self, a, b) }
    fn GetDisplayMode(&self, desc: *mut DDSURFACEDESC2) -> Result<()> {
        if desc.is_null() { return Err(E_INVALIDARG.into()); }
        unsafe { fill_desc_v2(&mut *desc) }; Ok(())
    }
    fn GetFourCCCodes(&self, a: *mut u32, b: *mut u32) -> Result<()> { IDirectDraw_Impl::GetFourCCCodes(self, a, b) }
    fn GetGDISurface(&self) -> Result<IDirectDrawSurface4> { Err(Error::from(HRESULT(DXERR_GENERIC as i32))) }
    fn GetMonitorFrequency(&self, a: *mut u32) -> Result<()> { IDirectDraw_Impl::GetMonitorFrequency(self, a) }
    fn GetScanLine(&self, a: *mut u32) -> Result<()> { IDirectDraw_Impl::GetScanLine(self, a) }
    fn GetVerticalBlankStatus(&self, a: *mut BOOL) -> Result<()> { IDirectDraw_Impl::GetVerticalBlankStatus(self, a) }
    fn Initialize(&self, a: *mut GUID) -> Result<()> { IDirectDraw_Impl::Initialize(self, a) }
    fn RestoreDisplayMode(&self) -> Result<()> { Ok(()) }
    fn SetCooperativeLevel(&self, a: HWND, b: u32) -> Result<()> { IDirectDraw_Impl::SetCooperativeLevel(self, a, b) }
    fn SetDisplayMode(&self, w: u32, h: u32, bpp: u32, _r: u32, _f: u32) -> Result<()> { set_display_mode(w, h, bpp) }
    fn WaitForVerticalBlank(&self, a: u32, b: HANDLE) -> Result<()> { IDirectDraw_Impl::WaitForVerticalBlank(self, a, b) }
    fn GetAvailableVidMem(&self, _a: *mut DDSCAPS2, _b: *mut u32, _c: *mut u32) -> Result<()> { Ok(()) }
    fn GetSurfaceFromDC(&self, _hdc: HDC) -> Result<IDirectDrawSurface4> { Err(Error::from(HRESULT(DXERR_GENERIC as i32))) }
    fn RestoreAllSurfaces(&self) -> Result<()> { Ok(()) }
    fn TestCooperativeLevel(&self) -> Result<()> { Ok(()) }
    fn GetDeviceIdentifier(&self, _a: *mut DDDEVICEIDENTIFIER, _b: u32) -> Result<()> { Ok(()) }
}

// === IDirectDraw7 ===
impl IDirectDraw7_Impl for DirectDrawImpl_Impl {
    fn Compact(&self) -> Result<()> { Ok(()) }
    fn CreateClipper(&self, a: u32, b: OutRef<'_, IDirectDrawClipper>, c: Ref<'_, IUnknown>) -> Result<()> { IDirectDraw_Impl::CreateClipper(self, a, b, c) }
    fn CreatePalette(&self, a: u32, b: *mut PALETTEENTRY, c: OutRef<'_, IDirectDrawPalette>, d: Ref<'_, IUnknown>) -> Result<()> { IDirectDraw_Impl::CreatePalette(self, a, b, c, d) }
    fn CreateSurface(&self, lpddsd: *mut DDSURFACEDESC2, lplpddsurface: OutRef<'_, IDirectDrawSurface7>, _punkouter: Ref<'_, IUnknown>) -> Result<()> {
        crate::dd_log!("IDirectDraw7::CreateSurface(lpDDSD={:p})", lpddsd);
        if lpddsd.is_null() { return Err(E_INVALIDARG.into()); }
        let desc = unsafe { &*lpddsd };
        crate::dd_log!("  desc: {}x{}, flags={:#x}, caps={:#x}", desc.dwWidth, desc.dwHeight, desc.dwFlags, desc.ddsCaps.dwCaps);
        let bpp = if desc.dwFlags & DDSD_PIXELFORMAT as u32 != 0 {
            unsafe { desc.Anonymous5.ddpfPixelFormat.Anonymous1.dwRGBBitCount }
        } else { 0 };
        let bbc = if desc.dwFlags & DDSD_BACKBUFFERCOUNT as u32 != 0 {
            unsafe { desc.Anonymous2.dwBackBufferCount }
        } else { 0 };
        crate::dd_log!("  bpp={}, backbuffer_count={}", bpp, bbc);
        let is_primary = (desc.ddsCaps.dwCaps & DDSCAPS_PRIMARYSURFACE as u32) != 0;
        let (w, h, b) = if is_primary {
            with_global(|g| (g.width, g.height, g.bpp))
        } else {
            let bb = if bpp > 0 { bpp } else { with_global(|g| g.bpp) };
            (desc.dwWidth, desc.dwHeight, bb)
        };
        crate::dd_log!("  creating surface: {}x{}@{}bpp, is_primary={}", w, h, b, is_primary);
        let mut surface = SurfaceImpl::new(w, h, b, is_primary);
        if is_primary && (desc.ddsCaps.dwCaps & DDSCAPS_FLIP as u32) != 0 {
            surface.caps |= DDSCAPS_FRONTBUFFER as u32;
            if bbc > 0 { surface.backbuffer_pixels = Some(std::cell::RefCell::new(vec![0; (surface.pitch * surface.height) as usize])); }
        }
        let surface_v1: IDirectDrawSurface = surface.into();
        let surface_v7: IDirectDrawSurface7 = surface_v1.cast()?;
        let _ = lplpddsurface.write(Some(surface_v7));
        Ok(())
    }
    fn DuplicateSurface(&self, _a: Ref<'_, IDirectDrawSurface7>) -> Result<IDirectDrawSurface7> { Err(Error::from(HRESULT(DXERR_GENERIC as i32))) }
    fn EnumDisplayModes(&self, _a: u32, _b: *mut DDSURFACEDESC2, _c: *mut core::ffi::c_void, _d: LPDDENUMMODESCALLBACK2) -> Result<()> { Ok(()) }
    fn EnumSurfaces(&self, _a: u32, _b: *mut DDSURFACEDESC2, _c: *mut core::ffi::c_void, _d: Option<unsafe extern "system" fn(Ref<'_, IDirectDrawSurface7>, *mut DDSURFACEDESC2, *mut core::ffi::c_void) -> HRESULT>) -> Result<()> { Ok(()) }
    fn FlipToGDISurface(&self) -> Result<()> { Ok(()) }
    fn GetCaps(&self, a: *mut DDCAPS_DX7, b: *mut DDCAPS_DX7) -> Result<()> { IDirectDraw_Impl::GetCaps(self, a, b) }
    fn GetDisplayMode(&self, desc: *mut DDSURFACEDESC2) -> Result<()> {
        if desc.is_null() { return Err(E_INVALIDARG.into()); }
        unsafe { fill_desc_v2(&mut *desc) }; Ok(())
    }
    fn GetFourCCCodes(&self, a: *mut u32, b: *mut u32) -> Result<()> { IDirectDraw_Impl::GetFourCCCodes(self, a, b) }
    fn GetGDISurface(&self) -> Result<IDirectDrawSurface7> { Err(Error::from(HRESULT(DXERR_GENERIC as i32))) }
    fn GetMonitorFrequency(&self, a: *mut u32) -> Result<()> { IDirectDraw_Impl::GetMonitorFrequency(self, a) }
    fn GetScanLine(&self, a: *mut u32) -> Result<()> { IDirectDraw_Impl::GetScanLine(self, a) }
    fn GetVerticalBlankStatus(&self, a: *mut BOOL) -> Result<()> { IDirectDraw_Impl::GetVerticalBlankStatus(self, a) }
    fn Initialize(&self, a: *mut GUID) -> Result<()> { IDirectDraw_Impl::Initialize(self, a) }
    fn RestoreDisplayMode(&self) -> Result<()> { Ok(()) }
    fn SetCooperativeLevel(&self, a: HWND, b: u32) -> Result<()> { IDirectDraw_Impl::SetCooperativeLevel(self, a, b) }
    fn SetDisplayMode(&self, w: u32, h: u32, bpp: u32, refresh: u32, flags: u32) -> Result<()> {
        crate::dd_log!("IDirectDraw7::SetDisplayMode({}, {}, {}, refresh={}, flags={:#x})", w, h, bpp, refresh, flags);
        set_display_mode(w, h, bpp)
    }
    fn WaitForVerticalBlank(&self, a: u32, b: HANDLE) -> Result<()> { IDirectDraw_Impl::WaitForVerticalBlank(self, a, b) }
    fn GetAvailableVidMem(&self, _a: *mut DDSCAPS2, _b: *mut u32, _c: *mut u32) -> Result<()> { Ok(()) }
    fn GetSurfaceFromDC(&self, _hdc: HDC) -> Result<IDirectDrawSurface7> { Err(Error::from(HRESULT(DXERR_GENERIC as i32))) }
    fn RestoreAllSurfaces(&self) -> Result<()> { Ok(()) }
    fn TestCooperativeLevel(&self) -> Result<()> { Ok(()) }
    fn GetDeviceIdentifier(&self, _a: *mut DDDEVICEIDENTIFIER2, _b: u32) -> Result<()> { Ok(()) }
    fn StartModeTest(&self, _a: *mut SIZE, _b: u32, _c: u32) -> Result<()> { Ok(()) }
    fn EvaluateMode(&self, _a: u32, _b: *mut u32) -> Result<()> { Ok(()) }
}
