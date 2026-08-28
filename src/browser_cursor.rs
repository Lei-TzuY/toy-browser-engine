// Browser-level convenience methods for the CSS cursor runtime.

use crate::browser::Browser;
use crate::cursor::{cursor_at_point, cursor_hit_test, CursorIcon};
use crate::document::PointerState;
use crate::script::NodePath;

impl Browser {
    /// Resolve the cursor requested by the page at a page-coordinate point.
    ///
    /// Keeping this on `Browser` gives window frontends a stable one-call API
    /// and keeps style-tree/document internals out of platform adapters.
    pub fn cursor_at_point(
        &self,
        x: f32,
        y: f32,
        viewport_width: f32,
        pointer: &PointerState,
    ) -> CursorIcon {
        cursor_at_point(self.document(), x, y, viewport_width, pointer)
    }

    /// Hit-test and resolve the cursor in one pass, returning the DOM path that
    /// should become the new hovered target alongside the requested cursor.
    pub fn cursor_hit_test(
        &self,
        x: f32,
        y: f32,
        viewport_width: f32,
        pointer: &PointerState,
    ) -> (Option<NodePath>, CursorIcon) {
        cursor_hit_test(self.document(), x, y, viewport_width, pointer)
    }
}
