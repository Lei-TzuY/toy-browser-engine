use browser_engine::net::fetch::{FetchRegistry, FetchRequest};
use browser_engine::net::Url;

fn request(path: &str) -> FetchRequest {
    FetchRequest::get(Url::parse(&format!("demo:///{path}")).unwrap())
}

#[test]
fn public_fetch_registry_reports_unflushed_abort_as_pending_work() {
    let mut registry = FetchRegistry::new().with_limit(2);
    let id = registry.start(request("slow"), "abort-me").unwrap();

    let sent = registry.take_outbox();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0, id);

    assert_eq!(
        registry.take_where(|handle| *handle == "abort-me"),
        vec![(id, "abort-me")]
    );
    assert!(registry.is_empty());
    assert!(registry.has_pending_work());

    assert_eq!(registry.take_cancellations(), vec![id]);
    assert!(!registry.has_pending_work());
}
