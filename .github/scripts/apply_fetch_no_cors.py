from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    if old not in text:
        raise SystemExit(f"missing patch point: {label}")
    return text.replace(old, new, 1)


host = Path("src/script/host.rs")
text = host.read_text()
text = replace_once(
    text,
    '''#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RequestMode {
    #[default]
    Cors,
    SameOrigin,
}

impl RequestMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            RequestMode::Cors => "cors",
            RequestMode::SameOrigin => "same-origin",
        }
    }
}''',
    '''#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RequestMode {
    #[default]
    Cors,
    SameOrigin,
    NoCors,
}

impl RequestMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            RequestMode::Cors => "cors",
            RequestMode::SameOrigin => "same-origin",
            RequestMode::NoCors => "no-cors",
        }
    }
}''',
    "RequestMode no-cors",
)
text = replace_once(
    text,
    '''/// Script-visible Fetch response type.
///
/// `basic` is used for same-origin and synthetic responses. A successful
/// cross-origin CORS fetch is tagged `cors` after its response gate succeeds.
/// Opaque response types can be added here when `no-cors` is implemented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResponseType {
    #[default]
    Basic,
    Cors,
}

impl ResponseType {
    pub const fn as_str(self) -> &'static str {
        match self {
            ResponseType::Basic => "basic",
            ResponseType::Cors => "cors",
        }
    }
}''',
    '''/// Script-visible Fetch response type.
///
/// Synthetic `new Response()` values are `default`, ordinary same-origin
/// network responses are `basic`, successful CORS responses are `cors`, and
/// cross-origin `no-cors` responses are exposed only through an `opaque`
/// filtered view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResponseType {
    #[default]
    Default,
    Basic,
    Cors,
    Opaque,
}

impl ResponseType {
    pub const fn as_str(self) -> &'static str {
        match self {
            ResponseType::Default => "default",
            ResponseType::Basic => "basic",
            ResponseType::Cors => "cors",
            ResponseType::Opaque => "opaque",
        }
    }
}''',
    "ResponseType variants",
)
text = replace_once(
    text,
    '''    /// `response.ok` — the 2xx range, and nothing else.
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}''',
    '''    /// Build the script-visible opaque filtered view of a successful
    /// cross-origin no-CORS response. Cookie/HSTS processing has already seen
    /// the internal wire response before this wrapper is constructed.
    pub fn opaque_from_wire(response: FetchResponse) -> ResponseData {
        ResponseData {
            // Keep the internal URL only as non-script-visible bookkeeping. The
            // `url` getter below suppresses it for opaque responses.
            url: response.url,
            status: 0,
            status_text: String::new(),
            headers: headers_ref(HeaderMap::new()),
            body: Body::empty(),
            redirected: false,
            response_type: ResponseType::Opaque,
        }
    }

    pub fn is_opaque(&self) -> bool {
        self.response_type == ResponseType::Opaque
    }

    pub fn script_url(&self) -> String {
        if self.is_opaque() {
            String::new()
        } else {
            self.url.to_string()
        }
    }

    pub fn body_used(&self) -> bool {
        !self.is_opaque() && self.body.used()
    }

    /// `response.ok` — the 2xx range, and nothing else.
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}''',
    "opaque response helpers",
)
host.write_text(text)

