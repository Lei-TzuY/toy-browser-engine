//! browser_engine — A minimal browser engine built from scratch in Rust.
//!
//! Pipeline:
//!  HTML string
//!   → `html::parse_html`      → DOM tree  (`dom::Node`)
//!   → `style::style_tree`     → Styled tree (`style::StyledNode`)
//!   → `layout::layout_tree`   → Layout tree (`layout::LayoutBox`)
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
#[allow(dead_code)]
#[path = "image.rs"]
mod image_base;
#[allow(dead_code)]
#[path = "image_final.rs"]
mod image_prev;
#[allow(dead_code)]
#[path = "image_ascii_final.rs"]
mod image_prev2;
#[allow(dead_code)]
#[path = "image_pgm_final.rs"]
mod image_prev3;
#[allow(dead_code)]
#[path = "image_pbm_final.rs"]
mod image_prev4;
#[allow(dead_code)]
#[path = "image_pam_final.rs"]
mod image_prev5;
#[allow(dead_code)]
#[path = "image_pfm_final.rs"]
mod image_prev6;
#[allow(dead_code)]
#[path = "image_bmp_final.rs"]
mod image_prev7;
#[path = "image_bmp_rle_final.rs"]
pub mod image;
pub mod input;
pub mod layout;
#[allow(dead_code)]
#[path = "net/mod.rs"]
mod net_base;
#[path = "net_final.rs"]
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
#[allow(dead_code)]
#[path = "validation_facade.rs"]
mod validation_facade;
#[path = "validation_final.rs"]
pub mod validation;

pub use browser::Browser;
pub use document::{Document, PointerState};
pub use html::extract_inline_styles;
pub use net::{MemoryLoader, ResourceLoader, Url};
