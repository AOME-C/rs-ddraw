use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::LibraryLoader::DisableThreadLibraryCalls;
use crate::dd_log;

const DLL_PROCESS_ATTACH: u32 = 1;
const DLL_PROCESS_DETACH: u32 = 0;

#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub unsafe extern "system" fn DllMain(h_module: HMODULE, dw_reason: u32, _lp_reserved: *const u8) -> i32 {
    match dw_reason {
        DLL_PROCESS_ATTACH => {
            crate::log::init();
            dd_log!("DllMain(DLL_PROCESS_ATTACH) hModule={:p}", h_module.0);
            unsafe { let _ = DisableThreadLibraryCalls(h_module); }
        }
        DLL_PROCESS_DETACH => {
            dd_log!("DllMain(DLL_PROCESS_DETACH)");
        }
        _ => {}
    }
    1
}