fetch = Path("src/script/fetch_api.rs")
text = fetch.read_text()
text = replace_once(
    text,
    '''    Actual {
        cors: Option<CorsFetchState>,
    },''',
    '''    Actual {
        cors: Option<CorsFetchState>,
        opaque: bool,
    },''',
    "PendingFetchStage opaque flag",
)
text = replace_once(
    text,
    '''            Ok((request, cookie_policy, cors)) => {''',
    '''            Ok((request, cookie_policy, cors, opaque)) => {''',
    "prepare_request tuple",
)
text = replace_once(
    text,
    '''                            stage: PendingFetchStage::Actual { cors },''',
    '''                            stage: PendingFetchStage::Actual { cors, opaque },''',
    "direct actual stage",
)
text = replace_once(
    text,
    '''            PendingFetchStage::Actual { cors } => match result {''',
    '''            PendingFetchStage::Actual { cors, opaque } => match result {''',
    "settle actual stage",
)
text = replace_once(
    text,
    '''                    let mut response_data = ResponseData::from_wire(response);
                    if cors.is_some() {
                        response_data.response_type = ResponseType::Cors;
                    }
                    let value = host_value(HostObject::Response(response_data));''',
    '''                    let mut response_data = if opaque {
                        debug_assert!(cors.is_none(), "opaque no-CORS responses are not CORS responses");
                        ResponseData::opaque_from_wire(response)
                    } else {
                        ResponseData::from_wire(response)
                    };
                    if cors.is_some() {
                        response_data.response_type = ResponseType::Cors;
                    }
                    let value = host_value(HostObject::Response(response_data));''',
    "opaque response wrapping",
)
text = replace_once(
    text,
    '''                    stage: PendingFetchStage::Actual { cors: Some(cors) },''',
    '''                    stage: PendingFetchStage::Actual {
                        cors: Some(cors),
                        opaque: false,
                    },''',
    "preflight actual stage",
)
text = replace_once(
    text,
    '''    ) -> Result<(RequestData, CookieRequestPolicy, Option<CorsFetchState>), FetchError> {''',
    '''    ) -> Result<
        (RequestData, CookieRequestPolicy, Option<CorsFetchState>, bool),
        FetchError,
    > {''',
    "prepare_request return type",
)
text = replace_once(
    text,
    '''        let source_origin = Origin::of(&self.url);
        let same_origin = source_origin.can_fetch(&request.url);
        let cross_origin_web = matches!(self.url.scheme(), "http" | "https")
            && matches!(request.url.scheme(), "http" | "https")
            && !same_origin;

        let cors = match mode {''',
    '''        let source_origin = Origin::of(&self.url);
        let same_origin = source_origin.can_fetch(&request.url);
        let cross_origin_web = matches!(self.url.scheme(), "http" | "https")
            && matches!(request.url.scheme(), "http" | "https")
            && !same_origin;

        if mode == RequestMode::NoCors {
            if !is_cors_safelisted_method(request.method) {
                return Err(FetchError::BadRequest(format!(
                    "no-cors mode only supports CORS-safelisted methods, not {}",
                    request.method
                )));
            }

            // A request-no-cors header guard only permits CORS-safelisted
            // request headers. This engine does not attach guard metadata to
            // Headers objects yet, so enforce the equivalent wire invariant at
            // fetch preparation time by dropping unsafe authored fields.
            {
                let mut headers = request.headers.borrow_mut();
                headers.delete("origin");
                headers.delete("access-control-request-method");
                headers.delete("access-control-request-headers");
            }
            let unsafe_names = cors_unsafe_request_header_names(&request);
            if !unsafe_names.is_empty() {
                let mut headers = request.headers.borrow_mut();
                for name in unsafe_names {
                    headers.delete(&name);
                }
            }
        }

        let mut opaque = false;
        let cors = match mode {''',
    "no-cors request guard",
)
text = replace_once(
    text,
    '''            RequestMode::Cors if !same_origin => {
                // CORS is only meaningful for network tuple origins here. Keep
                // the existing local-file containment boundary intact.
                return Err(FetchError::Blocked(format!(
                    "{} may not fetch {}",
                    source_origin.header_value(),
                    request.url
                )))
            }
            _ => None,
        };''',
    '''            RequestMode::Cors if !same_origin => {
                // CORS is only meaningful for network tuple origins here. Keep
                // the existing local-file containment boundary intact.
                return Err(FetchError::Blocked(format!(
                    "{} may not fetch {}",
                    source_origin.header_value(),
                    request.url
                )))
            }
            RequestMode::NoCors if cross_origin_web => {
                // no-cors sends the constrained request without a CORS
                // handshake. The internal response may still update browser
                // policy/cookies, but script receives only an opaque filter.
                opaque = true;
                None
            }
            RequestMode::NoCors if !same_origin => {
                // Preserve the engine's file/local containment boundary; this
                // no-cors implementation is intentionally HTTP(S)-only across
                // origins.
                return Err(FetchError::Blocked(format!(
                    "{} may not fetch {} in no-cors mode",
                    source_origin.header_value(),
                    request.url
                )))
            }
            _ => None,
        };''',
    "no-cors response tainting",
)
text = replace_once(
    text,
    '''        Ok((request, cookie_policy, cors))''',
    '''        Ok((request, cookie_policy, cors, opaque))''',
    "prepare_request return tuple",
)
text = replace_once(
    text,
    '''                    response_type: ResponseType::Basic,
                };
                host_value(HostObject::Response(response))''',
    '''                    response_type: ResponseType::Default,
                };
                host_value(HostObject::Response(response))''',
    "constructed Response default type",
)
text = replace_once(
    text,
    '''            HostObject::Response(response) => match prop {
                "status" => JsValue::Number(response.status as f32),
                "statusText" => JsValue::Str(response.status_text.clone()),
                "ok" => JsValue::Bool(response.ok()),
                "url" => JsValue::Str(response.url.to_string()),
                "redirected" => JsValue::Bool(response.redirected),
                "headers" => host_value(HostObject::Headers(response.headers.clone())),
                "bodyUsed" => JsValue::Bool(response.body.used()),
                "type" => JsValue::Str(response.response_type.as_str().to_string()),
                _ => JsValue::Undefined,
            },''',
    '''            HostObject::Response(response) => match prop {
                "status" => JsValue::Number(response.status as f32),
                "statusText" => JsValue::Str(response.status_text.clone()),
                "ok" => JsValue::Bool(response.ok()),
                "url" => JsValue::Str(response.script_url()),
                "redirected" => JsValue::Bool(response.redirected),
                "headers" => host_value(HostObject::Headers(response.headers.clone())),
                "bodyUsed" => JsValue::Bool(response.body_used()),
                "type" => JsValue::Str(response.response_type.as_str().to_string()),
                _ => JsValue::Undefined,
            },''',
    "Response getters opaque filter",
)
text = replace_once(
    text,
    '''            HostObject::Response(response) => match prop {
                "text" => self.consume_body(&response.body, false),
                "json" => self.consume_body(&response.body, true),
                _ => JsValue::Undefined,
            },''',
    '''            HostObject::Response(response) => match prop {
                "text" if response.is_opaque() => self.consume_null_body(false),
                "json" if response.is_opaque() => self.consume_null_body(true),
                "text" => self.consume_body(&response.body, false),
                "json" => self.consume_body(&response.body, true),
                _ => JsValue::Undefined,
            },''',
    "Response body opaque filter",
)
text = replace_once(
    text,
    '''    fn consume_body(&mut self, body: &Body, as_json: bool) -> JsValue {''',
    '''    fn consume_null_body(&mut self, as_json: bool) -> JsValue {
        let promise = promise::new_promise();
        if as_json {
            match json::parse("") {
                Ok(value) => self.settle_resolve(&promise, value),
                Err(message) => self.settle_reject(&promise, JsValue::Str(message)),
            }
        } else {
            self.settle_resolve(&promise, JsValue::Str(String::new()));
        }
        JsValue::Promise(promise)
    }

    fn consume_body(&mut self, body: &Body, as_json: bool) -> JsValue {''',
    "null body consumption",
)
text = replace_once(
    text,
    '''    match mode {
        "cors" | "" => Ok(RequestMode::Cors),
        "same-origin" => Ok(RequestMode::SameOrigin),
        other => Err(FetchError::BadRequest(format!(
            "unsupported fetch mode {other:?}: this engine supports cors and same-origin"
        ))),
    }''',
    '''    match mode {
        "cors" | "" => Ok(RequestMode::Cors),
        "same-origin" => Ok(RequestMode::SameOrigin),
        "no-cors" => Ok(RequestMode::NoCors),
        other => Err(FetchError::BadRequest(format!(
            "unsupported fetch mode {other:?}: this engine supports cors, same-origin, and no-cors"
        ))),
    }''',
    "check_mode no-cors",
)
fetch.write_text(text)

