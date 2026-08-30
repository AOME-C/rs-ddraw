//! Runtime configuration loaded from `ddraw.ini`.
//!
//! Ports `Settings.c` from ts-ddraw.

use std::ffi::CString;
use std::path::Path;

use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GetModuleFileNameA, GetModuleHandleExA,
};
use windows::Win32::System::Threading::{GetProcessAffinityMask, SetProcessAffinityMask};
use windows::Win32::System::WindowsProgramming::{GetPrivateProfileIntA, GetPrivateProfileStringA};
use windows::core::PCSTR;

use crate::state::{DMDFO_CENTER, DMDFO_DEFAULT, DMDFO_STRETCH, RENDERER_D3D9, RENDERER_GDI, RENDERER_OPENGL, state};

const SETTINGS_SECTION: &str = "ddraw";

fn section() -> CString {
    CString::new(SETTINGS_SECTION).unwrap()
}

/// 返回实际要读取的配置文件路径：优先 `rs-ddraw.ini`，不存在时回退到
/// `ddraw.ini`，两者都不存在则返回不存在的文件名（Windows INI API 会直接
/// 返回各项的默认值）。
///
/// 配置在 DLL 所在目录查找（与日志文件路径一致），因为游戏进程的 CWD
/// 往往不是游戏目录。
fn path() -> CString {
    let candidates = ["rs-ddraw.ini", "ddraw.ini"];
    if let Some(dir) = module_dir() {
        for name in candidates {
            let full = format!("{}\\{}", dir.trim_end_matches('\\'), name);
            if Path::new(&full).exists() {
                return CString::new(full).unwrap();
            }
        }
    }
    for name in candidates {
        if Path::new(name).exists() {
            return CString::new(name).unwrap();
        }
    }
    CString::new("rs-ddraw.ini").unwrap()
}

/// 取得本 DLL 所在目录（通过模块地址反查模块句柄），用于定位配置文件。
fn module_dir() -> Option<String> {
    unsafe {
        let mut h = HMODULE::default();
        // 使用本函数地址反查自身模块句柄。
        let addr = module_dir as *const u8;
        if GetModuleHandleExA(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, PCSTR(addr), &mut h).is_ok() {
            let mut buf = [0u8; 1024];
            let n = GetModuleFileNameA(Some(h), &mut buf);
            if n > 0 {
                let path = String::from_utf8_lossy(&buf[..n as usize]);
                return std::path::Path::new(path.as_ref()).parent().map(|p| p.to_string_lossy().to_string());
            }
        }
    }
    None
}

fn pcstr(c: &CString) -> PCSTR {
    PCSTR(c.as_c_str().as_ptr() as *const u8)
}

fn get_string(key: &str, default: &str) -> String {
    let key = CString::new(key).unwrap();
    let def = CString::new(default).unwrap();
    let mut buf = vec![0u8; 256];
    let n = unsafe {
        GetPrivateProfileStringA(pcstr(&section()), pcstr(&key), pcstr(&def), Some(buf.as_mut_slice()), pcstr(&path()))
    };
    if n == 0 {
        return default.to_string();
    }
    let end = (n as usize).min(buf.len());
    let len = buf[..end].iter().position(|&c| c == 0).unwrap_or(end);
    String::from_utf8_lossy(&buf[..len]).to_string()
}

fn get_bool(key: &str, default: bool) -> bool {
    let v = get_string(key, if default { "Yes" } else { "No" });
    let v = v.trim();
    v.eq_ignore_ascii_case("yes") || v.eq_ignore_ascii_case("true") || v == "1"
}

fn get_int(key: &str, default: i32) -> i32 {
    let key = CString::new(key).unwrap();
    unsafe { GetPrivateProfileIntA(pcstr(&section()), pcstr(&key), default, pcstr(&path())) as i32 }
}

fn eq_ignore(value: &str, target: &str) -> bool {
    value.eq_ignore_ascii_case(target)
}

