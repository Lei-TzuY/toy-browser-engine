// Fetch Body cloning helpers kept beside fetch_api.rs so Request/Response cloning
// stays in the script-facing layer rather than leaking stream semantics into the
// transport representation.

impl JsRuntime {
    fn clone_request_host(&mut self, request: &RequestData) -> JsValue {
        if request.body.used() {
            self.throw_type_error(
                "Failed to execute 'clone' on 'Request': body stream is already used".to_string(),
            );
            return JsValue::Undefined;
        }

        host_value(HostObject::Request(RequestData {
            url: request.url.clone(),
            method: request.method,
            headers: headers_ref(request.headers.borrow().clone()),
            body: if request.body.present() {
                Body::new(request.body.peek().unwrap_or_default())
            } else {
                Body::absent()
            },
            signal: request.signal.clone(),
            mode: request.mode,
            credentials: request.credentials,
            redirect: request.redirect,
            referrer: request.referrer.clone(),
            referrer_policy: request.referrer_policy,
            integrity: request.integrity.clone(),
        }))
    }

    fn clone_response_host(&mut self, response: &ResponseData) -> JsValue {
        if response.body_used() {
            self.throw_type_error(
                "Failed to execute 'clone' on 'Response': body stream is already used".to_string(),
            );
            return JsValue::Undefined;
        }

        host_value(HostObject::Response(ResponseData {
            url: response.url.clone(),
            status: response.status,
            status_text: response.status_text.clone(),
            headers: headers_ref(response.headers.borrow().clone()),
            body: if response.body.present() {
                Body::new(response.body.peek().unwrap_or_default())
            } else {
                Body::absent()
            },
            redirected: response.redirected,
            response_type: response.response_type,
        }))
    }
}
