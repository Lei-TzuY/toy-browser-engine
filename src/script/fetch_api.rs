// ============================================================
//  script/fetch_api.rs  —  fetch(), Request, Response, Headers
// ============================================================
//
//  The script-facing half of networking. It sits between two modules that know
//  nothing about each other: `net::fetch` owns the wire format and the
//  backends, `script::host` owns the objects a page holds, and this file is
//  where a JavaScript call becomes a request and a completion becomes a
//  settled promise.
//
//  The one rule that shapes everything here: **`fetch()` performs no I/O**.
//  It validates its arguments, creates a pending promise, records the request
//  and returns — all on the caller's stack, in constant time. The document
//  hands the request to a backend on a later turn, and the answer arrives as a
//  task.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use crate::cookie_network::{
    policy_registry_for_jar, CookieCredentials, CookieRequestPolicy,
};
use crate::cookie_same_site::SameSiteRequestContext;
use crate::net::fetch::{
    FetchError, FetchRequest, FetchResponse, HeaderMap, Method, Origin,
};
use crate::net::Url;

use super::host::{
    decode_text, headers_ref, AbortState, Body, HeadersRef, HostObject, IntersectionObserverData,
    IntersectionObserverEntryData, IntersectionObserverTarget, RequestCredentials, RequestData,
    RequestMode, ResizeObserverData, ResizeObserverEntryData, ResponseData, ResponseType, UrlData,
    UrlSearchParamsData,
};
use super::interp::{object_get, to_number, to_string, truthy, Builtin, JsRuntime, JsValue};
use super::json;
use super::promise::{self, PromiseRef};

/// The schemes a page may fetch, on top of its own.
const FETCHABLE_SCHEMES: &[&str] = &["http", "https", "file"];

/// Fetch defaults preflight permissions to five seconds when Max-Age is absent
/// or invalid. A user agent may impose its own upper bound.
const DEFAULT_CORS_PREFLIGHT_MAX_AGE_SECS: u64 = 5;
const MAX_CORS_PREFLIGHT_MAX_AGE_SECS: u64 = 7_200;
const MAX_CORS_PREFLIGHT_CACHE_ENTRIES_PER_SESSION: usize = 256;

#[derive(Debug, Clone)]
struct CorsPreflightCacheEntry {
    source_origin: String,
    target_url: String,
    credentialed: bool,
    allowed_methods: Vec<String>,
    allowed_headers: Vec<String>,
    expires_at_ms: u64,
}

#[derive(Debug)]
struct CorsPreflightSessionCache {
    /// Browser documents in one session share a CookieJar. A weak handle gives
    /// the preflight cache the same lifetime without keeping dead sessions alive.
    jar: Weak<RefCell<crate::cookie::CookieJar>>,
    entries: Vec<CorsPreflightCacheEntry>,
}

thread_local! {
    static CORS_PREFLIGHT_CACHES: RefCell<Vec<CorsPreflightSessionCache>> = RefCell::new(Vec::new());
}

fn with_cors_preflight_session_cache<R>(
    jar: &Rc<RefCell<crate::cookie::CookieJar>>,
    operation: impl FnOnce(&mut Vec<CorsPreflightCacheEntry>) -> R,
) -> R {
    CORS_PREFLIGHT_CACHES.with(|sessions| {
        let mut sessions = sessions.borrow_mut();
        sessions.retain(|session| session.jar.upgrade().is_some());
        let index = sessions
            .iter()
            .position(|session| {
                session
                    .jar
                    .upgrade()
                    .is_some_and(|live| Rc::ptr_eq(&live, jar))
            })
            .unwrap_or_else(|| {
                sessions.push(CorsPreflightSessionCache {
                    jar: Rc::downgrade(jar),
                    entries: Vec::new(),
                });
                sessions.len() - 1
            });
        operation(&mut sessions[index].entries)
    })
}

/// Browser-only state needed while a CORS fetch is in flight.
#[derive(Debug, Clone)]
struct CorsFetchState {
    source_origin: Origin,
    credentialed: bool,
    requested_method: Method,
    requested_headers: Vec<String>,
    needs_preflight: bool,
}

/// Which network stage currently owns a script-visible Fetch promise.
#[derive(Debug)]
enum PendingFetchStage {
    Actual {
        cors: Option<CorsFetchState>,
    },
    Preflight {
        request: RequestData,
        cookie_policy: CookieRequestPolicy,
        cors: CorsFetchState,
    },
}

/// What the runtime keeps for one request it is waiting on.
#[derive(Debug)]
pub struct PendingFetch {
    pub promise: PromiseRef,
    /// The signal watching this request, if `fetch` was given one.
    pub signal: Option<Rc<AbortState>>,
    stage: PendingFetchStage,
}

fn rect_to_js(r: &[f32; 4]) -> JsValue {
    JsValue::Object(Rc::new(RefCell::new(vec![
        ("x".to_string(), JsValue::Number(r[0])),
        ("y".to_string(), JsValue::Number(r[1])),
        ("width".to_string(), JsValue::Number(r[2])),
        ("height".to_string(), JsValue::Number(r[3])),
    ])))
}

impl JsRuntime {
    // ── fetch() ───────────────────────────────────────────────────────────

    /// `fetch(input, init)` — returns a pending promise, always.
    pub fn start_fetch(&mut self, args: Vec<JsValue>) -> JsValue {
        let promise = promise::new_promise();

        match self.prepare_request(&args) {
            Err(error) => self.reject_with(&promise, &error),
            Ok((request, cookie_policy, cors)) => {
                let signal = request.signal.clone();
                if signal.as_ref().is_some_and(|state| state.aborted()) {
                    self.reject_with(&promise, &FetchError::Aborted);
                } else {
                    let needs_preflight = match cors.as_ref() {
                        Some(state) if state.needs_preflight => {
                            !self.cors_preflight_cache_allows(&request, state)
                        }
                        _ => false,
                    };
                    let queued = if needs_preflight {
                        let cors = cors.expect("preflight requires CORS state");
                        let preflight = build_cors_preflight_request(&request, &cors);
                        let pending = PendingFetch {
                            promise: promise.clone(),
                            signal,
                            stage: PendingFetchStage::Preflight {
                                request,
                                cookie_policy,
                                cors,
                            },
                        };
                        self.queue_fetch(preflight, pending, cors_preflight_cookie_policy())
                    } else {
                        let wire = request.to_wire();
                        let pending = PendingFetch {
                            promise: promise.clone(),
                            signal,
                            stage: PendingFetchStage::Actual { cors },
                        };
                        self.queue_fetch(wire, pending, cookie_policy)
                    };

                    if let Err(error) = queued {
                        self.reject_with(&promise, &error);
                    }
                }
            }
        }
        JsValue::Promise(promise)
    }

    /// Settle one network stage of a Fetch promise. A successful preflight
    /// queues the actual request; only the actual response is exposed to script.
    pub fn settle_fetch(
        &mut self,
        pending: PendingFetch,
        result: Result<FetchResponse, FetchError>,
    ) {
        let PendingFetch {
            promise,
            signal,
            stage,
        } = pending;

        if signal.as_ref().is_some_and(|state| state.aborted()) {
            self.reject_with(&promise, &FetchError::Aborted);
            return;
        }

        match stage {
            PendingFetchStage::Actual { cors } => match result {
                Ok(mut response) => {
                    if let Some(cors) = &cors {
                        if let Err(error) = validate_cors_response(
                            &cors.source_origin,
                            cors.credentialed,
                            &response,
                        ) {
                            self.reject_with(&promise, &error);
                            return;
                        }
                        filter_cors_response_headers(&mut response, cors.credentialed);
                    }
                    let mut response_data = ResponseData::from_wire(response);
                    if cors.is_some() {
                        response_data.response_type = ResponseType::Cors;
                    }
                    let value = host_value(HostObject::Response(response_data));
                    self.settle_resolve(&promise, value);
                }
                Err(error) => self.reject_with(&promise, &error),
            },
            PendingFetchStage::Preflight {
                request,
                cookie_policy,
                cors,
            } => {
                let response = match result {
                    Ok(response) => response,
                    Err(error) => {
                        self.reject_with(&promise, &error);
                        return;
                    }
                };
                if let Err(error) = validate_cors_preflight_response(&cors, &response) {
                    self.reject_with(&promise, &error);
                    return;
                }
                self.store_cors_preflight_permissions(&request, &cors, &response);

                let wire = request.to_wire();
                let actual = PendingFetch {
                    promise: promise.clone(),
                    signal,
                    stage: PendingFetchStage::Actual { cors: Some(cors) },
                };
                if let Err(error) = self.queue_fetch(wire, actual, cookie_policy) {
                    self.reject_with(&promise, &error);
                }
            }
        }
    }

