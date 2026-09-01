// ============================================================
//  referrer_policy.rs — Referrer-Policy parsing and computation
// ============================================================

use crate::net::{FetchRequest, FetchResponse, Url};

/// The Referrer Policy values implemented by the browser engine.
///
/// `StrictOriginWhenCrossOrigin` is the modern browser default.  The enum is
/// deliberately network/toolkit neutral so document, Fetch and navigation
/// code can share one policy implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferrerPolicy {
    NoReferrer,
    NoReferrerWhenDowngrade,
    Origin,
    OriginWhenCrossOrigin,
    SameOrigin,
    StrictOrigin,
    StrictOriginWhenCrossOrigin,
    UnsafeUrl,
}

impl Default for ReferrerPolicy {
    fn default() -> Self {
        Self::StrictOriginWhenCrossOrigin
    }
}

/// Referrer state carried across an HTTP redirect chain.
///
/// Fetch conceptually keeps the original referrer source separate from the
/// serialized `Referer` header. That distinction matters when a redirect
/// response changes `Referrer-Policy`: rebuilding the next hop from the
/// previous header can permanently lose path/query information (or, worse,
/// accidentally treat an origin-only serialization as the real source URL).
///
/// This helper owns the stable source URL and the policy that is allowed to
/// change at each redirect response. Call [`prepare_request`] immediately
/// before dispatching each hop and [`observe_redirect_response`] before
/// preparing the following one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedirectReferrerState {
    source: Option<Url>,
    policy: ReferrerPolicy,
}

impl RedirectReferrerState {
    pub fn new(source: Option<Url>, policy: ReferrerPolicy) -> Self {
        Self { source, policy }
    }

    pub fn from_source(source: Url) -> Self {
        Self::new(Some(source), ReferrerPolicy::default())
    }

    pub fn no_referrer() -> Self {
        Self::new(None, ReferrerPolicy::default())
    }

    pub fn source(&self) -> Option<&Url> {
        self.source.as_ref()
    }

    pub fn policy(&self) -> ReferrerPolicy {
        self.policy
    }

    /// Apply a redirect response's Referrer-Policy update in wire order.
    pub fn observe_redirect_response(&mut self, response: &FetchResponse) {
        self.policy = self.policy.updated_on_redirect(response);
    }

    /// Replace any stale/caller-supplied Referer with the value appropriate for
    /// this request's target under the current redirect-chain policy.
    ///
    /// RedirectPlanner also strips the previous hop's Referer. Keeping the
    /// deletion here makes the state object safe when used by another
    /// orchestrator or when the first request already contains an authored
    /// value.
    pub fn prepare_request(&self, request: &mut FetchRequest) {
        request.headers.delete("referer");
        let Some(source) = self.source.as_ref() else {
            return;
        };
        if let Some(value) = self.policy.compute(source, &request.url) {
            request.headers.insert_raw("referer", &value);
        }
    }
}

impl ReferrerPolicy {
    /// Parse one Referrer-Policy token. Unknown and empty values are ignored.
    pub fn parse_token(input: &str) -> Option<Self> {
        match input.trim().to_ascii_lowercase().as_str() {
            "no-referrer" => Some(Self::NoReferrer),
            "no-referrer-when-downgrade" => Some(Self::NoReferrerWhenDowngrade),
            "origin" => Some(Self::Origin),
            "origin-when-cross-origin" => Some(Self::OriginWhenCrossOrigin),
            "same-origin" => Some(Self::SameOrigin),
            "strict-origin" => Some(Self::StrictOrigin),
            "strict-origin-when-cross-origin" => Some(Self::StrictOriginWhenCrossOrigin),
            "unsafe-url" => Some(Self::UnsafeUrl),
            _ => None,
        }
    }

    /// Parse a Referrer-Policy HTTP header value.
    ///
    /// The standard allows a comma-separated policy list so newer tokens can
    /// follow older fallbacks. User agents select the last token they
    /// understand, therefore unknown entries do not erase an earlier valid
    /// policy.
    pub fn from_header(value: &str) -> Option<Self> {
        value.split(',').filter_map(Self::parse_token).last()
    }

