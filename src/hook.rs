//! IAT (Import Address Table) hooking framework (ports `hook.c`).
//!
//! Intercepts a few Win32 calls the game makes so window positioning and the
//! cursor behave correctly under our wrapper.

use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, PAGE_READWRITE, VirtualProtect, VirtualQuery};
use windows::Win32::System::SystemServices::{
    IMAGE_DOS_HEADER, IMAGE_DOS_SIGNATURE, IMAGE_IMPORT_DESCRIPTOR, IMAGE_NT_SIGNATURE,
};
use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, SWP_NOSIZE};
use windows::core::BOOL;

use crate::state::state;
use crate::window;

type SetWindowPosFn = unsafe extern "system" fn(HWND, HWND, i32, i32, i32, i32, u32) -> i32;
type GetCursorPosFn = unsafe extern "system" fn(*mut POINT) -> i32;
type MoveWindowFn = unsafe extern "system" fn(HWND, i32, i32, i32, i32, BOOL) -> i32;

/// Cast a `FARPROC` returned by `GetProcAddress` into a typed function pointer.
unsafe fn to_fn<T>(p: FARPROC) -> Option<T> {
    p.map(|f| std::mem::transmute_copy(&f))
}

// Real (un-hooked) function pointers, resolved once at init via GetProcAddress
// so the fakes can call through to the original implementation without
// recursing into our own patched IAT entry.
static mut REAL_SETPOS: Option<SetWindowPosFn> = None;
static mut REAL_GETCURSORPOS: Option<GetCursorPosFn> = None;
static mut REAL_MOVEWINDOW: Option<MoveWindowFn> = None;

fn ieq(name: *const u8, target: &[u8]) -> bool {
    if name.is_null() {
        return false;
    }
    let mut i = 0;
    unsafe {
        loop {
            let c = *name.add(i);
            let t = *target.get(i).unwrap_or(&0);
            if t == 0 {
                return c == 0;
            }
            if c == 0 {
                return false;
            }
            if c.to_ascii_lowercase() != t.to_ascii_lowercase() {
                return false;
            }
            i += 1;
        }
    }
}

/// Patch `functionName` imported from `module_name` in module `hmod`.
unsafe fn hook_iat(hmod: HMODULE, module_name: &[u8], function_name: &[u8], new_function: usize) {
    if hmod.0.is_null() || new_function == 0 {
        return;
    }
    let base = hmod.0 as *mut u8;
    let dos = base as *const IMAGE_DOS_HEADER;
    if (*dos).e_magic != IMAGE_DOS_SIGNATURE {
        return;
    }
    let nt = base.add((*dos).e_lfanew as usize) as *const u32;
    if *nt != IMAGE_NT_SIGNATURE {
        return;
    }

    // Optional header follows the 4-byte NT signature and 20-byte file header.
    let opt = base.add((*dos).e_lfanew as usize + 4 + 20);
    let magic = *(opt as *const u16);
    let datadir_off = if magic == 0x20B { 0x70usize } else { 0x60usize };
    let datadir_base = opt.add(datadir_off);
    let import_entry = datadir_base.add(8) as *const u32;
    let import_rva = *import_entry as usize;
    if import_rva == 0 {
        return;
    }

    let mut desc = base.add(import_rva) as *const IMAGE_IMPORT_DESCRIPTOR;
    loop {
        let first_thunk_rva = (*desc).FirstThunk;
        if first_thunk_rva == 0 {
            break;
        }
        let name_rva = (*desc).Name;
        let imp_name = base.add(name_rva as usize) as *const u8;
        if ieq(imp_name, module_name) {
            let orig_rva = (*desc).Anonymous.OriginalFirstThunk;
            let orig_base =
                if orig_rva != 0 { base.add(orig_rva as usize) } else { base.add(first_thunk_rva as usize) };
            let mut first = base.add(first_thunk_rva as usize) as *mut usize;
            let mut orig = orig_base as *const usize;
            let top_bit = 1usize << (usize::BITS - 1);
            loop {
                let fn_addr = *first;
                let ord_addr = *orig;
                if fn_addr == 0 {
                    break;
                }
                if ord_addr & top_bit == 0 {
                    let name_ptr = base.add(ord_addr & !top_bit) as *const u8;
                    // IMAGE_IMPORT_BY_NAME: 2-byte hint followed by name.
                    let import_fn_name = name_ptr.add(2);
                    if ieq(import_fn_name, function_name) {
                        let mut mbi = MEMORY_BASIC_INFORMATION::default();
                        if VirtualQuery(
                            Some(first as *const core::ffi::c_void),
                            &mut mbi,
                            std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
                        ) != 0
                        {
                            let mut old = PAGE_READWRITE;
                            if VirtualProtect(
                                first as *const core::ffi::c_void,
                                std::mem::size_of::<usize>(),
                                PAGE_READWRITE,
                                &mut old,
                            )
                            .is_ok()
                            {
                                *first = new_function;
                                let _ = VirtualProtect(
                                    first as *const core::ffi::c_void,
                                    std::mem::size_of::<usize>(),
                                    old,
                                    &mut old,
                                );
                            }
                        }
                        break;
                    }
                }
                first = first.add(1);
                orig = orig.add(1);
            }
        }
        desc = desc.add(1);
    }
}