    fn cors_preflight_cache_allows(
        &self,
        request: &RequestData,
        cors: &CorsFetchState,
    ) -> bool {
        let now_ms = self
            .cookie_jar
            .borrow()
            .effective_now_ms(self.now_ms.max(0.0) as u64);
        let source_origin = cors.source_origin.header_value();
        let target_url = request.url.without_fragment().to_string();

        with_cors_preflight_session_cache(&self.cookie_jar, |entries| {
            entries.retain(|entry| entry.expires_at_ms > now_ms);
            entries.iter().any(|entry| {
                if entry.source_origin != source_origin
                    || entry.target_url != target_url
                    || (!entry.credentialed && cors.credentialed)
                {
                    return false;
                }

                let method_allowed = is_cors_safelisted_method(cors.requested_method)
                    || entry.allowed_methods.iter().any(|method| {
                        method == cors.requested_method.as_str()
                            || (method == "*" && !cors.credentialed)
                    });
                method_allowed
                    && cors.requested_headers.iter().all(|requested| {
                        entry.allowed_headers.iter().any(|allowed| {
                            allowed.eq_ignore_ascii_case(requested)
                                || (allowed == "*"
                                    && !cors.credentialed
                                    && !is_cors_non_wildcard_request_header_name(requested))
                        })
                    })
            })
        })
    }

    fn store_cors_preflight_permissions(
        &self,
        request: &RequestData,
        cors: &CorsFetchState,
        response: &FetchResponse,
    ) {
        let max_age = cors_preflight_max_age_seconds(response);
        if max_age == 0 {
            return;
        }

        let now_ms = self
            .cookie_jar
            .borrow()
            .effective_now_ms(self.now_ms.max(0.0) as u64);
        let source_origin = cors.source_origin.header_value();
        let target_url = request.url.without_fragment().to_string();
        let expires_at_ms = now_ms.saturating_add(max_age.saturating_mul(1_000));
        let allowed_methods = comma_tokens(response.headers.get("access-control-allow-methods"));
        let allowed_headers = comma_tokens(response.headers.get("access-control-allow-headers"));

        with_cors_preflight_session_cache(&self.cookie_jar, |entries| {
            entries.retain(|entry| {
                entry.expires_at_ms > now_ms
                    && (entry.source_origin != source_origin
                        || entry.target_url != target_url
                        || entry.credentialed != cors.credentialed)
            });
            if entries.len() >= MAX_CORS_PREFLIGHT_CACHE_ENTRIES_PER_SESSION {
                entries.remove(0);
            }
            entries.push(CorsPreflightCacheEntry {
                source_origin,
                target_url,
                credentialed: cors.credentialed,
                allowed_methods,
                allowed_headers,
                expires_at_ms,
            });
        });
    }

    fn queue_fetch(
        &mut self,
        request: FetchRequest,
        pending: PendingFetch,
        cookie_policy: CookieRequestPolicy,
    ) -> Result<(), FetchError> {
        let id = self.fetches.start(request, pending)?;
        if let Some(registry) = policy_registry_for_jar(&self.cookie_jar) {
            registry.set(id, cookie_policy);
        }
        Ok(())
    }

    /// Reject every request watching `state`, and stop their delivery.
    fn abort_requests(&mut self, state: &Rc<AbortState>) {
        let aborted = self.fetches.take_where(|pending| {
            pending
                .signal
                .as_ref()
                .is_some_and(|signal| Rc::ptr_eq(signal, state))
        });
        for (_, pending) in aborted {
            self.reject_with(&pending.promise, &FetchError::Aborted);
        }
    }

    fn reject_with(&mut self, promise: &PromiseRef, error: &FetchError) {
        self.settle_reject(promise, JsValue::Str(error.to_string()));
    }

    // ── Building a request ────────────────────────────────────────────────

    /// Turn `(input, init)` into a request plus browser-only Fetch policy.
    fn prepare_request(
        &mut self,
        args: &[JsValue],
    ) -> Result<(RequestData, CookieRequestPolicy, Option<CorsFetchState>), FetchError> {
        let input = args.first().cloned().unwrap_or(JsValue::Undefined);
        let init = args.get(1).cloned().unwrap_or(JsValue::Undefined);
        let request = self.build_request(input, init)?;
        let mode = request.mode;

        let scheme = request.url.scheme();
        if !FETCHABLE_SCHEMES.contains(&scheme) && scheme != self.url.scheme() {
            return Err(FetchError::UnsupportedScheme(scheme.to_string()));
        }
        if request.credentials == RequestCredentials::Include
            && !matches!(scheme, "http" | "https")
        {
            return Err(FetchError::BadRequest(
                "credentials mode \"include\" is only supported for HTTP(S) requests".into(),
            ));
        }

        let source_origin = Origin::of(&self.url);
        let same_origin = source_origin.can_fetch(&request.url);
        let cross_origin_web = matches!(self.url.scheme(), "http" | "https")
            && matches!(request.url.scheme(), "http" | "https")
            && !same_origin;

        let cors = match mode {
            RequestMode::SameOrigin if !same_origin => {
                return Err(FetchError::Blocked(format!(
                    "{} may not fetch {} in same-origin mode",
                    source_origin.header_value(),
                    request.url
                )))
            }
            RequestMode::Cors if cross_origin_web => {
                // Origin and the preflight-control fields are browser-owned.
                // Strip authored copies before classification so they cannot
                // influence either the preflight or the eventual actual request.
                {
                    let mut headers = request.headers.borrow_mut();
                    headers.delete("origin");
                    headers.delete("access-control-request-method");
                    headers.delete("access-control-request-headers");
                }

                let requested_headers = cors_unsafe_request_header_names(&request);
                let needs_preflight = !is_cors_safelisted_method(request.method)
                    || !requested_headers.is_empty();
                request
                    .headers
                    .borrow_mut()
                    .insert_raw("origin", &source_origin.header_value());
                Some(CorsFetchState {
                    source_origin: source_origin.clone(),
                    credentialed: request.credentials == RequestCredentials::Include,
                    requested_method: request.method,
                    requested_headers,
                    needs_preflight,
                })
            }
            RequestMode::Cors if !same_origin => {
                // CORS is only meaningful for network tuple origins here. Keep
                // the existing local-file containment boundary intact.
                return Err(FetchError::Blocked(format!(
                    "{} may not fetch {}",
                    source_origin.header_value(),
                    request.url
                )))
            }
            _ => None,
        };

        let credentials = match request.credentials {
            RequestCredentials::Omit => CookieCredentials::Omit,
            RequestCredentials::SameOrigin if same_origin => CookieCredentials::Include,
            RequestCredentials::SameOrigin => CookieCredentials::Omit,
            RequestCredentials::Include => CookieCredentials::Include,
        };
        let same_site = if conservative_same_site(&self.url, &request.url) {
            SameSiteRequestContext::same_site(request.method)
        } else {
            SameSiteRequestContext::cross_site_subresource(request.method)
        };
        let cookie_policy = CookieRequestPolicy::new(credentials, same_site);

        Ok((request, cookie_policy, cors))
    }

