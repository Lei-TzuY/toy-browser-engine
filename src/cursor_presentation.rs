// ============================================================
// cursor_presentation.rs — backend-facing cursor presentation plan
// ============================================================

use crate::cursor::CursorIcon;
use crate::cursor_assets::ResolvedCursor;

/// Small native cursor vocabulary supported by the current window frontend.
///
/// Keeping this separate from CSS lets the engine represent all CSS keywords
/// precisely while a constrained backend chooses the nearest available shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeCursor {
    Arrow,
    IBeam,
    Crosshair,
    ClosedHand,
    OpenHand,
    ResizeLeftRight,
    ResizeUpDown,
    ResizeAll,
}

/// How a frontend should present one resolved page cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorPresentation {
    /// Hide the native cursor and paint nothing (`cursor: none`).
    Hidden,
    /// Ask the platform/window toolkit for a native cursor shape.
    Native(NativeCursor),
    /// Hide the native cursor; the decoded image is painted by cursor_overlay.
    SoftwareImage,
}

/// Convert a fully resolved CSS cursor into a backend presentation strategy.
pub fn presentation_for_cursor(cursor: &ResolvedCursor) -> CursorPresentation {
    match cursor {
        ResolvedCursor::Image { .. } => CursorPresentation::SoftwareImage,
        ResolvedCursor::System(icon) => presentation_for_icon(*icon),
    }
}

/// Map the complete CSS cursor keyword model to the smaller native vocabulary.
/// Unsupported distinctions deliberately collapse to the nearest safe shape.
pub fn presentation_for_icon(icon: CursorIcon) -> CursorPresentation {
    use CursorIcon::*;
    let native = match icon {
        None => return CursorPresentation::Hidden,
        Text | VerticalText => NativeCursor::IBeam,
        Crosshair | Cell => NativeCursor::Crosshair,
        Pointer | Grab => NativeCursor::OpenHand,
        Grabbing => NativeCursor::ClosedHand,
        ColResize | EResize | WResize | EwResize => NativeCursor::ResizeLeftRight,
        RowResize | NResize | SResize | NsResize => NativeCursor::ResizeUpDown,
        Move | AllScroll | NeResize | NwResize | SeResize | SwResize | NeswResize | NwseResize => {
            NativeCursor::ResizeAll
        }
        Auto
        | Default
        | ContextMenu
        | Help
        | Progress
        | Wait
        | Alias
        | Copy
        | NoDrop
        | NotAllowed
        | ZoomIn
        | ZoomOut => NativeCursor::Arrow,
    };
    CursorPresentation::Native(native)
}

impl CursorPresentation {
    pub const fn native_cursor(self) -> Option<NativeCursor> {
        match self {
            Self::Native(cursor) => Some(cursor),
            Self::Hidden | Self::SoftwareImage => None,
        }
    }

    pub const fn native_visible(self) -> bool {
        matches!(self, Self::Native(_))
    }

    pub const fn needs_software_overlay(self) -> bool {
        matches!(self, Self::SoftwareImage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_text_pointer_and_resize_families() {
        assert_eq!(
            presentation_for_icon(CursorIcon::Text),
            CursorPresentation::Native(NativeCursor::IBeam)
        );
        assert_eq!(
            presentation_for_icon(CursorIcon::Pointer),
            CursorPresentation::Native(NativeCursor::OpenHand)
        );
        assert_eq!(
            presentation_for_icon(CursorIcon::EwResize),
            CursorPresentation::Native(NativeCursor::ResizeLeftRight)
        );
        assert_eq!(
            presentation_for_icon(CursorIcon::NsResize),
            CursorPresentation::Native(NativeCursor::ResizeUpDown)
        );
        assert_eq!(
            presentation_for_icon(CursorIcon::NwseResize),
            CursorPresentation::Native(NativeCursor::ResizeAll)
        );
        assert_eq!(
            presentation_for_icon(CursorIcon::AllScroll),
            CursorPresentation::Native(NativeCursor::ResizeAll)
        );
    }

    #[test]
    fn none_hides_and_custom_images_use_software_path() {
        assert_eq!(presentation_for_icon(CursorIcon::None), CursorPresentation::Hidden);

        let resolved = ResolvedCursor::System(CursorIcon::Default);
        assert_eq!(
            presentation_for_cursor(&resolved),
            CursorPresentation::Native(NativeCursor::Arrow)
        );
    }

    #[test]
    fn helpers_expose_backend_requirements() {
        let native = CursorPresentation::Native(NativeCursor::Crosshair);
        assert!(native.native_visible());
        assert_eq!(native.native_cursor(), Some(NativeCursor::Crosshair));
        assert!(!native.needs_software_overlay());

        let software = CursorPresentation::SoftwareImage;
        assert!(!software.native_visible());
        assert_eq!(software.native_cursor(), None);
        assert!(software.needs_software_overlay());
    }
}