/// Replacement for `user32!SetWindowPos` — forwards to the real call and, when
/// the game resizes our own window, recomputes the render viewport.
unsafe extern "system" fn fake_set_window_pos(
    hwnd: HWND,
    hwnd_insert_after: HWND,
    x: i32,
    y: i32,
    cx: i32,
    cy: i32,
    flags: u32,
) -> i32 {
    if let Some(f) = REAL_SETPOS {
        f(hwnd, hwnd_insert_after, x, y, cx, cy, flags);
    }
    if (flags & SWP_NOSIZE.0 as u32) == 0 {
        let (our, gw, gh) = {
            let st = state().lock().unwrap();
            (st.hwnd, st.width, st.height)
        };
        if hwnd == our {
            window::recompute_viewport(gw, gh);
        }
    }
    1
}

/// Replacement for `user32!MoveWindow` — forwards to the real call.
unsafe extern "system" fn fake_move_window(hwnd: HWND, x: i32, y: i32, cx: i32, cy: i32, repaint: BOOL) -> i32 {
    if let Some(f) = REAL_MOVEWINDOW {
        f(hwnd, x, y, cx, cy, repaint);
    }
    1
}

/// Replacement for `user32!GetCursorPos` — forwards to the real call and clamps
/// the reported position to our window while the cursor is locked.
unsafe extern "system" fn fake_get_cursor_pos(point: *mut POINT) -> i32 {
    if let Some(f) = REAL_GETCURSORPOS {
        f(point);
    }
    if !point.is_null() {
        let (locked, hwnd) = {
            let st = state().lock().unwrap();
            (st.mouse_is_locked != 0, st.hwnd)
        };
        if locked && !hwnd.is_invalid() {
            let mut wr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            GetWindowRect(hwnd, &mut wr);
            if (*point).x < wr.left {
                (*point).x = wr.left;
            }
            if (*point).x > wr.right {
                (*point).x = wr.right;
            }
            if (*point).y < wr.top {
                (*point).y = wr.top;
            }
            if (*point).y > wr.bottom {
                (*point).y = wr.bottom;
            }
        }
    }
    1
}

/// Initialize all API hooks. Safe to call multiple times.
pub(crate) fn init() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        if let Ok(hmod) = GetModuleHandleA(None) {
            let user32 = GetModuleHandleA(windows::core::PCSTR(b"user32.dll\0".as_ptr()));
            if let Ok(user32) = user32 {
                REAL_SETPOS = to_fn(GetProcAddress(user32, windows::core::PCSTR(b"SetWindowPos\0".as_ptr())));
                REAL_GETCURSORPOS = to_fn(GetProcAddress(user32, windows::core::PCSTR(b"GetCursorPos\0".as_ptr())));
                REAL_MOVEWINDOW = to_fn(GetProcAddress(user32, windows::core::PCSTR(b"MoveWindow\0".as_ptr())));
            }
            hook_iat(hmod, b"user32.dll\0", b"SetWindowPos\0", fake_set_window_pos as usize);
            hook_iat(hmod, b"user32.dll\0", b"MoveWindow\0", fake_move_window as usize);
            hook_iat(hmod, b"user32.dll\0", b"GetCursorPos\0", fake_get_cursor_pos as usize);
        }
    });
}