    /// The `Request` constructor, shared with `fetch`'s first argument.
    fn build_request(&mut self, input: JsValue, init: JsValue) -> Result<RequestData, FetchError> {
        let (
            mut url,
            mut method,
            mut headers,
            mut body,
            mut signal,
            mut mode,
            mut credentials,
        ) = match &input {
            JsValue::Host(host) => match host.as_request() {
                Some(existing) => (
                    existing.url.clone(),
                    existing.method,
                    existing.headers.borrow().clone(),
                    existing.body.peek(),
                    existing.signal.clone(),
                    existing.mode,
                    existing.credentials,
                ),
                None => return Err(FetchError::InvalidUrl(to_string(&input))),
            },
            other => (
                self.resolve_fetch_url(&to_string(other))?,
                Method::Get,
                HeaderMap::new(),
                None,
                None,
                RequestMode::Cors,
                RequestCredentials::SameOrigin,
            ),
        };

        if let JsValue::Object(props) = &init {
            for (key, value) in props.borrow().iter() {
                match key.as_str() {
                    "method" => {
                        let text = to_string(value);
                        method = Method::parse(&text).ok_or(FetchError::UnsupportedMethod(text))?;
                    }
                    "headers" => headers = self.header_map_from(value)?,
                    "body" => {
                        body = match value {
                            JsValue::Undefined | JsValue::Null => None,
                            other => Some(to_string(other).into_bytes()),
                        }
                    }
                    "signal" => {
                        signal = match value {
                            JsValue::Host(host) => host.as_abort_state().cloned(),
                            JsValue::Undefined | JsValue::Null => None,
                            _ => {
                                return Err(FetchError::BadRequest(
                                    "signal must be an AbortSignal".into(),
                                ))
                            }
                        }
                    }
                    "mode" => mode = check_mode(&to_string(value))?,
                    "credentials" => credentials = check_credentials(&to_string(value))?,
                    _ => {}
                }
            }
        }

        if body.is_some() && !method.allows_body() {
            return Err(FetchError::BadRequest(format!(
                "a {method} request cannot have a body"
            )));
        }
        if url.scheme().is_empty() {
            url = self.resolve_fetch_url(&url.to_string())?;
        }

        Ok(RequestData {
            url,
            method,
            headers: headers_ref(headers),
            body: match body {
                Some(bytes) => Body::new(bytes),
                None => Body::empty(),
            },
            signal,
            mode,
            credentials,
        })
    }

    fn resolve_fetch_url(&self, reference: &str) -> Result<Url, FetchError> {
        let trimmed = reference.trim();
        if trimmed.is_empty() {
            return Err(FetchError::InvalidUrl("(empty)".into()));
        }
        self.url
            .join(trimmed)
            .map_err(|_| FetchError::InvalidUrl(trimmed.to_string()))
    }

    fn header_map_from(&self, value: &JsValue) -> Result<HeaderMap, FetchError> {
        let mut headers = HeaderMap::new();
        match value {
            JsValue::Host(host) => match host.as_headers() {
                Some(existing) => headers = existing.borrow().clone(),
                None => {
                    return Err(FetchError::BadRequest(
                        "headers must be an object or a Headers".into(),
                    ))
                }
            },
            JsValue::Object(props) => {
                for (name, value) in props.borrow().iter() {
                    if HeaderMap::is_forbidden(name) {
                        continue;
                    }
                    headers
                        .append(name, &to_string(value))
                        .map_err(|e| FetchError::BadRequest(e.to_string()))?;
                }
            }
            JsValue::Undefined | JsValue::Null => {}
            _ => {
                return Err(FetchError::BadRequest(
                    "headers must be an object or a Headers".into(),
                ))
            }
        }
        Ok(headers)
    }

    // ── Constructors ──────────────────────────────────────────────────────

