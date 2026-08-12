//! browser_engine — A minimal browser engine built from scratch in Rust.
//!
//! Pipeline:
//!  HTML string
//!   → `html::parse_html`      → DOM tree  (`dom::Node`)
//!   → `style::style_tree`     → Styled tree  (`style::StyledNode`)
//!   → `layout::layout_tree`   → Layout tree  (`layout::LayoutBox`)
//!   → `paint::paint`          → Pixel canvas (`paint::Canvas`) → PPM file

pub mod dom;
pub mod html;
pub mod css;
pub mod style;
pub mod layout;
pub mod paint;
pub mod text;
pub mod script;

pub use html::extract_inline_styles;