    /// Parse Referrer-Policy from a response's raw header fields.
    ///
    /// Referrer Policy's response algorithm first extracts the complete header
    /// list and then walks every comma-separated token in wire order. Keeping
    /// raw fields separate here matters because HeaderMap::get() is a combined
    /// representation and redirect responses may legally repeat the header.
    /// The last recognized non-empty policy wins; extension/unknown tokens are
    /// ignored so servers can deploy new values with older fallbacks.
    pub fn from_response(response: &FetchResponse) -> Option<Self> {
        response
            .headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("referrer-policy"))
            .flat_map(|(_, value)| value.split(','))
            .filter_map(Self::parse_token)
            .last()
    }

    /// Apply Referrer Policy's redirect-update step to an existing policy.
    ///
    /// A redirect response with no recognized Referrer-Policy value leaves the
    /// request policy unchanged. A recognized value replaces it for the next
    /// hop. This small state transition is intentionally transport-neutral so
    /// redirect orchestrators can update policy before recomputing Referer.
    pub fn updated_on_redirect(self, response: &FetchResponse) -> Self {
        Self::from_response(response).unwrap_or(self)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoReferrer => "no-referrer",
            Self::NoReferrerWhenDowngrade => "no-referrer-when-downgrade",
            Self::Origin => "origin",
            Self::OriginWhenCrossOrigin => "origin-when-cross-origin",
            Self::SameOrigin => "same-origin",
            Self::StrictOrigin => "strict-origin",
            Self::StrictOriginWhenCrossOrigin => "strict-origin-when-cross-origin",
            Self::UnsafeUrl => "unsafe-url",
        }
    }

    /// Compute the value suitable for an outgoing HTTP `Referer` header.
    ///
    /// `None` means the header must be omitted. Fragment identifiers are never
    /// exposed. This engine's URL type already discards URL credentials while
    /// parsing, so the resulting value is also credential-free. Referrer
    /// Policy additionally requires a serialized full referrer longer than
    /// 4096 characters to be reduced to its origin before the policy is
    /// evaluated; `full_referrer` applies that limit centrally.
    pub fn compute(self, source: &Url, target: &Url) -> Option<String> {
        if !is_http_family(source) || !is_http_family(target) || source.host().is_empty() {
            return None;
        }

        let same = same_origin(source, target);
        let downgrade = is_downgrade(source, target);

        match self {
            Self::NoReferrer => None,
            Self::NoReferrerWhenDowngrade => {
                if downgrade { None } else { full_referrer(source) }
            }
            Self::Origin => origin_referrer(source),
            Self::OriginWhenCrossOrigin => {
                if same { full_referrer(source) } else { origin_referrer(source) }
            }
            Self::SameOrigin => {
                if same { full_referrer(source) } else { None }
            }
            Self::StrictOrigin => {
                if downgrade { None } else { origin_referrer(source) }
            }
            Self::StrictOriginWhenCrossOrigin => {
                if same {
                    full_referrer(source)
                } else if downgrade {
                    None
                } else {
                    origin_referrer(source)
                }
            }
            Self::UnsafeUrl => full_referrer(source),
        }
    }
}

fn is_http_family(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
}

fn effective_port(url: &Url) -> Option<u16> {
    url.port().or(match url.scheme() {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    })
}

fn default_http_port(scheme: &str) -> Option<u16> {
    match scheme {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    }
}

fn same_origin(a: &Url, b: &Url) -> bool {
    is_http_family(a)
        && is_http_family(b)
        && a.scheme() == b.scheme()
        && a.host().eq_ignore_ascii_case(b.host())
        && effective_port(a) == effective_port(b)
}

fn is_downgrade(source: &Url, target: &Url) -> bool {
    source.scheme() == "https" && target.scheme() == "http"
}