    pub(crate) fn construct_host(&mut self, builtin: Builtin, args: Vec<JsValue>) -> JsValue {
        match builtin {
            Builtin::HeadersCtor => {
                let source = args.first().cloned().unwrap_or(JsValue::Undefined);
                match self.header_map_from(&source) {
                    Ok(headers) => host_value(HostObject::Headers(headers_ref(headers))),
                    Err(error) => {
                        self.throw_type_error(error.to_string());
                        JsValue::Undefined
                    }
                }
            }
            Builtin::RequestCtor => {
                let input = args.first().cloned().unwrap_or(JsValue::Undefined);
                let init = args.get(1).cloned().unwrap_or(JsValue::Undefined);
                match self.build_request(input, init) {
                    Ok(request) => host_value(HostObject::Request(request)),
                    Err(error) => {
                        self.throw_type_error(error.to_string());
                        JsValue::Undefined
                    }
                }
            }
            Builtin::ResponseCtor => {
                let body = match args.first() {
                    None | Some(JsValue::Undefined) | Some(JsValue::Null) => Vec::new(),
                    Some(other) => to_string(other).into_bytes(),
                };
                let mut status = 200u16;
                let mut status_text: Option<String> = None;
                let mut headers = HeaderMap::new();

                if let Some(JsValue::Object(props)) = args.get(1) {
                    for (key, value) in props.borrow().iter() {
                        match key.as_str() {
                            "status" => {
                                status = super::interp::to_number(value).max(0.0) as u16;
                            }
                            "statusText" => status_text = Some(to_string(value)),
                            "headers" => match self.header_map_from(value) {
                                Ok(map) => headers = map,
                                Err(error) => {
                                    self.throw_type_error(error.to_string());
                                    return JsValue::Undefined;
                                }
                            },
                            _ => {}
                        }
                    }
                }
                let response = ResponseData {
                    url: self.url.clone(),
                    status,
                    status_text: status_text
                        .unwrap_or_else(|| crate::net::fetch::reason_phrase(status).to_string()),
                    headers: headers_ref(headers),
                    body: Body::new(body),
                    redirected: false,
                    response_type: ResponseType::Basic,
                };
                host_value(HostObject::Response(response))
            }
            Builtin::AbortControllerCtor => {
                host_value(HostObject::AbortController(AbortState::new()))
            }
            Builtin::URLCtor => {
                let url_str = to_string(args.first().unwrap_or(&JsValue::Undefined));
                let base_str = args.get(1).map(to_string);
                let parsed_url = if let Some(base) = base_str {
                    match Url::parse(&base) {
                        Ok(base_u) => match base_u.join(&url_str) {
                            Ok(u) => u,
                            Err(_) => {
                                self.throw_type_error(format!("Invalid URL: {url_str} with base {base}"));
                                return JsValue::Undefined;
                            }
                        },
                        Err(_) => {
                            self.throw_type_error(format!("Invalid base URL: {base}"));
                            return JsValue::Undefined;
                        }
                    }
                } else {
                    match Url::parse(&url_str) {
                        Ok(u) => u,
                        Err(_) => {
                            self.throw_type_error(format!("Invalid URL: {url_str}"));
                            return JsValue::Undefined;
                        }
                    }
                };
                host_value(HostObject::URL(Rc::new(RefCell::new(UrlData::new(parsed_url)))))
            }
            Builtin::URLSearchParamsCtor => {
                let init_val = args.first().unwrap_or(&JsValue::Undefined);
                let params = match init_val {
                    JsValue::Str(s) => UrlSearchParamsData::from_query(s, None),
                    JsValue::Array(arr) => {
                        let mut pairs = Vec::new();
                        for item in arr.borrow().iter() {
                            if let JsValue::Array(pair_arr) = item {
                                let b = pair_arr.borrow();
                                let k = b.first().map(to_string).unwrap_or_default();
                                let v = b.get(1).map(to_string).unwrap_or_default();
                                pairs.push((k, v));
                            }
                        }
                        UrlSearchParamsData::new(pairs, None)
                    }
                    JsValue::Host(h) => {
                        if let HostObject::URLSearchParams(other) = h.as_ref() {
                            UrlSearchParamsData::new(other.borrow().pairs.borrow().clone(), None)
                        } else {
                            UrlSearchParamsData::new(Vec::new(), None)
                        }
                    }
                    _ => UrlSearchParamsData::new(Vec::new(), None),
                };
                host_value(HostObject::URLSearchParams(Rc::new(RefCell::new(params))))
            }
            Builtin::AudioContextCtor => {
                host_value(HostObject::AudioContext(Rc::new(RefCell::new(crate::audio::AudioContext::new()))))
            }
            Builtin::IntersectionObserverCtor => {
                let mut thresholds = vec![0.0];
                if let Some(JsValue::Object(opts)) = args.get(1) {
                    for (k, v) in opts.borrow().iter() {
                        if k == "threshold" {
                            match v {
                                JsValue::Number(n) => thresholds = vec![*n],
                                JsValue::Array(arr) => {
                                    thresholds = arr.borrow().iter().map(|x| to_number(x)).collect();
                                }
                                _ => {}
                            }
                        }
                    }
                }
                let data = IntersectionObserverData::new(thresholds);
                host_value(HostObject::IntersectionObserver(Rc::new(RefCell::new(data))))
            }
            Builtin::ResizeObserverCtor => {
                let data = ResizeObserverData::new();
                host_value(HostObject::ResizeObserver(Rc::new(RefCell::new(data))))
            }
            Builtin::MapCtor => {
                let entries: Vec<(String, JsValue)> = if let Some(JsValue::Array(arr)) = args.first() {
                    arr.borrow().iter().filter_map(|item| {
                        if let JsValue::Array(pair) = item {
                            let pair = pair.borrow();
                            if pair.len() >= 2 {
                                Some((to_string(&pair[0]), pair[1].clone()))
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }).collect()
                } else {
                    Vec::new()
                };
                host_value(HostObject::JsMap(Rc::new(RefCell::new(entries))))
            }
            Builtin::SetCtor => {
                let items: Vec<String> = if let Some(JsValue::Array(arr)) = args.first() {
                    let mut seen = Vec::new();
                    for item in arr.borrow().iter() {
                        let s = to_string(item);
                        if !seen.contains(&s) {
                            seen.push(s);
                        }
                    }
                    seen
                } else {
                    Vec::new()
                };
                host_value(HostObject::JsSet(Rc::new(RefCell::new(items))))
            }
            other => {
                self.throw_type_error(format!("{other:?} is not a constructor"));
                JsValue::Undefined
            }
        }
    }

    // ── Properties ────────────────────────────────────────────────────────

    pub(crate) fn host_member(&mut self, host: &Rc<HostObject>, prop: &str) -> JsValue {
        match host.as_ref() {
            HostObject::Headers(_) => JsValue::Undefined,
            HostObject::Request(request) => match prop {
                "url" => JsValue::Str(request.url.to_string()),
                "method" => JsValue::Str(request.method.as_str().to_string()),
                "headers" => host_value(HostObject::Headers(request.headers.clone())),
                "bodyUsed" => JsValue::Bool(request.body.used()),
                "mode" => JsValue::Str(request.mode.as_str().to_string()),
                "credentials" => JsValue::Str(request.credentials.as_str().to_string()),
                "signal" => match &request.signal {
                    Some(state) => host_value(HostObject::AbortSignal(state.clone())),
                    None => JsValue::Null,
                },
                _ => JsValue::Undefined,
            },
            HostObject::Response(response) => match prop {
                "status" => JsValue::Number(response.status as f32),
                "statusText" => JsValue::Str(response.status_text.clone()),
                "ok" => JsValue::Bool(response.ok()),
                "url" => JsValue::Str(response.url.to_string()),
                "redirected" => JsValue::Bool(response.redirected),
                "headers" => host_value(HostObject::Headers(response.headers.clone())),
                "bodyUsed" => JsValue::Bool(response.body.used()),
                "type" => JsValue::Str(response.response_type.as_str().to_string()),
                _ => JsValue::Undefined,
            },
            HostObject::AbortController(state) => match prop {
                "signal" => host_value(HostObject::AbortSignal(state.clone())),
                _ => JsValue::Undefined,
            },
            HostObject::AbortSignal(state) => match prop {
                "aborted" => JsValue::Bool(state.aborted()),
                _ => JsValue::Undefined,
            },
            HostObject::URL(u_rc) => {
                let u = u_rc.borrow();
                match prop {
                    "href" => JsValue::Str(u.url.to_string()),
                    "origin" => JsValue::Str(u.origin()),
                    "protocol" => JsValue::Str(format!("{}:", u.url.scheme())),
                    "host" => JsValue::Str(if let Some(port) = u.url.port() {
                        format!("{}:{}", u.url.host(), port)
                    } else {
                        u.url.host().to_string()
                    }),
                    "hostname" => JsValue::Str(u.url.host().to_string()),
                    "port" => JsValue::Str(u.url.port().map(|p| p.to_string()).unwrap_or_default()),
                    "pathname" => JsValue::Str(u.url.path().to_string()),
                    "search" => JsValue::Str(u.url.query().map(|q| format!("?{}", q)).unwrap_or_default()),
                    "hash" => JsValue::Str(u.url.fragment().map(|f| format!("#{}", f)).unwrap_or_default()),
                    "searchParams" => {
                        let qs = u.url.query().unwrap_or("");
                        let params = UrlSearchParamsData::from_query(qs, Some(u_rc.clone()));
                        host_value(HostObject::URLSearchParams(Rc::new(RefCell::new(params))))
                    }
                    _ => JsValue::Undefined,
                }
            }
            HostObject::URLSearchParams(params) => match prop {
                "size" => JsValue::Number(params.borrow().pairs.borrow().len() as f32),
                _ => JsValue::Undefined,
            },
            HostObject::CanvasRenderingContext2D(ctx) => {
                let ctx = ctx.borrow();
                match prop {
                    "fillStyle" => JsValue::Str(format!(
                        "rgba({}, {}, {}, {})",
                        ctx.fill_style.r,
                        ctx.fill_style.g,
                        ctx.fill_style.b,
                        ctx.fill_style.a as f32 / 255.0
                    )),
                    "strokeStyle" => JsValue::Str(format!(
                        "rgba({}, {}, {}, {})",
                        ctx.stroke_style.r,
                        ctx.stroke_style.g,
                        ctx.stroke_style.b,
                        ctx.stroke_style.a as f32 / 255.0
                    )),
                    "lineWidth" => JsValue::Number(ctx.line_width),
                    "font" => JsValue::Str(format!("{}px sans-serif", ctx.font_size)),
                    "textAlign" => JsValue::Str(match ctx.text_align {
                        crate::layout::TextAlign::Left => "left".to_string(),
                        crate::layout::TextAlign::Center => "center".to_string(),
                        crate::layout::TextAlign::Right => "right".to_string(),
                    }),
                    "globalAlpha" => JsValue::Number(ctx.global_alpha),
                    "filter" => JsValue::Str(ctx.filter.clone()),
                    _ => JsValue::Undefined,
                }
            }
            HostObject::AudioContext(ctx) => {
                let c = ctx.borrow();
                match prop {
                    "sampleRate" => JsValue::Number(c.sample_rate),
                    "state" => JsValue::Str(c.state.clone()),
                    "destination" => host_value(HostObject::AudioNode(ctx.clone(), c.destination_id)),
                    _ => JsValue::Undefined,
                }
            }
            HostObject::AudioNode(ctx, node_id) => {
                let c = ctx.borrow();
                let Some(node) = c.get_node(*node_id) else {
                    return JsValue::Undefined;
                };
                match &node.kind {
                    crate::audio::AudioNodeKind::Oscillator { osc_type, .. } => match prop {
                        "type" => JsValue::Str(osc_type.as_str().to_string()),
                        "frequency" => host_value(HostObject::AudioParam(ctx.clone(), *node_id, "frequency".to_string())),
                        _ => JsValue::Undefined,
                    },
                    crate::audio::AudioNodeKind::Gain { .. } => match prop {
                        "gain" => host_value(HostObject::AudioParam(ctx.clone(), *node_id, "gain".to_string())),
                        _ => JsValue::Undefined,
                    },
                    crate::audio::AudioNodeKind::Destination => match prop {
                        "maxChannelCount" => JsValue::Number(2.0),
                        _ => JsValue::Undefined,
                    },
                }
            }
            HostObject::AudioParam(ctx, node_id, param_name) => {
                let c = ctx.borrow();
                let Some(node) = c.get_node(*node_id) else {
                    return JsValue::Undefined;
                };
                let param = match &node.kind {
                    crate::audio::AudioNodeKind::Oscillator { frequency, .. } if param_name == "frequency" => frequency,
                    crate::audio::AudioNodeKind::Gain { gain } if param_name == "gain" => gain,
                    _ => return JsValue::Undefined,
                };
                match prop {
                    "value" => JsValue::Number(param.value),
                    "defaultValue" => JsValue::Number(param.default_value),
                    "minValue" => JsValue::Number(param.min_value),
                    "maxValue" => JsValue::Number(param.max_value),
                    _ => JsValue::Undefined,
                }
            }
            HostObject::IntersectionObserver(data) => {
                let d = data.borrow();
                match prop {
                    "root" => JsValue::Null,
                    "rootMargin" => JsValue::Str(d.root_margin.clone()),
                    "thresholds" => {
                        let arr: Vec<JsValue> = d.thresholds.iter().map(|t| JsValue::Number(*t)).collect();
                        JsValue::Array(Rc::new(RefCell::new(arr)))
                    }
                    _ => JsValue::Undefined,
                }
            }
            HostObject::IntersectionObserverEntry(entry) => match prop {
                "isIntersecting" => JsValue::Bool(entry.is_intersecting),
                "intersectionRatio" => JsValue::Number(entry.intersection_ratio),
                "target" => JsValue::Str(entry.target_id.clone()),
                "boundingClientRect" => rect_to_js(&entry.bounding_client_rect),
                "intersectionRect" => rect_to_js(&entry.intersection_rect),
                "rootBounds" => rect_to_js(&entry.root_bounds),
                _ => JsValue::Undefined,
            },
            HostObject::ResizeObserver(_) => JsValue::Undefined,
            HostObject::ResizeObserverEntry(entry) => match prop {
                "target" => JsValue::Str(entry.target_id.clone()),
                "contentRect" => rect_to_js(&entry.content_rect),
                _ => JsValue::Undefined,
            },
            HostObject::JsMap(entries) => match prop {
                "size" => JsValue::Number(entries.borrow().len() as f32),
                _ => JsValue::Undefined,
            },
            HostObject::JsSet(items) => match prop {
                "size" => JsValue::Number(items.borrow().len() as f32),
                _ => JsValue::Undefined,
            },
            HostObject::Crypto => JsValue::Undefined,
        }
    }

    // ── Methods ───────────────────────────────────────────────────────────

    pub(crate) fn host_method(
        &mut self,
        host: &Rc<HostObject>,
        prop: &str,
        args: Vec<JsValue>,
    ) -> JsValue {
        match host.as_ref() {
            HostObject::Headers(headers) => self.headers_method(headers, prop, &args),
            HostObject::Request(request) => match prop {
                "text" => self.consume_body(&request.body, false),
                "json" => self.consume_body(&request.body, true),
                _ => JsValue::Undefined,
            },
            HostObject::Response(response) => match prop {
                "text" => self.consume_body(&response.body, false),
                "json" => self.consume_body(&response.body, true),
                _ => JsValue::Undefined,
            },
            HostObject::AbortController(state) => match prop {
                "abort" => {
                    if state.abort() {
                        let state = state.clone();
                        self.abort_requests(&state);
                    }
                    JsValue::Undefined
                }
                _ => JsValue::Undefined,
            },
            HostObject::AbortSignal(_) => JsValue::Undefined,
            HostObject::URL(u_rc) => match prop {
                "toString" | "toJSON" => JsValue::Str(u_rc.borrow().url.to_string()),
                _ => JsValue::Undefined,
            },
            HostObject::URLSearchParams(params) => match prop {
                "get" => {
                    let name = to_string(args.first().unwrap_or(&JsValue::Undefined));
                    params.borrow().get(&name).map(JsValue::Str).unwrap_or(JsValue::Null)
                }
                "getAll" => {
                    let name = to_string(args.first().unwrap_or(&JsValue::Undefined));
                    let all: Vec<JsValue> = params.borrow().get_all(&name).into_iter().map(JsValue::Str).collect();
                    JsValue::Array(Rc::new(std::cell::RefCell::new(all)))
                }
                "has" => {
                    let name = to_string(args.first().unwrap_or(&JsValue::Undefined));
                    JsValue::Bool(params.borrow().has(&name))
                }
                "set" => {
                    let name = to_string(args.first().unwrap_or(&JsValue::Undefined));
                    let val = to_string(args.get(1).unwrap_or(&JsValue::Undefined));
                    params.borrow().set(&name, &val);
                    JsValue::Undefined
                }
                "append" => {
                    let name = to_string(args.first().unwrap_or(&JsValue::Undefined));
                    let val = to_string(args.get(1).unwrap_or(&JsValue::Undefined));
                    params.borrow().append(&name, &val);
                    JsValue::Undefined
                }
                "delete" => {
                    let name = to_string(args.first().unwrap_or(&JsValue::Undefined));
                    params.borrow().delete(&name);
                    JsValue::Undefined
                }
                "toString" => JsValue::Str(params.borrow().to_query_string()),
                "keys" => {
                    let keys: Vec<JsValue> = params.borrow().pairs.borrow().iter().map(|(k, _)| JsValue::Str(k.clone())).collect();
                    JsValue::Array(Rc::new(std::cell::RefCell::new(keys)))
                }
                "values" => {
                    let vals: Vec<JsValue> = params.borrow().pairs.borrow().iter().map(|(_, v)| JsValue::Str(v.clone())).collect();
                    JsValue::Array(Rc::new(std::cell::RefCell::new(vals)))
                }
                "entries" => {
                    let entries: Vec<JsValue> = params.borrow().pairs.borrow().iter().map(|(k, v)| {
                        JsValue::Array(Rc::new(std::cell::RefCell::new(vec![JsValue::Str(k.clone()), JsValue::Str(v.clone())])))
                    }).collect();
                    JsValue::Array(Rc::new(std::cell::RefCell::new(entries)))
                }
                _ => JsValue::Undefined,
            },
            HostObject::CanvasRenderingContext2D(ctx) => {
                self.canvas_context_method(ctx, prop, args)
            }
            HostObject::AudioContext(ctx) => match prop {
                "createOscillator" => {
                    let id = ctx.borrow_mut().create_oscillator();
                    host_value(HostObject::AudioNode(ctx.clone(), id))
                }
                "createGain" => {
                    let id = ctx.borrow_mut().create_gain();
                    host_value(HostObject::AudioNode(ctx.clone(), id))
                }
                "close" => {
                    ctx.borrow_mut().state = "closed".to_string();
                    JsValue::Undefined
                }
                "resume" => {
                    ctx.borrow_mut().state = "running".to_string();
                    JsValue::Undefined
                }
                "suspend" => {
                    ctx.borrow_mut().state = "suspended".to_string();
                    JsValue::Undefined
                }
                _ => JsValue::Undefined,
            },
            HostObject::AudioNode(ctx, node_id) => match prop {
                "connect" => {
                    if let Some(dest_val) = args.first() {
                        if let JsValue::Host(h) = dest_val {
                            if let HostObject::AudioNode(_, dest_id) = h.as_ref() {
                                ctx.borrow_mut().connect(*node_id, *dest_id);
                            }
                        }
                    }
                    JsValue::Undefined
                }
                "disconnect" => {
                    ctx.borrow_mut().disconnect(*node_id);
                    JsValue::Undefined
                }
                "start" => {
                    if let Some(node) = ctx.borrow_mut().get_node_mut(*node_id) {
                        if let crate::audio::AudioNodeKind::Oscillator { ref mut started, .. } = node.kind {
                            *started = true;
                        }
                    }
                    JsValue::Undefined
                }
                "stop" => {
                    if let Some(node) = ctx.borrow_mut().get_node_mut(*node_id) {
                        if let crate::audio::AudioNodeKind::Oscillator { ref mut stopped, .. } = node.kind {
                            *stopped = true;
                        }
                    }
                    JsValue::Undefined
                }
                _ => JsValue::Undefined,
            },
            HostObject::AudioParam(ctx, node_id, param_name) => {
                let c = ctx.borrow();
                let Some(node) = c.get_node(*node_id) else {
                    return JsValue::Undefined;
                };
                let param = match &node.kind {
                    crate::audio::AudioNodeKind::Oscillator { frequency, .. } if param_name == "frequency" => frequency,
                    crate::audio::AudioNodeKind::Gain { gain } if param_name == "gain" => gain,
                    _ => return JsValue::Undefined,
                };
                match prop {
                    "value" => JsValue::Number(param.value),
                    "defaultValue" => JsValue::Number(param.default_value),
                    "minValue" => JsValue::Number(param.min_value),
                    "maxValue" => JsValue::Number(param.max_value),
                    _ => JsValue::Undefined,
                }
            }
            HostObject::IntersectionObserver(data) => match prop {
                "observe" => {
                    let target_id = args.first().map(to_string).unwrap_or_default();
                    let mut d = data.borrow_mut();
                    if !d.targets.iter().any(|t| t.element_id == target_id) {
                        d.targets.push(IntersectionObserverTarget {
                            element_id: target_id,
                            is_intersecting: true,
                            intersection_ratio: 1.0,
                        });
                    }
                    JsValue::Undefined
                }
                "unobserve" => {
                    let target_id = args.first().map(to_string).unwrap_or_default();
                    data.borrow_mut().targets.retain(|t| t.element_id != target_id);
                    JsValue::Undefined
                }
                "disconnect" => {
                    data.borrow_mut().targets.clear();
                    JsValue::Undefined
                }
                "takeRecords" => {
                    let entries: Vec<JsValue> = data.borrow().targets.iter().map(|t| {
                        host_value(HostObject::IntersectionObserverEntry(IntersectionObserverEntryData {
                            target_id: t.element_id.clone(),
                            is_intersecting: t.is_intersecting,
                            intersection_ratio: t.intersection_ratio,
                            bounding_client_rect: [0.0, 0.0, 0.0, 0.0],
                            intersection_rect: [0.0, 0.0, 0.0, 0.0],
                            root_bounds: [0.0, 0.0, 0.0, 0.0],
                        }))
                    }).collect();
                    JsValue::Array(Rc::new(RefCell::new(entries)))
                }
                _ => JsValue::Undefined,
            },
            HostObject::IntersectionObserverEntry(_) => JsValue::Undefined,
            HostObject::ResizeObserver(data) => match prop {
                "observe" => {
                    let target_id = args.first().map(to_string).unwrap_or_default();
                    let mut d = data.borrow_mut();
                    if !d.targets.contains(&target_id) {
                        d.targets.push(target_id);
                    }
                    JsValue::Undefined
                }
                "unobserve" => {
                    let target_id = args.first().map(to_string).unwrap_or_default();
                    data.borrow_mut().targets.retain(|t| t != &target_id);
                    JsValue::Undefined
                }
                "disconnect" => {
                    data.borrow_mut().targets.clear();
                    JsValue::Undefined
                }
                "takeRecords" => {
                    let entries: Vec<JsValue> = data.borrow().targets.iter().map(|target_id| {
                        host_value(HostObject::ResizeObserverEntry(ResizeObserverEntryData {
                            target_id: target_id.clone(),
                            content_rect: [0.0, 0.0, 100.0, 100.0],
                            border_box_size: (100.0, 100.0),
                            content_box_size: (100.0, 100.0),
                        }))
                    }).collect();
                    JsValue::Array(Rc::new(RefCell::new(entries)))
                }
                _ => JsValue::Undefined,
            },
            HostObject::ResizeObserverEntry(_) => JsValue::Undefined,
            HostObject::JsMap(entries) => match prop {
                "get" => {
                    let key = args.first().map(to_string).unwrap_or_default();
                    entries.borrow().iter()
                        .find(|(k, _)| k == &key)
                        .map(|(_, v)| v.clone())
                        .unwrap_or(JsValue::Undefined)
                }
                "set" => {
                    let key = args.first().map(to_string).unwrap_or_default();
                    let val = args.get(1).cloned().unwrap_or(JsValue::Undefined);
                    let mut e = entries.borrow_mut();
                    if let Some(entry) = e.iter_mut().find(|(k, _)| k == &key) {
                        entry.1 = val;
                    } else {
                        e.push((key, val));
                    }
                    host_value(HostObject::JsMap(entries.clone()))
                }
                "has" => {
                    let key = args.first().map(to_string).unwrap_or_default();
                    JsValue::Bool(entries.borrow().iter().any(|(k, _)| k == &key))
                }
                "delete" => {
                    let key = args.first().map(to_string).unwrap_or_default();
                    let mut e = entries.borrow_mut();
                    let len_before = e.len();
                    e.retain(|(k, _)| k != &key);
                    JsValue::Bool(e.len() < len_before)
                }
                "clear" => {
                    entries.borrow_mut().clear();
                    JsValue::Undefined
                }
                "keys" => {
                    let keys: Vec<JsValue> = entries.borrow().iter().map(|(k, _)| JsValue::Str(k.clone())).collect();
                    JsValue::Array(Rc::new(RefCell::new(keys)))
                }
                "values" => {
                    let vals: Vec<JsValue> = entries.borrow().iter().map(|(_, v)| v.clone()).collect();
                    JsValue::Array(Rc::new(RefCell::new(vals)))
                }
                "entries" => {
                    let pairs: Vec<JsValue> = entries.borrow().iter().map(|(k, v)| {
                        JsValue::Array(Rc::new(RefCell::new(vec![JsValue::Str(k.clone()), v.clone()])))
                    }).collect();
                    JsValue::Array(Rc::new(RefCell::new(pairs)))
                }
                _ => JsValue::Undefined,
            },
            HostObject::JsSet(items) => match prop {
                "add" => {
                    let val = args.first().map(to_string).unwrap_or_default();
                    let mut s = items.borrow_mut();
                    if !s.contains(&val) {
                        s.push(val);
                    }
                    host_value(HostObject::JsSet(items.clone()))
                }
                "has" => {
                    let val = args.first().map(to_string).unwrap_or_default();
                    JsValue::Bool(items.borrow().contains(&val))
                }
                "delete" => {
                    let val = args.first().map(to_string).unwrap_or_default();
                    let mut s = items.borrow_mut();
                    let len_before = s.len();
                    s.retain(|v| v != &val);
                    JsValue::Bool(s.len() < len_before)
                }
                "clear" => {
                    items.borrow_mut().clear();
                    JsValue::Undefined
                }
                "keys" | "values" => {
                    let vals: Vec<JsValue> = items.borrow().iter().map(|v| JsValue::Str(v.clone())).collect();
                    JsValue::Array(Rc::new(RefCell::new(vals)))
                }
                "entries" => {
                    let pairs: Vec<JsValue> = items.borrow().iter().map(|v| {
                        JsValue::Array(Rc::new(RefCell::new(vec![JsValue::Str(v.clone()), JsValue::Str(v.clone())])))
                    }).collect();
                    JsValue::Array(Rc::new(RefCell::new(pairs)))
                }
                _ => JsValue::Undefined,
            },
            HostObject::Crypto => match prop {
                "getRandomValues" => {
                    if let Some(arr_val) = args.first() {
                        if let JsValue::Array(items) = arr_val {
                            let mut items_mut = items.borrow_mut();
                            let len = items_mut.len();
                            for i in 0..len {
                                let pseudo = ((i as u32 * 1103515245 + 12345) % 256) as f32;
                                items_mut[i] = JsValue::Number(pseudo);
                            }
                            return arr_val.clone();
                        }
                    }
                    JsValue::Undefined
                }
                "randomUUID" => {
                    let uuid = format!(
                        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
                        0x110ec58a_u32,
                        0xa0f2_u16,
                        0xac4_u16,
                        0x8393_u16,
                        0xc0de00000001_u64
                    );
                    JsValue::Str(uuid)
                }
                _ => JsValue::Undefined,
            },
        }
    }

    fn canvas_context_method(
        &mut self,
        ctx: &Rc<std::cell::RefCell<crate::canvas::CanvasContext2D>>,
        prop: &str,
        args: Vec<JsValue>,
    ) -> JsValue {
        let mut c = ctx.borrow_mut();
        match prop {
            "clearRect" => {
                let x = args.first().map(to_number).unwrap_or(0.0);
                let y = args.get(1).map(to_number).unwrap_or(0.0);
                let w = args.get(2).map(to_number).unwrap_or(0.0);
                let h = args.get(3).map(to_number).unwrap_or(0.0);
                c.clear_rect(x, y, w, h);
                JsValue::Undefined
            }
            "fillRect" => {
                let x = args.first().map(to_number).unwrap_or(0.0);
                let y = args.get(1).map(to_number).unwrap_or(0.0);
                let w = args.get(2).map(to_number).unwrap_or(0.0);
                let h = args.get(3).map(to_number).unwrap_or(0.0);
                c.fill_rect(x, y, w, h);
                JsValue::Undefined
            }
            "strokeRect" => {
                let x = args.first().map(to_number).unwrap_or(0.0);
                let y = args.get(1).map(to_number).unwrap_or(0.0);
                let w = args.get(2).map(to_number).unwrap_or(0.0);
                let h = args.get(3).map(to_number).unwrap_or(0.0);
                c.stroke_rect(x, y, w, h);
                JsValue::Undefined
            }
            "beginPath" => {
                c.begin_path();
                JsValue::Undefined
            }
            "closePath" => {
                c.close_path();
                JsValue::Undefined
            }
            "moveTo" => {
                let x = args.first().map(to_number).unwrap_or(0.0);
                let y = args.get(1).map(to_number).unwrap_or(0.0);
                c.move_to(x, y);
                JsValue::Undefined
            }
            "lineTo" => {
                let x = args.first().map(to_number).unwrap_or(0.0);
                let y = args.get(1).map(to_number).unwrap_or(0.0);
                c.line_to(x, y);
                JsValue::Undefined
            }
            "rect" => {
                let x = args.first().map(to_number).unwrap_or(0.0);
                let y = args.get(1).map(to_number).unwrap_or(0.0);
                let w = args.get(2).map(to_number).unwrap_or(0.0);
                let h = args.get(3).map(to_number).unwrap_or(0.0);
                c.rect(x, y, w, h);
                JsValue::Undefined
            }
            "arc" => {
                let cx = args.first().map(to_number).unwrap_or(0.0);
                let cy = args.get(1).map(to_number).unwrap_or(0.0);
                let radius = args.get(2).map(to_number).unwrap_or(0.0);
                let start_angle = args.get(3).map(to_number).unwrap_or(0.0);
                let end_angle = args.get(4).map(to_number).unwrap_or(0.0);
                let counterclockwise = args.get(5).map(truthy).unwrap_or(false);
                c.arc(cx, cy, radius, start_angle, end_angle, counterclockwise);
                JsValue::Undefined
            }
            "fill" => {
                c.fill();
                JsValue::Undefined
            }
            "stroke" => {
                c.stroke();
                JsValue::Undefined
            }
            "translate" => {
                let dx = args.first().map(to_number).unwrap_or(0.0);
                let dy = args.get(1).map(to_number).unwrap_or(0.0);
                c.translate(dx, dy);
                JsValue::Undefined
            }
            "scale" => {
                let sx = args.first().map(to_number).unwrap_or(1.0);
                let sy = args.get(1).map(to_number).unwrap_or(1.0);
                c.scale(sx, sy);
                JsValue::Undefined
            }
            "rotate" => {
                let angle = args.first().map(to_number).unwrap_or(0.0);
                c.rotate(angle);
                JsValue::Undefined
            }
            "transform" => {
                let a = args.first().map(to_number).unwrap_or(1.0);
                let b = args.get(1).map(to_number).unwrap_or(0.0);
                let c_val = args.get(2).map(to_number).unwrap_or(0.0);
                let d = args.get(3).map(to_number).unwrap_or(1.0);
                let e = args.get(4).map(to_number).unwrap_or(0.0);
                let f = args.get(5).map(to_number).unwrap_or(0.0);
                c.transform_matrix(a, b, c_val, d, e, f);
                JsValue::Undefined
            }
            "setTransform" => {
                let a = args.first().map(to_number).unwrap_or(1.0);
                let b = args.get(1).map(to_number).unwrap_or(0.0);
                let c_val = args.get(2).map(to_number).unwrap_or(0.0);
                let d = args.get(3).map(to_number).unwrap_or(1.0);
                let e = args.get(4).map(to_number).unwrap_or(0.0);
                let f = args.get(5).map(to_number).unwrap_or(0.0);
                c.set_transform(a, b, c_val, d, e, f);
                JsValue::Undefined
            }
            "resetTransform" => {
                c.reset_transform();
                JsValue::Undefined
            }
            "quadraticCurveTo" => {
                let cpx = args.first().map(to_number).unwrap_or(0.0);
                let cpy = args.get(1).map(to_number).unwrap_or(0.0);
                let x = args.get(2).map(to_number).unwrap_or(0.0);
                let y = args.get(3).map(to_number).unwrap_or(0.0);
                c.quadratic_curve_to(cpx, cpy, x, y);
                JsValue::Undefined
            }
            "bezierCurveTo" => {
                let cp1x = args.first().map(to_number).unwrap_or(0.0);
                let cp1y = args.get(1).map(to_number).unwrap_or(0.0);
                let cp2x = args.get(2).map(to_number).unwrap_or(0.0);
                let cp2y = args.get(3).map(to_number).unwrap_or(0.0);
                let x = args.get(4).map(to_number).unwrap_or(0.0);
                let y = args.get(5).map(to_number).unwrap_or(0.0);
                c.bezier_curve_to(cp1x, cp1y, cp2x, cp2y, x, y);
                JsValue::Undefined
            }
            "fillText" => {
                let text = to_string(args.first().unwrap_or(&JsValue::Undefined));
                let x = args.get(1).map(to_number).unwrap_or(0.0);
                let y = args.get(2).map(to_number).unwrap_or(0.0);
                c.fill_text(&text, x, y);
                JsValue::Undefined
            }
            "strokeText" => {
                let text = to_string(args.first().unwrap_or(&JsValue::Undefined));
                let x = args.get(1).map(to_number).unwrap_or(0.0);
                let y = args.get(2).map(to_number).unwrap_or(0.0);
                c.stroke_text(&text, x, y);
                JsValue::Undefined
            }
            "measureText" => {
                let text = to_string(args.first().unwrap_or(&JsValue::Undefined));
                let w = c.measure_text(&text);
                let obj = vec![("width".to_string(), JsValue::Number(w))];
                JsValue::Object(Rc::new(std::cell::RefCell::new(obj)))
            }
            "save" => {
                c.save();
                JsValue::Undefined
            }
            "restore" => {
                c.restore();
                JsValue::Undefined
            }
            "getImageData" => {
                let sx = args.first().map(to_number).unwrap_or(0.0) as i32;
                let sy = args.get(1).map(to_number).unwrap_or(0.0) as i32;
                let sw = (args.get(2).map(to_number).unwrap_or(0.0).max(0.0)) as u32;
                let sh = (args.get(3).map(to_number).unwrap_or(0.0).max(0.0)) as u32;
                let data = c.get_image_data(sx, sy, sw, sh);
                let data_items: Vec<JsValue> = data
                    .into_iter()
                    .map(|b| JsValue::Number(b as f32))
                    .collect();
                let obj = vec![
                    ("width".to_string(), JsValue::Number(sw as f32)),
                    ("height".to_string(), JsValue::Number(sh as f32)),
                    (
                        "data".to_string(),
                        JsValue::Array(Rc::new(std::cell::RefCell::new(data_items))),
                    ),
                ];
                JsValue::Object(Rc::new(std::cell::RefCell::new(obj)))
            }
            "putImageData" => {
                if let Some(JsValue::Object(img_obj)) = args.first() {
                    let sw = object_get(img_obj, "width")
                        .map(|v| to_number(&v) as u32)
                        .unwrap_or(0);
                    let sh = object_get(img_obj, "height")
                        .map(|v| to_number(&v) as u32)
                        .unwrap_or(0);
                    let data = object_get(img_obj, "data")
                        .and_then(|v| match v {
                            JsValue::Array(arr) => Some(
                                arr.borrow()
                                    .iter()
                                    .map(|x| to_number(x) as u8)
                                    .collect::<Vec<u8>>(),
                            ),
                            _ => None,
                        })
                        .unwrap_or_default();
                    let dx = args.get(1).map(to_number).unwrap_or(0.0) as i32;
                    let dy = args.get(2).map(to_number).unwrap_or(0.0) as i32;
                    c.put_image_data(&data, dx, dy, sw, sh);
                }
                JsValue::Undefined
            }
            _ => JsValue::Undefined,
        }
    }

    fn headers_method(&mut self, headers: &HeadersRef, prop: &str, args: &[JsValue]) -> JsValue {
        let name = to_string(args.first().unwrap_or(&JsValue::Undefined));
        let value = to_string(args.get(1).unwrap_or(&JsValue::Undefined));

        match prop {
            "get" => match headers.borrow().get(&name) {
                Some(found) => JsValue::Str(found),
                None => JsValue::Null,
            },
            "has" => JsValue::Bool(headers.borrow().has(&name)),
            "set" | "append" => {
                if HeaderMap::is_forbidden(&name) {
                    return JsValue::Undefined;
                }
                let outcome = if prop == "set" {
                    headers.borrow_mut().set(&name, &value)
                } else {
                    headers.borrow_mut().append(&name, &value)
                };
                if let Err(error) = outcome {
                    self.throw_type_error(error.to_string());
                }
                JsValue::Undefined
            }
            "delete" => {
                headers.borrow_mut().delete(&name);
                JsValue::Undefined
            }
            "keys" => {
                let names: Vec<JsValue> = headers
                    .borrow()
                    .names()
                    .into_iter()
                    .map(JsValue::Str)
                    .collect();
                JsValue::Array(Rc::new(std::cell::RefCell::new(names)))
            }
            "entries" => {
                let entries: Vec<JsValue> = headers
                    .borrow()
                    .iter()
                    .map(|(name, value)| {
                        JsValue::Array(Rc::new(std::cell::RefCell::new(vec![
                            JsValue::Str(name.to_string()),
                            JsValue::Str(value.to_string()),
                        ])))
                    })
                    .collect();
                JsValue::Array(Rc::new(std::cell::RefCell::new(entries)))
            }
            _ => JsValue::Undefined,
        }
    }

    fn consume_body(&mut self, body: &Body, as_json: bool) -> JsValue {
        let promise = promise::new_promise();
        match body.take() {
            Err(message) => self.settle_reject(&promise, JsValue::Str(message)),
            Ok(bytes) => {
                let text = decode_text(&bytes);
                if as_json {
                    match json::parse(&text) {
                        Ok(value) => self.settle_resolve(&promise, value),
                        Err(message) => self.settle_reject(&promise, JsValue::Str(message)),
                    }
                } else {
                    self.settle_resolve(&promise, JsValue::Str(text));
                }
            }
        }
        JsValue::Promise(promise)
    }

    pub(crate) fn json_method(&mut self, prop: &str, args: &[JsValue]) -> JsValue {
        match prop {
            "stringify" => {
                JsValue::Str(json::stringify(args.first().unwrap_or(&JsValue::Undefined)))
            }
            "parse" => {
                let text = to_string(args.first().unwrap_or(&JsValue::Undefined));
                match json::parse(&text) {
                    Ok(value) => value,
                    Err(message) => {
                        self.throw_value(JsValue::Str(message));
                        JsValue::Undefined
                    }
                }
            }
            _ => JsValue::Undefined,
        }
    }
}

fn host_value(object: HostObject) -> JsValue {
    JsValue::Host(Rc::new(object))
}

fn check_mode(mode: &str) -> Result<RequestMode, FetchError> {
    match mode {
        "cors" | "" => Ok(RequestMode::Cors),
        "same-origin" => Ok(RequestMode::SameOrigin),
        other => Err(FetchError::BadRequest(format!(
            "unsupported fetch mode {other:?}: this engine supports cors and same-origin"
        ))),
    }
}

fn check_credentials(credentials: &str) -> Result<RequestCredentials, FetchError> {
    match credentials {
        "same-origin" | "" => Ok(RequestCredentials::SameOrigin),
        "omit" => Ok(RequestCredentials::Omit),
        "include" => Ok(RequestCredentials::Include),
        other => Err(FetchError::BadRequest(format!(
            "unsupported credentials mode {other:?}: this engine supports same-origin, omit, and include"
        ))),
    }
}

fn conservative_same_site(source: &Url, target: &Url) -> bool {
    source.scheme() == target.scheme() && source.host().eq_ignore_ascii_case(target.host())
}

fn is_cors_safelisted_method(method: Method) -> bool {
    matches!(method, Method::Get | Method::Head | Method::Post)
}

fn contains_cors_unsafe_request_header_byte(value: &str) -> bool {
    value.bytes().any(|byte| {
        (byte < 0x20 && byte != b'\t')
            || byte == 0x7f
            || b"\"():<>?@[\\]{}".contains(&byte)
    })
}

fn is_cors_safelisted_request_header(name: &str, value: &str) -> bool {
    if value.len() > 128 {
        return false;
    }
    match name {
        "accept" => !contains_cors_unsafe_request_header_byte(value),
        "accept-language" | "content-language" => value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || b" *,-.;=".contains(&byte)
        }),
        "content-type" => {
            if contains_cors_unsafe_request_header_byte(value) {
                return false;
            }
            let mime = value
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            matches!(
                mime.as_str(),
                "application/x-www-form-urlencoded" | "multipart/form-data" | "text/plain"
            )
        }
        _ => false,
    }
}

