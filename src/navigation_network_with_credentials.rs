// Keep the established navigation/session implementation intact while layering
// credential-mode aware subresource fetch policy in the same Rust module.  The
// `include!` split mirrors the cookie module's core/extension organization and
// lets the additive implementation share NavigationNetwork's private session
// state without widening that state to the whole crate.
include!("navigation_network.rs");
include!("navigation_network_credentials_ext.rs");