fn origin_referrer(source: &Url) -> Option<String> {
    let port = effective_port(source)?;
    let default = default_http_port(source.scheme())?;
    let authority = if port == default {
        source.host().to_string()
    } else {
        format!("{}:{port}", source.host())
    };
    Some(format!("{}://{authority}/", source.scheme()))
}

/// Serialize the stripped HTTP(S) URL using WHATWG default-port semantics.
///
/// The engine's general-purpose `Url` type intentionally preserves an explicit
/// `:80`/`:443` in `port()`. A standards URL record instead stores a special
/// scheme's default port as null, so its serializer omits that port. Referrer
/// Policy consumes the URL record after stripping credentials/fragment, hence
/// normalize the default port here rather than leaking the engine's internal
/// preservation detail onto the wire.
fn serialized_full_referrer(source: &Url) -> Option<String> {
    let default = default_http_port(source.scheme())?;
    if source.host().is_empty() {
        return None;
    }

    let mut serialized = format!("{}://{}", source.scheme(), source.host());
    if let Some(port) = source.port() {
        if port != default {
            serialized.push(':');
            serialized.push_str(&port.to_string());
        }
    }

    let path = source.path();
    if path.is_empty() {
        serialized.push('/');
    } else {
        serialized.push_str(path);
    }
    if let Some(query) = source.query() {
        serialized.push('?');
        serialized.push_str(query);
    }
    Some(serialized)
}