fn cors_unsafe_request_header_names(request: &RequestData) -> Vec<String> {
    let mut names = Vec::new();
    for (name, value) in request.headers.borrow().iter() {
        if name != "origin" && !is_cors_safelisted_request_header(name, value) {
            names.push(name.to_string());
        }
    }
    names.sort();
    names.dedup();
    names
}

fn build_cors_preflight_request(request: &RequestData, cors: &CorsFetchState) -> FetchRequest {
    let mut headers = HeaderMap::new();
    headers.insert_raw("origin", &cors.source_origin.header_value());
    headers.insert_raw(
        "access-control-request-method",
        cors.requested_method.as_str(),
    );
    if !cors.requested_headers.is_empty() {
        headers.insert_raw(
            "access-control-request-headers",
            &cors.requested_headers.join(", "),
        );
    }
    FetchRequest::new(request.url.clone(), Method::Options, headers, None)
}

fn cors_preflight_cookie_policy() -> CookieRequestPolicy {
    CookieRequestPolicy::new(
        CookieCredentials::Omit,
        SameSiteRequestContext::cross_site_subresource(Method::Options),
    )
}

const CORS_SAFELISTED_RESPONSE_HEADERS: &[&str] = &[
    "cache-control",
    "content-language",
    "content-length",
    "content-type",
    "expires",
    "last-modified",
    "pragma",
];

