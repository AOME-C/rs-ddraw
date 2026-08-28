pub(crate) mod d3d9;
pub(crate) mod gdi;
pub(crate) mod opengl;

/// Trait for rendering backends.
///
/// Each backend (GDI, OpenGL, D3D9) implements this trait to present
/// the DirectDraw surface pixels to the screen.
pub(crate) trait Renderer {
    /// Initialize the renderer for the given window.
    fn create(&mut self, hwnd: isize, width: u32, height: u32) -> bool;

    /// Render a frame from the given pixel buffer.
    fn render(&mut self, pixels: &[u8], width: u32, height: u32, bpp: u32);

    /// Release all renderer resources.
    fn release(&mut self);
}

/// Available renderer backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RendererKind {
    Gdi,
    OpenGL,
    Direct3D9,
}

/// Auto-select the best available renderer.
pub(crate) fn select_renderer() -> RendererKind {
    // TODO: probe D3D9 availability, then OpenGL, fallback to GDI
    RendererKind::Gdi
}
