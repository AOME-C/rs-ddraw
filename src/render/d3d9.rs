use super::Renderer;

/// Direct3D 9 renderer.
pub(crate) struct D3d9Renderer {
    hwnd: isize,
    width: u32,
    height: u32,
}

impl D3d9Renderer {
    pub fn new() -> Self {
        Self {
            hwnd: 0,
            width: 0,
            height: 0,
        }
    }
}

impl Renderer for D3d9Renderer {
    fn create(&mut self, hwnd: isize, width: u32, height: u32) -> bool {
        self.hwnd = hwnd;
        self.width = width;
        self.height = height;
        // TODO: Direct3DCreate9, create device, create surfaces/textures
        true
    }

    fn render(&mut self, _pixels: &[u8], _width: u32, _height: u32, _bpp: u32) {
        // TODO: lock texture, copy pixels, draw fullscreen quad, Present
    }

    fn release(&mut self) {
        // TODO: release D3D9 resources
    }
}