fn is_forbidden_response_header_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("set-cookie") || name.eq_ignore_ascii_case("set-cookie2")
}

/// Apply the Fetch CORS response-header-name filter before a cross-origin
/// response becomes script-visible. The wire response remains available to
/// lower networking layers for policy processing; only the Response wrapper is
/// narrowed here.
fn filter_cors_response_headers(response: &mut FetchResponse, credentialed: bool) {
    let exposed = comma_tokens(response.headers.get("access-control-expose-headers"));
    let wildcard = !credentialed && exposed.iter().any(|name| name == "*");

    for name in response.headers.names() {
        let safelisted = CORS_SAFELISTED_RESPONSE_HEADERS
            .iter()
            .any(|allowed| name.eq_ignore_ascii_case(allowed));
        let explicitly_exposed = exposed
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(&name));
        if is_forbidden_response_header_name(&name)
            || (!wildcard && !safelisted && !explicitly_exposed)
        {
            response.headers.delete(&name);
        }
    }
}

fn validate_cors_response(
    source_origin: &Origin,
    credentialed: bool,
    response: &FetchResponse,
) -> Result<(), FetchError> {
    let allow_origin = response.headers.get("access-control-allow-origin");
    let serialized = source_origin.header_value();

    if credentialed {
        if allow_origin.as_deref() != Some(serialized.as_str()) {
            return Err(FetchError::Blocked(
                "CORS: credentialed response requires an exact Access-Control-Allow-Origin value"
                    .into(),
            ));
        }
        if response
            .headers
            .get("access-control-allow-credentials")
            .as_deref()
            != Some("true")
        {
            return Err(FetchError::Blocked(
                "CORS: credentialed response requires Access-Control-Allow-Credentials: true"
                    .into(),
            ));
        }
        return Ok(());
    }

    if matches!(allow_origin.as_deref(), Some("*"))
        || allow_origin.as_deref() == Some(serialized.as_str())
    {
        Ok(())
    } else {
        Err(FetchError::Blocked(
            "CORS: cross-origin response did not allow the document origin".into(),
        ))
    }
}

