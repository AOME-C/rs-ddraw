//! Window procedure and window management helpers.
//!
//! Ports the window handling portions of `IDirectDraw.c` / `main.c`.

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::DirectDraw::DDSCL_FULLSCREEN;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

use crate::state::{state, RENDERER_D3D9, RENDERER_GDI, RENDERER_OPENGL, TIMER_FIX_WINDOWPOS};

// Virtual key constants (numeric to avoid enum casts).
const VK_TAB: i32 = 0x09;
const VK_END: i32 = 0x23;
const VK_PRIOR: i32 = 0x21;
const VK_NEXT: i32 = 0x22;
const VK_CONTROL: i32 = 0x11;
const VK_RMENU: i32 = 0xA5;
const VK_RCONTROL: i32 = 0xA3;
const SC_CLOSE: u32 = 0xF060;

/// Replace the window's WndProc with our own and return the previous one.
pub(crate) unsafe fn subclass(hwnd: HWND) -> usize {
    let new_proc = wnd_proc as usize as isize;
    let prev = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, new_proc as _);
    prev as usize
}

/// Replacement window procedure. Forwards unhandled messages to the original.
unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_SIZE => {
            let mut st = state().lock().unwrap();
            let mut wr = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            GetWindowRect(hwnd, &mut wr);
            st.win_rect = wr;
            st.render.invalidate = true;
        }
        WM_MOVE => {
            let mut st = state().lock().unwrap();
            let mut wr = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            GetWindowRect(hwnd, &mut wr);
            st.win_rect = wr;
        }
        WM_ACTIVATEAPP => {
            let mut st = state().lock().unwrap();
            st.focus_gained = if wparam.0 != 0 { 1 } else { 0 };
        }
        WM_ACTIVATE => {
            let active = (wparam.0 & 0xffff) != 0;
            let mut st = state().lock().unwrap();
            st.focus_gained = if active { 1 } else { 0 };
            let dw_flags = st.dw_flags;
            drop(st);
            if active {
                mouse_lock();
            } else {
                if (dw_flags & DDSCL_FULLSCREEN as u32) != 0 {
                    let _ = ShowWindow(hwnd, SW_MINIMIZE);
                }
                mouse_unlock(false);
            }
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN => {
            mouse_lock();
        }
        675 => {
            // WM_MOUSELEAVE
            mouse_unlock(false);
        }
        WM_CLOSE => {
            mouse_unlock(false);
        }
        WM_PARENTNOTIFY => {
            // TS menu redraw workaround: force a repaint of the client area.
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
                WM_SYSCOMMAND => {
            if (wparam.0 & 0xfff0) == SC_CLOSE as usize {
                std::process::exit(0);
            }
        }
        WM_ERASEBKGND => {
            // The renderer thread paints the whole client area; let the OS
            // erase to the window's background brush would flash black between
            // presents (flicker). Claim we handled it so nothing is erased.
            return LRESULT(1);
        }
        WM_PAINT => {
            // Validate the update region without repainting: our renderer
            // thread owns the pixels. Prevents the OS from painting/erasing.
            let _ = ValidateRect(Some(hwnd), None);
            return LRESULT(0);
        }
        WM_KEYDOWN => {
            handle_hotkeys(hwnd, wparam.0 as i32);
        }
        WM_TIMER => {
            if wparam.0 as usize == TIMER_FIX_WINDOWPOS {
                let (gw, gh) = {
                    let st = state().lock().unwrap();
                    (st.width, st.height)
                };
                set_window_size(gw, gh);
            }
        }
        _ => {}
    }

    let prev = { let st = state().lock().unwrap(); st.wnd_proc as usize };
    if prev != 0 {
        let prev_proc: WNDPROC = std::mem::transmute::<usize, WNDPROC>(prev);
        CallWindowProcW(prev_proc, hwnd, msg, wparam, lparam)
    } else {
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}

/// Keyboard hotkeys (mirrors ts-ddraw's WndProc WM_KEYDOWN).
unsafe fn handle_hotkeys(_hwnd: HWND, key: i32) {
    let ctrl = GetAsyncKeyState(VK_CONTROL) < 0;
    let rmenu = GetAsyncKeyState(VK_RMENU) < 0;
    let rctrl = GetAsyncKeyState(VK_RCONTROL) < 0;

    let mut st = state().lock().unwrap();
    if ctrl {
        match key {
            VK_TAB => {
                drop(st);
                mouse_unlock(true);
                return;
            }
            VK_END => {
                if st.auto_renderer {
                    let r = st.renderer;
                    drop(st);
                    // Cycle renderer: GDI -> OpenGL -> D3D9 -> GDI.
                    let next = if r == RENDERER_GDI {
                        RENDERER_OPENGL
                    } else if r == RENDERER_OPENGL {
                        RENDERER_D3D9
                    } else {
                        RENDERER_GDI
                    };
                    crate::state::set_renderer(next);
                    // Force the running renderer to re-init.
                    let mut s = state().lock().unwrap();
                    s.render.invalidate = true;
                    return;
                }
            }
            VK_PRIOR => {
                // PageUp: increase target FPS by 5.
                let tfps = (st.target_fps + 5.0).min(1000.0);
                st.target_fps = tfps;
                st.target_frame_len = 1000.0 / tfps;
            }
            VK_NEXT => {
                // PageDown: decrease target FPS by 5 (min 1).
                let tfps = (st.target_fps - 5.0).max(1.0);
                st.target_fps = tfps;
                st.target_frame_len = 1000.0 / tfps;
            }
            _ => {}
        }
    } else if rmenu && rctrl {
        drop(st);
        if state().lock().unwrap().mouse_is_locked != 0 {
            mouse_unlock(true);
        } else {
            mouse_lock();
        }
        return;
    } else if rctrl {
        if key == 'R' as i32 {
            st.draw_fps = !st.draw_fps;
        }
    }
}

/// Compute the render viewport (MaintainAspectRatio / Windowboxing /
/// StretchToFullscreen) from the game resolution. Updates `state.render`.
fn compute_viewport(game_w: i32, game_h: i32) {
    let (cfg_maintain, cfg_windowbox, cfg_stretch_fs, stretch_w, stretch_h, screen_w, screen_h) = {
        let st = state().lock().unwrap();
        (
            st.maintain_aspect_ratio,
            st.windowboxing,
            st.stretch_to_fullscreen,
            st.stretch_to_width,
            st.stretch_to_height,
            st.screen_width,
            st.screen_height,
        )
    };

    let gw = game_w.max(1);
    let gh = game_h.max(1);

    let mut render_w = stretch_w;
    let mut render_h = stretch_h;
    if cfg_stretch_fs {
        render_w = screen_w;
        render_h = screen_h;
    }
    if render_w < gw {
        render_w = gw;
    }
    if render_h < gh {
        render_h = gh;
    }

    let mut vp_left = 0;
    let mut vp_top = 0;
    let mut vp_w = render_w;
    let mut vp_h = render_h;

    if cfg_windowbox {
        vp_w = gw;
        vp_h = gh;
        for i in (2..=20).rev() {
            if gw * i <= render_w && gh * i <= render_h {
                vp_w *= i;
                vp_h *= i;
                break;
            }
        }
        vp_top = render_h / 2 - vp_h / 2;
        vp_left = render_w / 2 - vp_w / 2;
    } else if cfg_maintain {
        vp_w = render_w;
        vp_h = ((gh as f32 / gw as f32) * vp_w as f32) as i32;
        if vp_h > render_h {
            vp_w = ((vp_w as f32 / vp_h as f32) * render_h as f32) as i32;
            vp_h = render_h;
        }
        vp_top = render_h / 2 - vp_h / 2;
        vp_left = render_w / 2 - vp_w / 2;
    }

    let mut st = state().lock().unwrap();
    st.render.width = render_w;
    st.render.height = render_h;
    st.render.viewport.left = vp_left;
    st.render.viewport.top = vp_top;
    st.render.viewport.right = vp_left + vp_w;
    st.render.viewport.bottom = vp_top + vp_h;
    st.render.scale_w = vp_w as f32 / gw as f32;
    st.render.scale_h = vp_h as f32 / gh as f32;
    st.render.stretched = vp_w != gw || vp_h != gh || vp_left != 0 || vp_top != 0;
    st.render.invalidate = true;
}

/// Recompute the render viewport only (used when the game resizes our window).
pub(crate) unsafe fn recompute_viewport(game_w: i32, game_h: i32) {
    compute_viewport(game_w, game_h);
}

/// Resize and reposition the game window, computing the render viewport
/// (MaintainAspectRatio / Windowboxing / StretchToFullscreen).
///
/// `game_w`/`game_h` are the game's requested resolution (surface size).
pub(crate) unsafe fn set_window_size(game_w: i32, game_h: i32) {
    crate::dd_log!("set_window_size: begin compute_viewport");
    compute_viewport(game_w, game_h);
    crate::dd_log!("set_window_size: compute_viewport done");

    let (hwnd, fullscreen, screen_w, screen_h, render_w, render_h) = {
        let st = state().lock().unwrap();
        (
            st.hwnd,
            (st.dw_flags & DDSCL_FULLSCREEN as u32) != 0,
            st.screen_width,
            st.screen_height,
            st.render.width,
            st.render.height,
        )
    };
    crate::dd_log!("set_window_size: locked ({})", if fullscreen { "fullscreen" } else { "windowed" });
    if !hwnd.is_invalid() {
        let (w, h) = if fullscreen { (screen_w, screen_h) } else { (render_w, render_h) };
        crate::dd_log!("set_window_size: SetWindowPos begin {}x{}", w, h);
        let _ = SetWindowPos(hwnd, Some(HWND_TOP), 0, 0, w, h, SWP_SHOWWINDOW);
        crate::dd_log!("set_window_size: SetWindowPos done");
    }
}

/// Change the display resolution (fullscreen mode).
///
/// ts-ddraw forwards `SetDisplayMode` to the real DirectDraw and does not call
/// `ChangeDisplaySettings` itself; the wrapper window *is* the display. Calling
/// `ChangeDisplaySettingsA` here blocks on the display-driver reset (especially
/// for large resolutions like 2560x1600) and freezes the game, so we only
/// record state and confine the cursor.
pub(crate) unsafe fn set_display_mode(_width: i32, _height: i32, _bpp: i32) -> bool {
    let mut st = state().lock().unwrap();
    st.focus_gained = 1;
    drop(st);
    mouse_lock();
    true
}

/// Confine the cursor to the window's client area.
pub(crate) unsafe fn mouse_lock() {
    crate::dd_log!("mouse_lock: begin");
    let hwnd = { let st = state().lock().unwrap(); st.hwnd };
    if hwnd.is_invalid() {
        return;
    }
    let mut rc = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    GetClientRect(hwnd, &mut rc);
    let mut wr = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    GetWindowRect(hwnd, &mut wr);
    let clip = RECT {
        left: wr.left + rc.left,
        top: wr.top + rc.top,
        right: wr.left + rc.right,
        bottom: wr.top + rc.bottom,
    };
    crate::dd_log!("mouse_lock: ClipCursor begin");
    let _ = ClipCursor(Some(&clip));
    crate::dd_log!("mouse_lock: ClipCursor done");
    let mut st = state().lock().unwrap();
    st.mouse_is_locked = 1;
}

/// Release the cursor confinement and optionally show it.
pub(crate) unsafe fn mouse_unlock(show: bool) {
    let _ = ClipCursor(None);
    if show {
        let _ = ShowCursor(true);
    } else {
        let _ = ShowCursor(false);
    }
    let mut st = state().lock().unwrap();
    st.mouse_is_locked = 0;
}

/// Center the cursor within the window's client area.
pub(crate) unsafe fn center_mouse(hwnd: HWND) {
    let mut rc = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    GetClientRect(hwnd, &mut rc);
    let mut wr = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    GetWindowRect(hwnd, &mut wr);
    let x = wr.left + (rc.left + rc.right) / 2;
    let y = wr.top + (rc.top + rc.bottom) / 2;
    let _ = SetCursorPos(x, y);
}