response_type_test = Path("tests/fetch_response_type.rs")
text = response_type_test.read_text()
text = replace_once(
    text,
    '''fn constructed_response_type_is_basic() {''',
    '''fn constructed_response_type_is_default() {''',
    "constructed response test name",
)
text = replace_once(
    text,
    '''            assert_eq!(browser.document().runtime.console, vec!["basic"]);''',
    '''            assert_eq!(browser.document().runtime.console, vec!["default"]);''',
    "constructed response expected type",
)
response_type_test.write_text(text)

Path("tests/fetch_no_cors.rs").write_text(r'''use std::rc::Rc;

use browser_engine::browser::Browser;
use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchResponse, ManualNetwork, MemoryLoader, Method, Url};

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

fn response(endpoint: &str) -> FetchResponse {
    let mut response = FetchResponse::synthetic(
        url(endpoint),
        201,
        Some("text/plain"),
        b"secret body".to_vec(),
    );
    response.headers.append_raw("x-secret", "classified");
    response
}

fn browser_for(page: &str, script: &str, endpoint: &str, response: FetchResponse) -> (Browser, Rc<ManualNetwork>) {
    let mut loader = MemoryLoader::new();
    loader.insert(page, format!("<script>{script}</script>"));
    let transport = Rc::new(ManualNetwork::new());
    transport.respond(endpoint, response);
    let browser = Browser::open_with_network(
        Box::new(loader),
        transport.clone(),
        &url(page),
        Rc::new(ManualClock::new()),
    )
    .expect("page loads");
    (browser, transport)
}

#[test]
fn no_cors_mode_is_visible_cloned_and_produces_an_opaque_response() {
    let page = "http://page.test/index.html";
    let endpoint = "http://api.test/data";
    let (mut browser, transport) = browser_for(
        page,
        r#"
            var original = new Request("http://api.test/data", { mode: "no-cors" });
            var clone = new Request(original);
            console.log(original.mode);
            console.log(clone.mode);
            fetch(clone).then(function (response) { console.log(response.type); });
        "#,
        endpoint,
        response(endpoint),
    );

    assert_eq!(browser.document().runtime.console, vec!["no-cors", "no-cors"]);
    assert_eq!(browser.tick().requests_sent, 1);
    let sent = transport.requests();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].headers.get("origin").is_none());

    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    assert_eq!(
        browser.document().runtime.console,
        vec!["no-cors", "no-cors", "opaque"]
    );
}

#[test]
fn opaque_response_hides_status_url_headers_and_body() {
    let page = "http://page.test/index.html";
    let endpoint = "http://api.test/data";
    let (mut browser, transport) = browser_for(
        page,
        r#"
            fetch("http://api.test/data", { mode: "no-cors" }).then(function (response) {
                console.log(response.type === "opaque" ? "type-ok" : "type-leak");
                console.log(response.status === 0 ? "status-ok" : "status-leak");
                console.log(response.statusText === "" ? "status-text-ok" : "status-text-leak");
                console.log(response.ok === false ? "ok-filtered" : "ok-leak");
                console.log(response.url === "" ? "url-ok" : "url-leak");
                console.log(response.redirected === false ? "redirect-ok" : "redirect-leak");
                console.log(response.headers.get("x-secret") === null ? "headers-ok" : "headers-leak");
                console.log(response.bodyUsed === false ? "body-unused" : "body-used");
                return response.text().then(function (text) {
                    console.log(text === "" ? "body-ok" : "body-leak");
                    console.log(response.bodyUsed === false ? "body-still-unused" : "body-used-after-read");
                });
            });
        "#,
        endpoint,
        response(endpoint),
    );

    assert_eq!(browser.tick().requests_sent, 1);
    assert_eq!(transport.complete_all(), 1);
    browser.tick();

    assert_eq!(
        browser.document().runtime.console,
        vec![
            "type-ok",
            "status-ok",
            "status-text-ok",
            "ok-filtered",
            "url-ok",
            "redirect-ok",
            "headers-ok",
            "body-unused",
            "body-ok",
            "body-still-unused",
        ]
    );
}

#[test]
fn no_cors_drops_unsafe_headers_without_preflighting() {
    let page = "http://page.test/index.html";
    let endpoint = "http://api.test/data";
    let (mut browser, transport) = browser_for(
        page,
        r#"
            fetch("http://api.test/data", {
                mode: "no-cors",
                method: "POST",
                headers: {
                    "X-Token": "secret",
                    "Content-Type": "application/json"
                },
                body: "payload"
            }).then(function (response) { console.log(response.type); });
        "#,
        endpoint,
        response(endpoint),
    );

    assert_eq!(browser.tick().requests_sent, 1);
    let sent = transport.requests();
    assert_eq!(sent.len(), 1, "no OPTIONS preflight is created");
    assert_eq!(sent[0].method, Method::Post);
    assert!(sent[0].headers.get("x-token").is_none());
    assert!(sent[0].headers.get("content-type").is_none());
    assert!(sent[0].headers.get("origin").is_none());

    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    assert_eq!(browser.document().runtime.console, vec!["opaque"]);
}

#[test]
fn no_cors_rejects_a_non_safelisted_method_before_transport() {
    let page = "http://page.test/index.html";
    let endpoint = "http://api.test/data";
    let (browser, transport) = browser_for(
        page,
        r#"
            fetch("http://api.test/data", { mode: "no-cors", method: "PUT" })
                .then(function () { console.log("unexpected"); })
                .catch(function () { console.log("blocked"); });
        "#,
        endpoint,
        response(endpoint),
    );

    assert!(transport.requests().is_empty());
    assert_eq!(browser.document().runtime.console, vec!["blocked"]);
}

#[test]
fn same_origin_no_cors_response_remains_basic_and_readable() {
    let page = "http://page.test/index.html";
    let endpoint = "http://page.test/data";
    let (mut browser, transport) = browser_for(
        page,
        r#"
            fetch("/data", { mode: "no-cors" }).then(function (response) {
                console.log(response.type);
                return response.text().then(function (text) { console.log(text); });
            });
        "#,
        endpoint,
        response(endpoint),
    );

    assert_eq!(browser.tick().requests_sent, 1);
    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    assert_eq!(browser.document().runtime.console, vec!["basic", "secret body"]);
}

#[test]
fn credentialed_no_cors_updates_cookie_state_but_stays_opaque() {
    let page = "http://page.test:8000/index.html";
    let endpoint = "http://page.test:9000/data";
    let mut wire = response(endpoint);
    wire.headers.append_raw("set-cookie", "server=new; Path=/");

    let (mut browser, transport) = browser_for(
        page,
        r#"
            fetch("http://page.test:9000/data", {
                mode: "no-cors",
                credentials: "include"
            }).then(function (response) {
                console.log(response.type);
                console.log(response.headers.get("set-cookie") === null ? "cookie-hidden" : "cookie-leak");
            });
        "#,
        endpoint,
        wire,
    );

    let jar = browser.cookie_jar();
    assert!(jar.borrow_mut().store_set_cookie(
        "session=old; Path=/",
        &url("http://page.test:9000/"),
        0,
    ));

    assert_eq!(browser.tick().requests_sent, 1);
    let sent = transport.requests();
    assert_eq!(sent[0].headers.get("cookie").as_deref(), Some("session=old"));
    assert!(sent[0].headers.get("origin").is_none());

    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    assert_eq!(browser.document().runtime.console, vec!["opaque", "cookie-hidden"]);

    let stored = jar
        .borrow()
        .get_http_cookie_header(&url(endpoint), 0)
        .unwrap_or_default();
    assert!(stored.contains("session=old"), "{stored}");
    assert!(stored.contains("server=new"), "{stored}");
}
''')