fn cors_preflight_max_age_seconds(response: &FetchResponse) -> u64 {
    response
        .headers
        .get("access-control-max-age")
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_CORS_PREFLIGHT_MAX_AGE_SECS)
        .min(MAX_CORS_PREFLIGHT_MAX_AGE_SECS)
}

fn is_cors_non_wildcard_request_header_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("authorization")
}

fn comma_tokens(value: Option<String>) -> Vec<String> {
    value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect()
}

fn validate_cors_preflight_response(
    cors: &CorsFetchState,
    response: &FetchResponse,
) -> Result<(), FetchError> {
    if response.redirected {
        return Err(FetchError::Blocked(
            "CORS: redirected preflight responses are not supported".into(),
        ));
    }
    if !(200..300).contains(&response.status) {
        return Err(FetchError::Blocked(format!(
            "CORS: preflight response status {} is not successful",
            response.status
        )));
    }
    validate_cors_response(&cors.source_origin, cors.credentialed, response)?;

    if !is_cors_safelisted_method(cors.requested_method) {
        let methods = comma_tokens(response.headers.get("access-control-allow-methods"));
        let wildcard = !cors.credentialed && methods.iter().any(|method| method == "*");
        let exact = methods
            .iter()
            .any(|method| method == cors.requested_method.as_str());
        if !wildcard && !exact {
            return Err(FetchError::Blocked(format!(
                "CORS: preflight did not allow method {}",
                cors.requested_method
            )));
        }
    }

    if !cors.requested_headers.is_empty() {
        let allowed = comma_tokens(response.headers.get("access-control-allow-headers"));
        let wildcard = !cors.credentialed && allowed.iter().any(|header| header == "*");
        for requested in &cors.requested_headers {
            let wildcard_allows = wildcard && !is_cors_non_wildcard_request_header_name(requested);
            if !wildcard_allows
                && !allowed
                    .iter()
                    .any(|header| header.eq_ignore_ascii_case(requested))
            {
                return Err(FetchError::Blocked(format!(
                    "CORS: preflight did not allow request header {requested}"
                )));
            }
        }
    }

    Ok(())
}
