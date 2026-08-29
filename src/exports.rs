use std::ffi::c_void;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::DirectDraw::*;

use crate::ddraw::DirectDrawImpl;
use crate::ddraw::clipper::ClipperImpl;
use crate::dd_log;

// --- Core DirectDraw exports ---

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DirectDrawCreate(
    lpguid: *mut GUID,
    lplpdd: *mut Option<IDirectDraw>,
    punkouter: *mut Option<IUnknown>,
) -> HRESULT {
    dd_log!("DirectDrawCreate(lpGUID={:p}, lplpDD={:p}, pUnkOuter={:p})", lpguid, lplpdd, punkouter);
    if lplpdd.is_null() {
        dd_log!("  -> E_INVALIDARG (lplpDD is null)");
        return E_INVALIDARG;
    }
    unsafe {
        crate::config::load();
        crate::hook::init();
        crate::util::init_system();
    }
    let dd: IDirectDraw = DirectDrawImpl {}.into();
    unsafe { *lplpdd = Some(dd) };
    dd_log!("  -> DD_OK (IDirectDraw created)");
    HRESULT(0)
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DirectDrawCreateEx(
    lpguid: *mut GUID,
    lplpdd: *mut Option<IDirectDraw7>,
    iid: *const GUID,
    punkouter: *mut Option<IUnknown>,
) -> HRESULT {
    dd_log!("DirectDrawCreateEx(lpGUID={:p}, lplpDD={:p}, iid={:p}, pUnkOuter={:p})", lpguid, lplpdd, iid, punkouter);
    if lplpdd.is_null() {
        dd_log!("  -> E_INVALIDARG (lplpDD is null)");
        return E_INVALIDARG;
    }
    let dd: IDirectDraw7 = DirectDrawImpl {}.into();
    unsafe {
        crate::config::load();
        crate::hook::init();
        crate::util::init_system();
        *lplpdd = Some(dd);
    }
    dd_log!("  -> DD_OK (IDirectDraw7 created)");
    HRESULT(0)
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DirectDrawCreateClipper(
    dwflags: u32,
    lplpddclipper: *mut Option<IDirectDrawClipper>,
    punkouter: *mut Option<IUnknown>,
) -> HRESULT {
    dd_log!("DirectDrawCreateClipper(dwFlags={:#x}, lplpDDClipper={:p}, pUnkOuter={:p})", dwflags, lplpddclipper, punkouter);
    if lplpddclipper.is_null() {
        dd_log!("  -> E_INVALIDARG");
        return E_INVALIDARG;
    }
    let clipper: IDirectDrawClipper = ClipperImpl { hwnd: 0 }.into();
    unsafe { *lplpddclipper = Some(clipper) };
    dd_log!("  -> DD_OK");
    HRESULT(0)
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DirectDrawEnumerateA(
    _lpcallback: *mut c_void,
    _lpcontext: *mut c_void,
) -> HRESULT {
    dd_log!("DirectDrawEnumerateA -> DD_OK");
    HRESULT(0)
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DirectDrawEnumerateW(
    _lpcallback: *mut c_void,
    _lpcontext: *mut c_void,
) -> HRESULT {
    dd_log!("DirectDrawEnumerateW -> DD_OK");
    HRESULT(0)
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DirectDrawEnumerateExA(
    _lpcallback: *mut c_void,
    _lpcontext: *mut c_void,
    _dwflags: u32,
) -> HRESULT {
    dd_log!("DirectDrawEnumerateExA -> DD_OK");
    HRESULT(0)
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DirectDrawEnumerateExW(
    _lpcallback: *mut c_void,
    _lpcontext: *mut c_void,
    _dwflags: u32,
) -> HRESULT {
    dd_log!("DirectDrawEnumerateExW -> DD_OK");
    HRESULT(0)
}

// --- Stub exports ---

#[unsafe(no_mangle)]
pub extern "system" fn AcquireDDThreadLock() -> u32 { 0 }

#[unsafe(no_mangle)]
pub extern "system" fn ReleaseDDThreadLock() -> u32 { 0 }

#[unsafe(no_mangle)]
pub extern "system" fn CompleteCreateSysmemSurface(_a: u32) -> u32 { 0 }

#[unsafe(no_mangle)]
pub extern "system" fn D3DParseUnknownCommand(
    _lpcmd: *const c_void,
    _lpretcmd: *mut *mut c_void,
) -> HRESULT {
    E_NOTIMPL
}

#[unsafe(no_mangle)]
pub extern "system" fn DDInternalLock(_a: u32, _b: u32) -> u32 { 0 }

#[unsafe(no_mangle)]
pub extern "system" fn DDInternalUnlock(_a: u32) -> u32 { 0 }
