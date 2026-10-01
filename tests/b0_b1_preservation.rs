use std::rc::Rc;

use browser_engine::document::PointerState;
use browser_engine::eventloop::ManualClock;
use browser_engine::layout::{layout_tree, BoxType, LayoutBox};
use browser_engine::net::{ManualNetwork, MemoryLoader, Url};
use browser_engine::script::dom_api;
use browser_engine::Browser;

fn text(browser: &Browser, id: &str) -> String {
    let path = dom_api::get_element_by_id(&browser.document().dom, id).expect("element exists");
    dom_api::text_content(dom_api::node_at(&browser.document().dom, &path).unwrap())
}

fn find_table_box<'a>(b: &'a LayoutBox<'a>) -> Option<&'a LayoutBox<'a>> {
    if matches!(b.box_type, BoxType::Table(_)) {
        return Some(b);
    }
    for child in &b.children {
        if let Some(found) = find_table_box(child) {
            return Some(found);
        }
    }
    None
}

/// (1) Prove main MessageChannel bidirectional/close and MutationObserver record/disconnect
/// behavior survives an in-flight ordinary Fetch.
#[test]
fn test_message_channel_and_mutation_observer_survive_in_flight_fetch() {
    let mut loader = MemoryLoader::new();
    loader.insert(
        "http://example.test/index.html",
        r#"<!DOCTYPE html>
        <html>
        <body>
            <div id="target"></div>
            <script>
                fetch("/in-flight-endpoint")
                    .then(function(r) { return r.text(); })
                    .then(function(body) {
                        console.log("fetch_resolved:" + body);
                    });

                // MessageChannel bidirectional messaging and close behavior
                const channel = new MessageChannel();
                channel.port2.onmessage = function(e) {
                    console.log("port2_received:" + e.data);
                };
                channel.port1.postMessage("ping_from_port1");

                channel.port1.onmessage = function(e) {
                    console.log("port1_received:" + e.data);
                };
                channel.port2.postMessage("pong_from_port2");

                channel.port1.close();
                channel.port1.postMessage("dropped_message");

                // MutationObserver observe, record, and disconnect behavior
                const target = document.getElementById("target");
                const observer = new MutationObserver(function(mutations) {});
                observer.observe(target, { childList: true, attributes: true });

                const records = observer.takeRecords();
                console.log("records_len:" + records.length);
                if (records.length > 0) {
                    console.log("record_type:" + records[0].type);
                }

                observer.disconnect();
                console.log("records_after_disconnect:" + observer.takeRecords().length);
            </script>
        </body>
        </html>"#,
    );

    let manual = Rc::new(ManualNetwork::new());
    // Auto-complete is false by default: request will stay pending in-flight
    manual.respond_text("http://example.test/in-flight-endpoint", "response_payload");

    let clock = Rc::new(ManualClock::new());
    let mut browser = Browser::open_with_network(
        Box::new(loader),
        manual.clone(),
        &Url::parse("http://example.test/index.html").unwrap(),
        clock,
    )
    .expect("open browser");

    // Tick the event loop once to dispatch the queued network request to manual
    browser.tick();

    // Fetch has started and is currently in-flight
    assert_eq!(manual.requests().len(), 1, "exactly one request issued");
    assert_eq!(manual.pending_count(), 1, "fetch is currently in flight");

    // MessageChannel and MutationObserver operations have run and recorded their logs
    let logs = &browser.document().runtime.console;
    assert_eq!(logs[0], "port2_received:ping_from_port1");
    assert_eq!(logs[1], "port1_received:pong_from_port2");
    assert_eq!(logs[2], "records_len:1");
    assert_eq!(logs[3], "record_type:childList");
    assert_eq!(logs[4], "records_after_disconnect:0");
    assert_eq!(logs.len(), 5, "fetch has not resolved yet");

    // Complete in-flight fetch and settle event loop
    assert!(manual.complete_url("in-flight-endpoint"));
    browser.settle_network(16);

    assert_eq!(manual.pending_count(), 0, "no requests pending after settle");
    assert_eq!(manual.requests().len(), 1, "no extra wire requests sent");

    let final_logs = &browser.document().runtime.console;
    assert_eq!(final_logs.len(), 6);
    assert_eq!(final_logs[5], "fetch_resolved:response_payload");
}

