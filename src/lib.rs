//! browser_engine — A minimal browser engine built from scratch in Rust.
//!
//! Pipeline:
//!  HTML string
//!   → `html::parse_html`      → DOM tree  (`dom::Node`)
//!   → `style::style_tree`     → Styled tree (`style::StyledNode`)
//!   → `layout::layout_tree`   → Layout tree (`layout::LayoutBox`)
//!   → `paint::paint`          → Pixel canvas (`paint::Canvas`) → PPM file

pub mod animation;
pub mod audio;
#[allow(dead_code)]
#[path = "browser.rs"]
mod browser_prev;
#[path = "browser_cookie_session_final.rs"]
pub mod browser;
pub mod canvas;
pub mod cookie;
pub mod cookie_network;
pub mod cookie_same_site;
pub mod css;
pub mod document;
pub mod dom;
pub mod editing;
pub mod eventloop;
pub mod form_state;
pub mod forms;
pub mod hsts;
pub mod hsts_network;
pub mod html;
pub mod image;
pub mod input;
pub mod layout;
pub mod navigation_network;
pub mod net;
pub mod paint;
pub mod redirect_policy;
pub mod referrer_policy;
pub mod script;
pub mod select_state;
pub mod session_network;
pub mod style;
pub mod svg;
pub mod text;
pub mod transition;
pub mod validation;

pub use animation::AnimationManager;
pub use browser::Browser;
pub use cookie_network::{CookieJarRef, CookieNetwork};
pub use cookie_same_site::{cookie_allows_request, same_site_allows, SameSiteRequestContext};
pub use document::{Document, PointerState};
pub use hsts::{HstsCache, HstsPolicy};
pub use hsts_network::{HstsCacheRef, HstsNetwork};
pub use html::extract_inline_styles;
pub use navigation_network::NavigationNetwork;
pub use net::{MemoryLoader, ResourceLoader, Url};
pub use redirect_policy::{RedirectError, RedirectPlanner, FETCH_MAX_REDIRECTS};
pub use referrer_policy::ReferrerPolicy;
pub use session_network::SessionNetwork;
pub use transition::TransitionManager;
