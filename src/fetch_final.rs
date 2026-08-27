// ============================================================
//  fetch_final.rs — public fetch facade
// ============================================================
//
// Keep the request/response vocabulary and deterministic test backends from
// the raw implementation, but make the public threaded/routing backends use
// the completion-tracked wrappers from `net_final` too. This prevents callers
// of `net::fetch::ThreadedNetwork` from bypassing the lifecycle guarantee that
// `net::ThreadedNetwork` provides.

pub use crate::net_base::fetch::{
    reason_phrase, FetchCompletion, FetchError, FetchId, FetchRegistry, FetchRequest,
    FetchResponse, HeaderError, HeaderMap, LocalNetwork, ManualNetwork, Method, NetworkBackend,
    OfflineNetwork, Origin, MAX_IN_FLIGHT_FETCHES,
};

pub use super::{DefaultNetwork, ThreadedNetwork};