fn full_referrer(source: &Url) -> Option<String> {
    if !is_http_family(source) || source.host().is_empty() {
        return None;
    }
    let serialized = serialized_full_referrer(source)?;
    if serialized.chars().count() > 4096 {
        origin_referrer(source)
    } else {
        Some(serialized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    fn response_with_policies(fields: &[&str]) -> FetchResponse {
        let mut response = FetchResponse::synthetic(
            url("https://redirect.test/hop"),
            302,
            None,
            Vec::new(),
        );
        for value in fields {
            response.headers.append_raw("referrer-policy", value);
        }
        response
    }

    #[test]
    fn redirect_state_recomputes_from_stable_source_after_policy_change() {
        let source = url("https://source.test/private/page?q=1#secret");
        let mut state = RedirectReferrerState::new(
            Some(source),
            ReferrerPolicy::StrictOriginWhenCrossOrigin,
        );
        let mut first = FetchRequest::get(url("https://source.test/first"));
        state.prepare_request(&mut first);
        assert_eq!(
            first.headers.get("referer"),
            Some("https://source.test/private/page?q=1".to_string())
        );

        let response = response_with_policies(&["unsafe-url", "origin"]);
        state.observe_redirect_response(&response);
        let mut next = FetchRequest::get(url("https://other.test/next"));
        state.prepare_request(&mut next);
        assert_eq!(
            next.headers.get("referer"),
            Some("https://source.test/".to_string())
        );
    }

    #[test]
    fn redirect_state_removes_stale_referer_when_policy_becomes_no_referrer() {
        let mut state = RedirectReferrerState::from_source(url(
            "https://source.test/private/page?q=1#secret",
        ));
        let response = response_with_policies(&["no-referrer"]);
        state.observe_redirect_response(&response);

        let mut next = FetchRequest::get(url("https://target.test/next"));
        next.headers.insert_raw("referer", "https://stale.invalid/leak");
        state.prepare_request(&mut next);
        assert!(!next.headers.has("referer"));
    }

    #[test]
    fn header_uses_last_recognized_policy() {
        assert_eq!(
            ReferrerPolicy::from_header("no-referrer, future-policy, origin"),
            Some(ReferrerPolicy::Origin)
        );
        assert_eq!(ReferrerPolicy::from_header("future-policy"), None);
    }

    #[test]
    fn response_parser_uses_all_fields_in_wire_order() {
        let response = response_with_policies(&[
            "no-referrer, future-policy",
            "origin-when-cross-origin, strict-origin",
        ]);
        assert_eq!(
            ReferrerPolicy::from_response(&response),
            Some(ReferrerPolicy::StrictOrigin)
        );
    }

    #[test]
    fn redirect_policy_update_keeps_current_policy_when_response_has_no_known_token() {
        let response = response_with_policies(&["future-policy", "another-extension"]);
        assert_eq!(
            ReferrerPolicy::Origin.updated_on_redirect(&response),
            ReferrerPolicy::Origin
        );
    }

    #[test]
    fn redirect_policy_update_adopts_last_known_response_token() {
        let response = response_with_policies(&[
            "unsafe-url, future-policy",
            "strict-origin-when-cross-origin",
        ]);
        assert_eq!(
            ReferrerPolicy::UnsafeUrl.updated_on_redirect(&response),
            ReferrerPolicy::StrictOriginWhenCrossOrigin
        );
    }

    #[test]
    fn strict_origin_when_cross_origin_is_modern_default() {
        let source = url("https://example.test/a/page?q=1#secret");
        assert_eq!(
            ReferrerPolicy::default().compute(&source, &url("https://example.test/b")),
            Some("https://example.test/a/page?q=1".into())
        );
        assert_eq!(
            ReferrerPolicy::default().compute(&source, &url("https://cdn.example.test/x")),
            Some("https://example.test/".into())
        );
        assert_eq!(
            ReferrerPolicy::default().compute(&source, &url("http://example.test/x")),
            None
        );
    }

    #[test]
    fn default_ports_are_same_origin_and_omitted_from_referrers() {
        let source = url("https://example.test:443/path?q=1#secret");
        assert_eq!(
            ReferrerPolicy::SameOrigin.compute(&source, &url("https://example.test/next")),
            Some("https://example.test/path?q=1".into())
        );
        assert_eq!(
            ReferrerPolicy::Origin.compute(&source, &url("https://other.test/next")),
            Some("https://example.test/".into())
        );
        assert_eq!(
            ReferrerPolicy::SameOrigin.compute(&source, &url("https://example.test:444/next")),
            None
        );
    }

    #[test]
    fn nondefault_ports_remain_in_full_and_origin_referrers() {
        let source = url("https://example.test:8443/path?q=1#secret");
        assert_eq!(
            ReferrerPolicy::UnsafeUrl.compute(&source, &url("https://other.test/next")),
            Some("https://example.test:8443/path?q=1".into())
        );
        assert_eq!(
            ReferrerPolicy::Origin.compute(&source, &url("https://other.test/next")),
            Some("https://example.test:8443/".into())
        );
    }

    #[test]
    fn fragments_never_leave_the_document() {
        let source = url("https://example.test/path?q=1#token");
        assert_eq!(
            ReferrerPolicy::UnsafeUrl.compute(&source, &url("http://other.test/")),
            Some("https://example.test/path?q=1".into())
        );
    }

    #[test]
    fn oversized_full_referrer_falls_back_to_origin() {
        let prefix = "https://example.test/";
        let source = url(&format!("{prefix}{}", "a".repeat(4097 - prefix.chars().count())));
        assert_eq!(source.without_fragment().to_string().chars().count(), 4097);
        assert_eq!(
            ReferrerPolicy::UnsafeUrl.compute(&source, &url("https://other.test/")),
            Some("https://example.test/".into())
        );
    }

    #[test]
    fn referrer_at_4096_characters_is_not_reduced() {
        let prefix = "https://example.test/";
        let source_text = format!("{prefix}{}", "a".repeat(4096 - prefix.chars().count()));
        let source = url(&source_text);
        assert_eq!(source.without_fragment().to_string().chars().count(), 4096);
        assert_eq!(
            ReferrerPolicy::UnsafeUrl.compute(&source, &url("https://other.test/")),
            Some(source_text)
        );
    }

    #[test]
    fn local_and_opaque_sources_do_not_create_http_referrers() {
        assert_eq!(
            ReferrerPolicy::UnsafeUrl.compute(&url("file:///tmp/a.html"), &url("https://example.test/")),
            None
        );
        assert_eq!(
            ReferrerPolicy::UnsafeUrl.compute(&url("data:text/plain,hello"), &url("https://example.test/")),
            None
        );
    }
}