/// Load configuration from `ddraw.ini` into the global state.
pub unsafe fn load() {
    let mut s = state().lock().unwrap();

    s.maintain_aspect_ratio = get_bool("MaintainAspectRatio", s.maintain_aspect_ratio);
    s.windowboxing = get_bool("Windowboxing", s.windowboxing);
    s.stretch_to_fullscreen = get_bool("StretchToFullscreen", s.stretch_to_fullscreen);
    s.stretch_to_width = get_int("StretchToWidth", s.stretch_to_width);
    s.stretch_to_height = get_int("StretchToHeight", s.stretch_to_height);
    s.draw_fps = get_int("DrawFPS", s.draw_fps as i32) != 0;

    let (r, auto) = get_renderer("Renderer", "auto");
    s.renderer = r;
    s.auto_renderer = auto;

    s.primary_surface2tex = get_bool("PrimarySurface2Tex", s.primary_surface2tex);
    s.gl_finish = get_bool("GlFinish", s.gl_finish);
    s.convert_on_gpu = get_bool("ConvertOnGPU", true);

    let tfps = get_int("TargetFPS", 0);
    if tfps > 0 {
        s.target_fps = tfps as f64;
        s.target_frame_len = 1000.0 / s.target_fps;
    } else {
        s.target_fps = 0.0;
        s.target_frame_len = 16.0;
    }

    if get_bool("VSync", false) {
        s.swap_interval = 1;
    } else {
        s.swap_interval = 0;
    }

    apply_affinity(&mut s, get_bool("SingleProcAffinity", true));

    s.edge_timeout_ms = get_int("MonitorEdgeTimer", s.edge_timeout_ms);

    crate::state::RGB555.store(get_bool("rgb555", false), std::sync::atomic::Ordering::Relaxed);

    s.gl_fence_sync = get_bool("GlFenceSync", s.gl_fence_sync);

    s.fixed_output = get_fixed_output("FixedOutput", "stretch");

    // Environment variable overrides (mirrors ts-ddraw's DDRAW_* vars).
    if let Ok(v) = std::env::var("DDRAW_DRAW_FPS")
        && (v.trim().eq_ignore_ascii_case("yes") || v.trim().eq_ignore_ascii_case("true") || v.trim() == "1") {
            s.draw_fps = true;
        }
    if let Ok(v) = std::env::var("DDRAW_TARGET_FPS")
        && let Ok(n) = v.trim().parse::<i32>()
            && n > 0 {
                s.target_fps = n as f64;
                s.target_frame_len = 1000.0 / s.target_fps;
            }
}

fn apply_affinity(s: &mut crate::state::DDrawState, single: bool) {
    let proc = unsafe { windows::Win32::System::Threading::GetCurrentProcess() };
    let mut sys_aff: usize = 0;
    let mut proc_aff: usize = 0;
    unsafe {
        if GetProcessAffinityMask(proc, &mut proc_aff, &mut sys_aff).is_ok() {
            s.proc_affinity = proc_aff;
            s.system_affinity = sys_aff;
        }
    }
    if single {
        unsafe {
            let _ = SetProcessAffinityMask(proc, 1);
        }
        s.proc_affinity = 0;
        s.system_affinity = 0;
    }
}

fn get_renderer(key: &str, default: &str) -> (i32, bool) {
    let value = get_string(key, default);
    let value = value.trim();
    if eq_ignore(value, "opengl") {
        (RENDERER_OPENGL, false)
    } else if eq_ignore(value, "gdi") {
        (RENDERER_GDI, false)
    } else if eq_ignore(value, "d3d9") {
        (RENDERER_D3D9, false)
    } else if eq_ignore(value, "auto") {
        match default {
            "d3d9" => (RENDERER_D3D9, true),
            "opengl" => (RENDERER_OPENGL, true),
            _ => (RENDERER_D3D9, true),
        }
    } else {
        (RENDERER_D3D9, false)
    }
}

fn get_fixed_output(key: &str, default: &str) -> u32 {
    let value = get_string(key, default);
    let value = value.trim();
    if eq_ignore(value, "default") {
        DMDFO_DEFAULT
    } else if eq_ignore(value, "center") {
        DMDFO_CENTER
    } else if eq_ignore(value, "stretch") {
        DMDFO_STRETCH
    } else {
        DMDFO_DEFAULT
    }
}
