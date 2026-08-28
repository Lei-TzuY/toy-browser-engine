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
//  task. That is what makes
//
//      console.log("A"); fetch(u).then(() => console.log("C")); console.log("B");
//
//  print A, B, C however fast the resource is, including one already sitting
//  in memory.

use std::rc::Rc;

use crate::net::fetch::{FetchError, FetchResponse, HeaderMap, Method};
use crate::net::Url;
use crate::security_origin::SecurityOrigin;

use super::host::{
    decode_text, headers_ref, AbortState, Body, HeadersRef, HostObject, RequestData, ResponseData,
};
use super::interp::{to_string, Builtin, JsRuntime, JsValue};
use super::json;
use super::promise::{self, PromiseRef};

/// The schemes a page may fetch, on top of its own.
///
/// `data:` is self-contained and does not consult another origin, so it is
/// explicitly allowed. The allowlist remains closed for `javascript:`,
/// `about:` and schemes the engine has not deliberately implemented.
const FETCHABLE_SCHEMES: &[&str] = &["http", "https", "file", "data"];

/// What the runtime keeps for one request it is waiting on.
///
/// The promise is here and nowhere else, which is the whole navigation story:
/// dropping the document drops the registry, drops this, and drops the
/// promise, so a completion for the previous page can never settle anything.
#[derive(Debug)]
pub struct PendingFetch {
    pub promise: PromiseRef,
    /// The signal watching this request, if `fetch` was given one.
    pub signal: Option<Rc<AbortState>>,
}

impl JsRuntime {
    // ── fetch() ───────────────────────────────────────────────────────────

    /// `fetch(input, init)` — returns a pending promise, always.
    ///
    /// Every failure path rejects that promise rather than throwing, so a bad
    /// URL or a blocked origin reaches `.catch()` like any other network
    /// problem instead of unwinding through the caller.
    pub fn start_fetch(&mut self, args: Vec<JsValue>) -> JsValue {
        let promise = promise::new_promise();

        match self.prepare_request(&args) {
            Err(error) => self.reject_with(&promise, &error),
            Ok(request) => {
                let signal = request.signal.clone();
                if signal.as_ref().is_some_and(|state| state.aborted()) {
                    // Already aborted before it began.
                    self.reject_with(&promise, &FetchError::Aborted);
                } else {
                    let pending = PendingFetch {
                        promise: promise.clone(),
                        signal,
                    };
                    if let Err(error) = self.fetches.start(request.to_wire(), pending) {
                        self.reject_with(&promise, &error);
                    }
                }
            }
        }
        JsValue::Promise(promise)
    }

