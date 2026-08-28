use browser_engine::{
    net::{DefaultLoader, MemoryLoader, Url},
    script::dom_api,
    Browser, Document,
};

fn browser_with_script(script: &str) -> Browser {
    let mut memory = MemoryLoader::new();
    memory.insert(
        "demo:///index.html",
        format!(
            r#"<!doctype html>
<html>
  <body>
    <div id="meta">pending</div>
    <div id="out">pending</div>
    <script>{script}</script>
  </body>
</html>"#
        ),
    );
    Browser::open(
        Box::new(DefaultLoader::new().with_memory(memory)),
        &Url::parse("demo:///index.html").unwrap(),
    )
    .unwrap()
}

fn text(browser: &Browser, id: &str) -> String {
    let path = dom_api::get_element_by_id(&browser.document().dom, id).expect("element exists");
    dom_api::text_content(dom_api::node_at(&browser.document().dom, &path).unwrap())
}

#[test]
fn javascript_fetch_reads_percent_encoded_data_url() {
    let mut browser = browser_with_script(
        r#"
        fetch("data:text/plain,hello%20from%20data")
          .then(function (response) {
            document.getElementById("meta").textContent =
              response.status + "|" + response.headers.get("content-type") + "|" + response.url;
            return response.text();
          })
          .then(function (body) {
            document.getElementById("out").textContent = body;
          })
          .catch(function (error) {
            document.getElementById("out").textContent = "ERR:" + error;
          });
        "#,
    );

    assert_eq!(text(&browser, "out"), "pending");
    let report = browser.settle_network(16);
    assert!(report.requests_sent >= 1);
    assert!(report.network_completions >= 1);
    assert_eq!(text(&browser, "out"), "hello from data");
    assert_eq!(
        text(&browser, "meta"),
        "200|text/plain|data:text/plain,hello%20from%20data"
    );
}

#[test]
fn javascript_fetch_decodes_base64_json_and_response_json() {
    // {"answer":42}
    let mut browser = browser_with_script(
        r#"
        fetch("data:application/json;base64,eyJhbnN3ZXIiOjQyfQ==")
          .then(function (response) { return response.json(); })
          .then(function (value) {
            document.getElementById("out").textContent = "answer=" + value.answer;
          })
          .catch(function (error) {
            document.getElementById("out").textContent = "ERR:" + error;
          });
        "#,
    );

    browser.settle_network(16);
    assert_eq!(text(&browser, "out"), "answer=42");
}

#[test]
fn data_fetch_remains_read_only_for_non_get_methods() {
    let mut browser = browser_with_script(
        r#"
        fetch("data:text/plain,immutable", { method: "POST", body: "write" })
          .then(function (response) {
            document.getElementById("meta").textContent = response.status + "|" + response.ok;
            return response.text();
          })
          .then(function (body) {
            document.getElementById("out").textContent = body;
          })
          .catch(function (error) {
            document.getElementById("out").textContent = "ERR:" + error;
          });
        "#,
    );

    browser.settle_network(16);
    assert_eq!(text(&browser, "meta"), "405|false");
    assert_eq!(text(&browser, "out"), "this source only serves reads");
}

#[test]
fn allowing_data_does_not_expose_about_or_javascript_schemes() {
    let mut browser = browser_with_script(
        r#"
        let failures = 0;
        fetch("about:blank").catch(function (_) { failures = failures + 1; });
        fetch("javascript:alert(1)").catch(function (_) { failures = failures + 1; });
        Promise.resolve().then(function () {
          document.getElementById("out").textContent = "failures=" + failures;
        });
        "#,
    );

    browser.tick();
    browser.tick();
    assert_eq!(text(&browser, "out"), "failures=2");
}

#[test]
fn opaque_document_origin_still_cannot_fetch_local_resources() {
    let loader = DefaultLoader::new();
    let document = Document::from_html(
        r#"<!doctype html><body>pending<script>
          fetch("file:///tmp/secret.txt")
            .then(function () { document.body.textContent = "unexpected"; })
            .catch(function () { document.body.textContent = "blocked"; });
        </script></body>"#,
        &Url::parse("about:blank").unwrap(),
        &loader,
    );

    assert!(dom_api::text_content(&document.dom).contains("blocked"));
    assert!(!dom_api::text_content(&document.dom).contains("unexpected"));
}
