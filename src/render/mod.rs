//! Renderer thread and backend orchestration (ports `render.c`).

pub(crate) mod d3d9;
pub(crate) mod gdi;
pub(crate) mod opengl;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    BitBlt, GetDC, HDC, ReleaseDC, SRCCOPY, SetBkMode, SetTextColor, TextOutA, TRANSPARENT,
};
use windows::Win32::UI::WindowsAndMessaging::{EnumChildWindows, GetClientRect, GetWindowRect};
use windows::core::BOOL;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

static PRESENT_LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static NO_PRIMARY_LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

use crate::counter::{counter_get, counter_start};
use crate::state::{
    state, RENDERER_D3D9, RENDERER_GDI, RENDERER_OPENGL, SurfaceBuffers,
};

/// Human-readable name for a `RENDERER_*` id (used in logs).
fn renderer_name(r: i32) -> &'static str {
    match r {
        RENDERER_D3D9 => "d3d9",
        RENDERER_OPENGL => "opengl",
        RENDERER_GDI => "gdi",
        _ => "unknown",
    }
}

/// Draw the current FPS counter on the destination DC (used when `DrawFPS` is on).
pub(crate) unsafe fn draw_fps(hdc: HDC, fps: f64) {
    let text = format!("{:.0} FPS", fps);
    let _ = SetBkMode(hdc, TRANSPARENT);
    let _ = SetTextColor(hdc, COLORREF(0x00FFFF00));
    let _ = TextOutA(hdc, 5, 5, text.as_bytes());
}

/// State passed to the child-window enumeration callback.
struct ChildComposite {
    surface_hdc: HDC,
    win_left: i32,
    win_top: i32,
}

/// Paint the region of the primary surface that lies under each game child
/// window into that child window, so child windows (e.g. in-game menus)
/// aren't left blank. Ports ts-ddraw's `EnumChildProc` / `EnumChildWindows`.
extern "system" fn enum_child_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    unsafe {
        let data = &*(lparam.0 as *const ChildComposite);
        let child_dc = GetDC(Some(hwnd));
        let mut size = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        GetClientRect(hwnd, &mut size);
        let mut pos = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        GetWindowRect(hwnd, &mut pos);
        let w = size.right - size.left;
        let h = size.bottom - size.top;
        if w > 0 && h > 0 {
            let _ = BitBlt(
                child_dc,
                0,
                0,
                w,
                h,
                Some(data.surface_hdc),
                pos.left - data.win_left,
                pos.top - data.win_top,
                SRCCOPY,
            );
        }
        let _ = ReleaseDC(Some(hwnd), child_dc);
        BOOL(1)
    }
}

/// Composite the surface into any child windows of the game window.
pub(crate) unsafe fn composite_child_windows(main_hwnd: HWND, surface_hdc: HDC) {
    if main_hwnd.is_invalid() || surface_hdc.is_invalid() {
        return;
    }
    let mut wr = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    GetWindowRect(main_hwnd, &mut wr);
    let data = ChildComposite {
        surface_hdc,
        win_left: wr.left,
        win_top: wr.top,
    };
    EnumChildWindows(
        Some(main_hwnd),
        Some(enum_child_proc),
        LPARAM(&data as *const ChildComposite as isize),
    );
}

/// Spawn the renderer thread exactly once.
pub(crate) fn start() {
    let already = {
        let st = state().lock().unwrap();
        st.render_thread.is_some()
    };
    if already {
        return;
    }
    let _handle = thread::spawn(render_thread);
    let mut st = state().lock().unwrap();
    st.render_thread = None;
}

