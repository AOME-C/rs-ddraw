/// IAT (Import Address Table) hooking framework.
///
/// Intercepts Win32 API calls made by the game to redirect
/// cursor, window, and rendering operations through cnc-ddraw.

/// Initialize all API hooks.
pub(crate) fn init() {
    // TODO: patch IAT of game executable and loaded modules
    // Hook: GetCursorPos, ClipCursor, SetWindowPos, BitBlt, StretchBlt, etc.
}

/// Revert all API hooks.
pub(crate) fn exit() {
    // TODO: restore original IAT entries
}
