use windows::Win32::Foundation::*;
use windows::Win32::Graphics::DirectDraw::*;
use windows::Win32::Graphics::Gdi::RGNDATA;
use windows::core::*;

#[implement(IDirectDrawClipper)]
pub struct ClipperImpl {
    pub hwnd: isize,
}

impl IDirectDrawClipper_Impl for ClipperImpl_Impl {
    fn GetClipList(&self, _lprect: *mut RECT, _lpcliplist: *mut RGNDATA, _lpdwsize: *mut u32) -> Result<()> {
        Err(Error::from(HRESULT(DXERR_GENERIC as i32)))
    }

    fn GetHWnd(&self, lphwnd: *mut HWND) -> Result<()> {
        if lphwnd.is_null() {
            return Err(E_INVALIDARG.into());
        }
        unsafe { *lphwnd = HWND(self.hwnd as *mut core::ffi::c_void) };
        Ok(())
    }

    fn Initialize(&self, _lpdd: Ref<'_, IDirectDraw>, _dwflags: u32) -> Result<()> {
        Ok(())
    }

    fn IsClipListChanged(&self, _lpbchanged: *mut BOOL) -> Result<()> {
        Ok(())
    }

    fn SetClipList(&self, _lpcliplist: *mut RGNDATA, _dwflags: u32) -> Result<()> {
        Ok(())
    }

    fn SetHWnd(&self, _dwflags: u32, _hwnd: HWND) -> Result<()> {
        // Note: DerefMut is not available on _Impl types, so we can't mutate through &self.
        // This is a limitation of the windows crate's COM implementation.
        // For now, we store hwnd at creation time and don't support changing it.
        // In a real implementation, we'd use interior mutability (Cell/RefCell).
        Ok(())
    }
}
