//! browser_engine — A minimal browser engine built from scratch in Rust.
//!
//! Pipeline:
//!  HTML string
//!   → `html::parse_html`      → DOM tree  (`dom::Node`)
//!   → `style::style_tree`     → Styled tree  (`style::StyledNode`)
//!   → `layout::layout_tree`   → Layout tree  (`layout::LayoutBox`)
//!   → `paint::paint`          → Pixel canvas (`paint::Canvas`) → PPM file

pub mod browser;
pub mod css;
pub mod document;
pub mod dom;
pub mod editing;
pub mod eventloop;
pub mod form_state;
pub mod forms;
pub mod html;
pub mod image;
pub mod input;
pub mod layout;
pub mod net;
pub mod paint;
pub mod script;
pub mod select_state;
pub mod style;
pub mod text;
#[allow(dead_code)]
#[path = "validation.rs"]
mod validation_base;
#[allow(dead_code)]
#[path = "validation_ext.rs"]
mod validation_constraints;
#[path = "validation_facade.rs"]
pub mod validation;

pub use browser::Browser;
pub use document::{Document, PointerState};
pub use html::extract_inline_styles;
pub use net::{MemoryLoader, ResourceLoader, Url};