/// (2) Prove Fetch with AbortSignal.abort(), timeout(0), and already-aborted any()
/// sends no wire request while non-aborted Fetch succeeds.
#[test]
fn test_aborted_fetches_send_no_wire_requests_while_non_aborted_succeeds() {
    let mut loader = MemoryLoader::new();
    loader.insert(
        "http://example.test/index.html",
        r#"<!DOCTYPE html>
        <html>
        <body>
            <script>
                // 1. Fetch with AbortSignal.abort()
                const s1 = AbortSignal.abort();
                fetch("/aborted-direct", { signal: s1 })
                    .then(function() { console.log("s1:unexpected_success"); })
                    .catch(function(err) { console.log("s1:aborted"); });

                // 2. Fetch with AbortSignal.timeout(0)
                const s2 = AbortSignal.timeout(0);
                fetch("/aborted-timeout", { signal: s2 })
                    .then(function() { console.log("s2:unexpected_success"); })
                    .catch(function(err) { console.log("s2:aborted"); });

                // 3. Fetch with already-aborted AbortSignal.any()
                const s3 = AbortSignal.any([s1]);
                fetch("/aborted-any", { signal: s3 })
                    .then(function() { console.log("s3:unexpected_success"); })
                    .catch(function(err) { console.log("s3:aborted"); });

                // 4. Non-aborted fetch
                fetch("/allowed-request")
                    .then(function(r) { return r.text(); })
                    .then(function(body) { console.log("allowed:" + body); })
                    .catch(function(err) { console.log("allowed:unexpected_failure"); });
            </script>
        </body>
        </html>"#,
    );

    let manual = Rc::new(ManualNetwork::new());
    manual.set_auto_complete(true);
    manual.respond_text("http://example.test/allowed-request", "ok_data");

    let clock = Rc::new(ManualClock::new());
    let mut browser = Browser::open_with_network(
        Box::new(loader),
        manual.clone(),
        &Url::parse("http://example.test/index.html").unwrap(),
        clock,
    )
    .expect("open browser");

    browser.settle_network(16);

    let wire_requests = manual.requests();
    assert_eq!(wire_requests.len(), 1, "exactly one wire request sent for non-aborted fetch");
    assert_eq!(
        wire_requests[0].url.to_string(),
        "http://example.test/allowed-request"
    );

    let logs = &browser.document().runtime.console;
    assert_eq!(logs[0], "s1:aborted");
    assert_eq!(logs[1], "s2:aborted");
    assert_eq!(logs[2], "s3:aborted");
    assert_eq!(logs[3], "allowed:ok_data");
    assert_eq!(logs.len(), 4);
}