fn render_thread() {
    // Wait for the primary surface to be created.
    loop {
        {
            let st = state().lock().unwrap();
            if st.primary.is_some() {
                break;
            }
            if !st.running.load(Ordering::Relaxed) {
                return;
            }
        }
        thread::sleep(Duration::from_millis(10));
    }

    enum Backend {
        D3D9(d3d9::D3D9State),
        GL(opengl::OglState),
        GDI,
    }

    let (hwnd, hdc, w, h, renderer, auto) = {
        let st = state().lock().unwrap();
        (
            st.hwnd,
            st.hdc,
            st.width,
            st.height,
            st.renderer,
            st.auto_renderer,
        )
    };

    // Auto order: D3D9 -> OpenGL -> GDI. An explicit choice falls back to GDI if
    // its backend fails to initialize.
    let order: [i32; 4] = if auto {
        [RENDERER_D3D9, RENDERER_OPENGL, RENDERER_GDI, RENDERER_GDI]
    } else {
        [renderer, RENDERER_GDI, RENDERER_GDI, RENDERER_GDI]
    };

    let mut backend = {
        let mut chosen = RENDERER_GDI;
        let mut result: Option<Backend> = None;
        for &r in &order {
            let b = match r {
                RENDERER_D3D9 => d3d9::D3D9State::new(hwnd, w, h).map(Backend::D3D9),
                RENDERER_OPENGL => opengl::OglState::new(hdc, w, h).map(Backend::GL),
                RENDERER_GDI => Some(Backend::GDI),
                _ => None,
            };
            match b {
                Some(bk) => {
                    chosen = r;
                    result = Some(bk);
                    break;
                }
                None => {
                    crate::dd_log!(
                        "renderer {} unavailable, switching to {}",
                        renderer_name(r),
                        renderer_name(if r == RENDERER_GDI {
                            RENDERER_GDI
                        } else {
                            order[order.iter().position(|&x| x == r).unwrap() + 1]
                        })
                    );
                }
            }
        }
        if let Some(bk) = result {
            if chosen != RENDERER_GDI {
                if auto {
                    crate::dd_log!("auto: selected renderer {}", renderer_name(chosen));
                } else {
                    crate::dd_log!("renderer {} initialized", renderer_name(chosen));
                }
            }
            state().lock().unwrap().renderer = chosen;
            bk
        } else {
            Backend::GDI
        }
    };

    crate::dd_log!(
        "render thread started, backend={}, target_fps={}",
        match backend {
            Backend::D3D9(_) => "d3d9",
            Backend::GL(_) => "opengl",
            Backend::GDI => "gdi",
        },
        { state().lock().unwrap().target_fps }
    );

    let mut start = counter_start();
    let mut last_w: i32 = -1;
    let mut last_h: i32 = -1;

    loop {
        if !state().lock().unwrap().running.load(Ordering::Relaxed) {
            break;
        }

        let primary: Option<std::sync::Arc<SurfaceBuffers>> = {
            state().lock().unwrap().primary.clone()
        };
        // Present only when the game has produced a *complete* frame. cnc-ddraw
        // uploads on `surface_updated` (raised on Blt/Flip/Unlock/ReleaseDC of
        // the primary), i.e. *after* the draw finishes — never on
        // WaitForVerticalBlank, which many games call *before* drawing. Uploading
        // on vblank would capture a half-drawn (black) primary. Each backend
        // draws its last good frame when no new one is signalled; GDI only
        // repaints the window when a new frame is signalled.
        let mut dirty = crate::state::take_dirty();
        // Force an upload when the primary surface size changes (e.g. the game
        // recreates it after the loading screen). Without this the texture
        // would be the wrong size and the screen would freeze on the last good
        // frame even if the game keeps rendering.
        if let Some(ref buffers) = primary {
            if buffers.width != last_w || buffers.height != last_h {
                dirty = true;
                last_w = buffers.width;
                last_h = buffers.height;
            }
        }
        if let Some(ref buffers) = primary {
            if !PRESENT_LOGGED.swap(true, Ordering::Relaxed) {
                crate::dd_log!(
                    "first present: {}x{} bpp={}",
                    buffers.width,
                    buffers.height,
                    buffers.bpp
                );
            }
            match &mut backend {
                Backend::D3D9(d) => d.present(buffers, dirty),
                Backend::GL(g) => g.present(buffers, dirty),
                Backend::GDI => {
                    if dirty {
                        gdi::present(buffers);
                    }
                }
            }
        } else if !NO_PRIMARY_LOGGED.swap(true, Ordering::Relaxed) {
            crate::dd_log!("render loop tick: primary surface still not set");
        }

        let elapsed = counter_get(start);
        if elapsed > 0.0 {
            let fps = 1000.0 / elapsed;
            let mut st = state().lock().unwrap();
            // Exponential smoothing for a stable readout.
            st.fps = if st.fps == 0.0 { fps } else { st.fps * 0.9 + fps * 0.1 };
        }

        let target_fps = { state().lock().unwrap().target_fps };
        // When TargetFPS=0, cap at ~60fps. Sleep (no busy-spin) so we don't peg
        // a CPU core, which would starve the game's audio/mix thread.
        let frame_len = if target_fps > 0.0 {
            1000.0 / target_fps
        } else {
            1000.0 / 60.0
        };
        let elapsed = counter_get(start);
        if elapsed < frame_len {
            let sleep_ms = (frame_len - elapsed) as u64;
            if sleep_ms > 0 {
                thread::sleep(Duration::from_millis(sleep_ms));
            }
        }

        start = counter_start();
    }

    match backend {
        Backend::D3D9(d) => d.release(),
        Backend::GL(g) => g.release(),
        Backend::GDI => {}
    }
}