    /// Settle the promise of a request the network has finished with.
    ///
    /// Called from the document's network phase — a task — so the reactions it
    /// releases run at the checkpoint that follows, never inline.
    pub fn settle_fetch(
        &mut self,
        pending: PendingFetch,
        result: Result<FetchResponse, FetchError>,
    ) {
        // An abort raised while the answer was in flight wins over the answer.
        if pending.signal.as_ref().is_some_and(|state| state.aborted()) {
            self.reject_with(&pending.promise, &FetchError::Aborted);
            return;
        }
        match result {
            Ok(response) => {
                let value = host_value(HostObject::Response(ResponseData::from_wire(response)));
                self.settle_resolve(&pending.promise, value);
            }
            Err(error) => self.reject_with(&pending.promise, &error),
        }
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

    /// Turn `(input, init)` into a request, or explain why it cannot be one.
    fn prepare_request(&mut self, args: &[JsValue]) -> Result<RequestData, FetchError> {
        let input = args.first().cloned().unwrap_or(JsValue::Undefined);
        let init = args.get(1).cloned().unwrap_or(JsValue::Undefined);
        let request = self.build_request(input, init)?;

        // Policy, applied once, on the URL that will actually be requested.
        let scheme = request.url.scheme();
        if !FETCHABLE_SCHEMES.contains(&scheme) && scheme != self.url.scheme() {
            return Err(FetchError::UnsupportedScheme(scheme.to_string()));
        }
        // A data URL is a self-contained resource rather than a request to a
        // different origin. All other supported schemes use the explicit
        // security-origin model so opaque URLs never acquire local privileges.
        let origin = SecurityOrigin::of(&self.url);
        if scheme != "data" && !origin.can_fetch(&request.url) {
            return Err(FetchError::Blocked(format!(
                "{} may not fetch {}",
                origin.header_value(),
                request.url
            )));
        }
        Ok(request)
    }

    /// The `Request` constructor, shared with `fetch`'s first argument.
    fn build_request(&mut self, input: JsValue, init: JsValue) -> Result<RequestData, FetchError> {
        // A Request as input supplies the defaults; a string supplies only a URL.
        let (mut url, mut method, mut headers, mut body, mut signal) = match &input {
            JsValue::Host(host) => match host.as_request() {
                Some(existing) => (
                    existing.url.clone(),
                    existing.method,
                    existing.headers.borrow().clone(),
                    existing.body.peek(),
                    existing.signal.clone(),
                ),
                None => {
                    return Err(FetchError::InvalidUrl(to_string(&input)));
                }
            },
            other => (
                self.resolve_fetch_url(&to_string(other))?,
                Method::Get,
                HeaderMap::new(),
                None,
                None,
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
                    // Accepted, with the subset this engine actually enforces.
                    "mode" => check_mode(&to_string(value))?,
                    "credentials" => check_credentials(&to_string(value))?,
                    // `url` on an init object is not a thing; anything else is
                    // ignored the way an unknown init member is in Fetch.
                    _ => {}
                }
            }
        }

        if body.is_some() && !method.allows_body() {
            return Err(FetchError::BadRequest(format!(
                "a {method} request cannot have a body"
            )));
        }
        // A Request as input may still be re-pointed by a string second form;
        // keep the URL absolute either way.
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
        })
    }

    /// Resolve a fetch URL against the document, as a relative reference.
    fn resolve_fetch_url(&self, reference: &str) -> Result<Url, FetchError> {
        let trimmed = reference.trim();
        if trimmed.is_empty() {
            return Err(FetchError::InvalidUrl("(empty)".into()));
        }
        self.url
            .join(trimmed)
            .map_err(|_| FetchError::InvalidUrl(trimmed.to_string()))
    }

    /// Read a `headers` init member: a plain object or a `Headers`.
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

    /// `new Headers(...)`, `new Request(...)`, `new Response(...)`,
    /// `new AbortController()`.
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
                        // A constructor is not a promise: this one throws.
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
                };
                host_value(HostObject::Response(response))
            }
            Builtin::AbortControllerCtor => {
                host_value(HostObject::AbortController(AbortState::new()))
            }
            other => {
                self.throw_type_error(format!("{other:?} is not a constructor"));
                JsValue::Undefined
            }
        }
    }

    // ── Properties ────────────────────────────────────────────────────────

    /// Read a property of a Web-platform object.
    pub(crate) fn host_member(&mut self, host: &Rc<HostObject>, prop: &str) -> JsValue {
        match host.as_ref() {
            HostObject::Headers(_) => JsValue::Undefined,
            HostObject::Request(request) => match prop {
                "url" => JsValue::Str(request.url.to_string()),
                "method" => JsValue::Str(request.method.as_str().to_string()),
                "headers" => host_value(HostObject::Headers(request.headers.clone())),
                "bodyUsed" => JsValue::Bool(request.body.used()),
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
                // Only one response type exists here: there is no opaque
                // cross-origin mode to report.
                "type" => JsValue::Str("basic".to_string()),
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
        }
    }

    // ── Methods ───────────────────────────────────────────────────────────

    /// Call a method of a Web-platform object.
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
        }
    }

    fn headers_method(&mut self, headers: &HeadersRef, prop: &str, args: &[JsValue]) -> JsValue {
        let name = to_string(args.first().unwrap_or(&JsValue::Undefined));
        let value = to_string(args.get(1).unwrap_or(&JsValue::Undefined));

        match prop {
            "get" => match headers.borrow().get(&name) {
                Some(found) => JsValue::Str(found),
                // Fetch returns null, not undefined, for a header that is absent.
                None => JsValue::Null,
            },
            "has" => JsValue::Bool(headers.borrow().has(&name)),
            "set" | "append" => {
                if HeaderMap::is_forbidden(&name) {
                    // Silently ignored, as Fetch specifies for forbidden names.
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

    /// `response.text()` / `response.json()`, and the same on a `Request`.
    ///
    /// The bytes are already in memory, but the answer is still a promise and
    /// its handlers still run as microtasks — reading a body is never
    /// synchronous, however local it is.
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

    // ── JSON ──────────────────────────────────────────────────────────────

    /// `JSON.parse` / `JSON.stringify`.
    ///
    /// Parsing throws a `SyntaxError` on bad input the way JavaScript does,
    /// rather than quietly producing `undefined`.
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

/// Wrap a host object as a value.
fn host_value(object: HostObject) -> JsValue {
    JsValue::Host(Rc::new(object))
}

/// `mode`: the engine only does same-origin, so say so rather than pretend.
fn check_mode(mode: &str) -> Result<(), FetchError> {
    match mode {
        // Both are accepted, and both are enforced as same-origin, because
        // there is no CORS preflight here to make `cors` mean more.
        "cors" | "same-origin" | "" => Ok(()),
        other => Err(FetchError::BadRequest(format!(
            "unsupported fetch mode {other:?}: this engine only does same-origin requests"
        ))),
    }
}

/// `credentials`: there is no cookie jar, so anything that needs one fails.
fn check_credentials(credentials: &str) -> Result<(), FetchError> {
    match credentials {
        "same-origin" | "omit" | "" => Ok(()),
        other => Err(FetchError::BadRequest(format!(
            "unsupported credentials mode {other:?}: this engine sends no cookies or auth"
        ))),
    }
}
