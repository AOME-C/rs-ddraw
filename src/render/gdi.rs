//! GDI (software) presentation backend (ports the GDI branch of `render.c`).

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

use crate::state::{SurfaceBuffers, state};

/// Present the primary surface to the window using GDI Bit/StretchBlt.
pub(crate) fn present(buffers: &SurfaceBuffers) {
    let (hwnd, dst_hdc, vp, fps, draw_fps) = {
        let st = state().lock().unwrap();
        (st.hwnd, st.hdc, st.render.viewport, st.fps, st.draw_fps)
    };
    if hwnd.is_invalid() || dst_hdc.is_invalid() {
        return;
    }
    let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe {
        GetClientRect(hwnd, &mut rc);
        let w = rc.right - rc.left;
        let h = rc.bottom - rc.top;
        if w <= 0 || h <= 0 {
            return;
        }

        // Destination rectangle: the configured viewport, or the full client
        // area when no viewport has been computed yet.
        let (dx, dy, dw, dh) = if vp.right > vp.left && vp.bottom > vp.top {
            (vp.left, vp.top, vp.right - vp.left, vp.bottom - vp.top)
        } else {
            (0, 0, w, h)
        };

        // Only clear the letterbox margins (outside the viewport) to black.
        // Clearing the whole client area every present causes a black flash
        // (flicker) because FillRect and StretchBlt are separate GDI ops and
        // the compositor can show the black fill between them.
        let full = dx == 0 && dy == 0 && dw == w && dh == h;
        if !full {
            let brush = HBRUSH(GetStockObject(BLACK_BRUSH).0);
            if dy > 0 {
                let _ = FillRect(dst_hdc, &RECT { left: 0, top: 0, right: w, bottom: dy }, brush);
            }
            if dy + dh < h {
                let _ = FillRect(dst_hdc, &RECT { left: 0, top: dy + dh, right: w, bottom: h }, brush);
            }
            if dx > 0 {
                let _ = FillRect(dst_hdc, &RECT { left: 0, top: dy, right: dx, bottom: dy + dh }, brush);
            }
            if dx + dw < w {
                let _ = FillRect(dst_hdc, &RECT { left: dx + dw, top: dy, right: w, bottom: dy + dh }, brush);
            }
        }

        // Read the surface under its lock so we never StretchBlt while the game
        // is mid-Blt into the same DIB (avoids torn frames). Keep the lock
        // scoped to just the copy; composite_child_windows below must stay
        // outside it (it sends messages to the game thread and would deadlock).
        let _guard = buffers.lock.lock();
        let _ = StretchBlt(dst_hdc, dx, dy, dw, dh, Some(buffers.hdc), 0, 0, buffers.width, buffers.height, SRCCOPY);

        if draw_fps {
            crate::render::draw_fps(dst_hdc, fps);
        }

        crate::render::composite_child_windows(hwnd, buffers.hdc);
    }
}
