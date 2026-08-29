use browser_engine::document::Document;
use browser_engine::net::{MemoryLoader, Url};

fn run_js(js: &str) -> Document {
    let loader = MemoryLoader::new();
    let url = Url::parse("http://example.com/app/index.html").unwrap();
    let full_html = format!(
        "<!DOCTYPE html><html><body><script>{}</script></body></html>",
        js
    );
    Document::from_html(&full_html, &url, &loader)
}

#[test]
fn test_message_channel_and_ports() {
    let doc = run_js(r#"
        const channel = new MessageChannel();
        channel.port2.onmessage = function(e) {
            console.log("port2_received:" + e.data);
        };
        channel.port1.postMessage("ping_from_port1");

        // Two-way communication
        channel.port1.onmessage = function(e) {
            console.log("port1_received:" + e.data);
        };
        channel.port2.postMessage("pong_from_port2");

        // Test close
        channel.port1.close();
        channel.port1.postMessage("dropped_message");
    "#);

    let logs = doc.runtime.console;
    assert_eq!(logs[0], "port2_received:ping_from_port1");
    assert_eq!(logs[1], "port1_received:pong_from_port2");
    assert_eq!(logs.len(), 2, "No messages should be sent after port is closed");
}
