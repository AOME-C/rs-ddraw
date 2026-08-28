use super::Renderer;

/// OpenGL-based renderer with shader support.
pub(crate) struct OglRenderer {
    hwnd: isize,
    hdc: isize,
    context: isize,
    width: u32,
    height: u32,
}

impl OglRenderer {
    pub fn new() -> Self {
        Self {
            hwnd: 0,
            hdc: 0,
            context: 0,
            width: 0,
            height: 0,
        }
    }
}

impl Renderer for OglRenderer {
    fn create(&mut self, hwnd: isize, width: u32, height: u32) -> bool {
        self.hwnd = hwnd;
        self.width = width;
        self.height = height;
        // TODO: wglCreateContext, set pixel format, load GL functions
        true
    }

    fn render(&mut self, _pixels: &[u8], _width: u32, _height: u32, _bpp: u32) {
        // TODO: upload texture, draw quad with shader, SwapBuffers
    }

    fn release(&mut self) {
        // TODO: wglDeleteContext, release resources
        self.context = 0;
        self.hdc = 0;
    }
}