/// (3) Prove public-session rendering preserves table spacing/row-height and computed
/// scroll/visual properties through a Fetch-driven DOM update.
#[test]
fn test_public_session_rendering_preserves_table_and_scroll_visual_properties() {
    let mut loader = MemoryLoader::new();
    loader.insert(
        "http://example.test/index.html",
        r#"<!DOCTYPE html>
        <html>
        <head>
            <style>
                table#main-table {
                    display: table;
                    width: 310px;
                    border-spacing: 10px;
                }
                tr {
                    display: table-row;
                }
                td {
                    display: table-cell;
                }
                .col1 {
                    width: 100px;
                    height: 40px;
                }
                .col2 {
                    width: 200px;
                    height: 60px;
                }
                div#scroll-container {
                    scroll-behavior: smooth;
                    scroll-snap-type: y mandatory;
                    outline: 2px solid #ff0000;
                    outline-offset: 4px;
                }
                div#scroll-item {
                    scroll-snap-align: center;
                }
                div#visual-box {
                    text-shadow: 2px 2px 4px #000000;
                    caret-color: #00ff00;
                }
            </style>
        </head>
        <body>
            <div id="status">initial</div>
            <table id="main-table"><tr><td class="col1">A</td><td class="col2">B</td></tr></table>
            <div id="scroll-container">
                <div id="scroll-item">Item</div>
            </div>
            <div id="visual-box">Visual</div>
            <script>
                fetch("/update-content")
                    .then(function(r) { return r.text(); })
                    .then(function(txt) {
                        document.getElementById("status").textContent = txt;
                        console.log("dom_updated:" + txt);
                    });
            </script>
        </body>
        </html>"#,
    );

    let manual = Rc::new(ManualNetwork::new());
    manual.respond_text("http://example.test/update-content", "dom_content_updated");

    let clock = Rc::new(ManualClock::new());
    let mut browser = Browser::open_with_network(
        Box::new(loader),
        manual.clone(),
        &Url::parse("http://example.test/index.html").unwrap(),
        clock,
    )
    .expect("open browser");

    // Pre-update checks
    let path_table = dom_api::get_element_by_id(&browser.document().dom, "main-table").unwrap();
    let path_container = dom_api::get_element_by_id(&browser.document().dom, "scroll-container").unwrap();
    let path_item = dom_api::get_element_by_id(&browser.document().dom, "scroll-item").unwrap();
    let path_visual = dom_api::get_element_by_id(&browser.document().dom, "visual-box").unwrap();

    let styled_before = browser.document().style_tree(800.0, &PointerState::default());
    let table_styled_before = styled_before.find_at_path(&path_table).unwrap();
    assert_eq!(table_styled_before.border_spacing(), 10.0);
    assert_eq!(table_styled_before.border_collapse(), "separate");

    let container_styled_before = styled_before.find_at_path(&path_container).unwrap();
    assert_eq!(container_styled_before.scroll_behavior(), "smooth");
    assert_eq!(container_styled_before.scroll_snap_type(), "y mandatory");
    assert_eq!(container_styled_before.outline_offset(), 4.0);

    let item_styled_before = styled_before.find_at_path(&path_item).unwrap();
    assert_eq!(item_styled_before.scroll_snap_align(), "center");

    let visual_styled_before = styled_before.find_at_path(&path_visual).unwrap();
    assert!(visual_styled_before.text_shadow().is_some());
    assert!(visual_styled_before.caret_color().is_some());

    // Verify isolated table layout matches table_layout test
    let table_layout_before = layout_tree(table_styled_before, 400.0);
    let row_before = table_layout_before
        .children
        .iter()
        .find(|c| matches!(c.box_type, BoxType::TableRow(_)))
        .expect("table row box");
    assert_eq!(row_before.children.len(), 2);
    let cell1_before = &row_before.children[0];
    let cell2_before = &row_before.children[1];
    assert_eq!(
        cell2_before.dimensions.content.x,
        cell1_before.dimensions.content.x + cell1_before.dimensions.content.width + 10.0
    );
    assert_eq!(row_before.dimensions.content.height, 60.0);
    assert_eq!(cell1_before.dimensions.content.height, 60.0);

    // Verify full document layout
    let doc_layout_before = browser.document().layout(&styled_before, 800.0);
    let table_box_before = find_table_box(&doc_layout_before).expect("table box exists in document layout");
    let doc_row_before = table_box_before
        .children
        .iter()
        .find(|c| matches!(c.box_type, BoxType::TableRow(_)))
        .expect("table row box in document layout");
    assert_eq!(doc_row_before.dimensions.content.height, 60.0);

    // Verify initial rendering
    let canvas_before = browser.render(800, 600, 0.0, &PointerState::default());
    assert_eq!(canvas_before.width, 800);
    assert_eq!(canvas_before.height, 600);
    assert_eq!(text(&browser, "status"), "initial");

    // Complete the Fetch request and settle
    browser.tick();
    assert_eq!(manual.requests().len(), 1);
    assert!(manual.complete_url("update-content"));
    browser.settle_network(16);

    // Verify DOM was updated by Fetch
    assert_eq!(text(&browser, "status"), "dom_content_updated");
    assert_eq!(
        browser.document().runtime.console.last().map(String::as_str),
        Some("dom_updated:dom_content_updated")
    );
    assert_eq!(manual.requests().len(), 1, "still exactly one wire request");

    // Post-update checks: styling, spacing, row height, scroll/visual properties preserved
    let styled_after = browser.document().style_tree(800.0, &PointerState::default());
    let table_styled_after = styled_after.find_at_path(&path_table).unwrap();
    assert_eq!(table_styled_after.border_spacing(), 10.0);
    assert_eq!(table_styled_after.border_collapse(), "separate");

    let container_styled_after = styled_after.find_at_path(&path_container).unwrap();
    assert_eq!(container_styled_after.scroll_behavior(), "smooth");
    assert_eq!(container_styled_after.scroll_snap_type(), "y mandatory");
    assert_eq!(container_styled_after.outline_offset(), 4.0);

    let item_styled_after = styled_after.find_at_path(&path_item).unwrap();
    assert_eq!(item_styled_after.scroll_snap_align(), "center");

    let visual_styled_after = styled_after.find_at_path(&path_visual).unwrap();
    assert!(visual_styled_after.text_shadow().is_some());
    assert!(visual_styled_after.caret_color().is_some());

    let table_layout_after = layout_tree(table_styled_after, 400.0);
    let row_after = table_layout_after
        .children
        .iter()
        .find(|c| matches!(c.box_type, BoxType::TableRow(_)))
        .expect("table row box");
    assert_eq!(row_after.children.len(), 2);
    let cell1_after = &row_after.children[0];
    let cell2_after = &row_after.children[1];
    assert_eq!(
        cell2_after.dimensions.content.x,
        cell1_after.dimensions.content.x + cell1_after.dimensions.content.width + 10.0
    );
    assert_eq!(row_after.dimensions.content.height, 60.0);
    assert_eq!(cell1_after.dimensions.content.height, 60.0);

    let doc_layout_after = browser.document().layout(&styled_after, 800.0);
    let table_box_after = find_table_box(&doc_layout_after).expect("table box exists in document layout");
    let doc_row_after = table_box_after
        .children
        .iter()
        .find(|c| matches!(c.box_type, BoxType::TableRow(_)))
        .expect("table row box in document layout");
    assert_eq!(doc_row_after.dimensions.content.height, 60.0);

    // Verify rendering after update
    let canvas_after = browser.render(800, 600, 0.0, &PointerState::default());
    assert_eq!(canvas_after.width, 800);
    assert_eq!(canvas_after.height, 600);
}
