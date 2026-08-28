use super::Renderer;

/// GDI-based renderer (software fallback).
pub(crate) struct GdiRenderer {
    hwnd: isize,
    hdc: isize,
    width: u32,
    height: u32,
}

impl GdiRenderer {
    pub fn new() -> Self {
        Self {
            hwnd: 0,
            hdc: 0,
            width: 0,
            height: 0,
        }
    }
}

impl Renderer for GdiRenderer {
    fn create(&mut self, hwnd: isize, width: u32, height: u32) -> bool {
        self.hwnd = hwnd;
        self.width = width;
        self.height = height;
        // TODO: GetDC, create compatible bitmap
        true
    }

    fn render(&mut self, _pixels: &[u8], _width: u32, _height: u32, _bpp: u32) {
        // TODO: StretchBlt / SetDIBitsToDevice
    }

    fn release(&mut self) {
        // TODO: ReleaseDC, delete objects
        self.hdc = 0;
    }
}
