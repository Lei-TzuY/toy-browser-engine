// ============================================================
//  navigation_network_credentials_ext.rs — Fetch credential policy
// ============================================================

/// Cookie credential policy for browser-owned requests layered on top of the
/// synchronous NavigationNetwork redirect loop.
///
/// `SameOrigin` compares every effective redirect-hop URL against the stable
/// request/client source, after HSTS has transformed the target. `Include`
/// allows cookie processing on any HTTP(S) hop, still subject to the cookie
/// jar's Domain/Path/Secure/SameSite rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NetworkCredentialsMode {
    SameOrigin,
    Include,
}

impl NetworkCredentialsMode {
    fn allows(self, source: Option<&Url>, target: &Url) -> bool {
        match self {
            Self::Include => true,
            Self::SameOrigin => source.is_some_and(|source| {
                crate::net::Origin::of(source) == crate::net::Origin::of(target)
            }),
        }
    }
}

impl NavigationNetwork {
    /// Perform a redirect-aware request while enforcing a Fetch credential
    /// mode against a stable client/source URL.
    ///
    /// This is deliberately crate-private: HTML-facing callers translate their
    /// own state (for example `crossorigin`) into this transport-neutral policy.
    /// Existing navigation entry points retain their historical always-eligible
    /// cookie behavior, so this addition is source- and behavior-compatible.
    pub(crate) fn fetch_with_referrer_and_credentials(
        &self,
        request: &FetchRequest,
        context: SameSiteRequestContext,
        mut referrer: Option<RedirectReferrerState>,
        credentials_mode: NetworkCredentialsMode,
        credential_source: Option<&Url>,
    ) -> Result<FetchResponse, FetchError> {
        let mut planner = RedirectPlanner::default();
        let mut current = request.clone();
        let mut hop_context = SameSiteRequestContext::new(
            context.same_site,
            context.top_level_navigation,
            current.method,
        );

        loop {
            let (mut effective, allow_credentials) = self.prepare_request_with_credentials(
                &current,
                hop_context,
                credentials_mode,
                credential_source,
            );
            if let Some(state) = referrer.as_ref() {
                state.prepare_request(&mut effective);
            }

            let mut response = self.loader.fetch_once(&effective)?;

            // Set-Cookie is a credentialed side effect. Anonymous CORS may
            // still learn transport security from a cross-origin response, but
            // it must not mutate the cookie jar when the credentials mode says
            // this hop is ineligible.
            self.absorb_response_with_credentials(&mut response, allow_credentials);

            let next = planner
                .next_request(&effective, &response)
                .map_err(fetch_error_from_redirect)?;
            let Some(mut next) = next else {
                if planner.followed() > 0 {
                    response.redirected = true;
                }
                return Ok(response);
            };

            if let Some(state) = referrer.as_mut() {
                state.observe_redirect_response(&response);
            }

            let next_effective = self.effective_url(&next.url);
            let chain_same_site = hop_context.same_site
                && conservative_same_site(&effective.url, &next_effective);
            next.url = next_effective;
            hop_context = SameSiteRequestContext::new(
                chain_same_site,
                hop_context.top_level_navigation,
                next.method,
            );
            current = next;
        }
    }

    /// Apply HSTS, strip any caller-authored Cookie header, and conditionally
    /// attach jar cookies according to the requested credential mode.
    fn prepare_request_with_credentials(
        &self,
        request: &FetchRequest,
        context: SameSiteRequestContext,
        credentials_mode: NetworkCredentialsMode,
        credential_source: Option<&Url>,
    ) -> (FetchRequest, bool) {
        let mut effective = request.clone();
        effective.url = self.effective_url(&effective.url);
        let allow_credentials = credentials_mode.allows(credential_source, &effective.url);

        if matches!(effective.url.scheme(), "http" | "https") {
            // Cookie is browser-owned even when credentials are disabled. This
            // prevents an authored header from bypassing credential mode.
            effective.headers.delete("cookie");

            if allow_credentials {
                if let Some(value) = self.cookie_jar.borrow().get_http_cookie_header_for_context(
                    &effective.url,
                    self.now_ms(),
                    SameSiteRequestContext::new(
                        context.same_site,
                        context.top_level_navigation,
                        effective.method,
                    ),
                ) {
                    effective.headers.insert_raw("cookie", &value);
                }
            }
        }

        (effective, allow_credentials)
    }

    /// Apply response-owned browser state without accepting Set-Cookie when the
    /// request's credential mode excluded credentials for this hop.
    fn absorb_response_with_credentials(
        &self,
        response: &mut FetchResponse,
        allow_credentials: bool,
    ) {
        if allow_credentials {
            self.absorb_response(response);
            return;
        }

        // Never expose or store response cookies from an ineligible request.
        response.headers.delete("set-cookie");

        // HSTS is transport security state, not a credential, so it continues
        // to be learned from otherwise valid HTTPS responses.
        let first_sts = response
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("strict-transport-security"))
            .map(|(_, value)| value.to_string());
        if let Some(value) = first_sts {
            self.hsts_cache
                .borrow_mut()
                .observe_response(&response.url, &value, self.now_ms());
        }
    }
}
