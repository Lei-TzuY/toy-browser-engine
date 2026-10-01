use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};

use browser_engine::net::{
    DefaultLoader, FetchError, FetchRequest, FetchResponse, HttpLoader, LoadError, MemoryLoader,
    Resource, ResourceLoader, Url,
};

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

struct LegacyLoader {
    loads: AtomicUsize,
    fetches: AtomicUsize,
}

impl LegacyLoader {
    fn new() -> Self {
        Self {
            loads: AtomicUsize::new(0),
            fetches: AtomicUsize::new(0),
        }
    }
}

impl ResourceLoader for LegacyLoader {
    fn load(&self, target: &Url) -> Result<Resource, LoadError> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        Ok(Resource::new(
            target.clone(),
            Some("text/html".into()),
            b"legacy".to_vec(),
        ))
    }

    fn fetch(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        self.fetches.fetch_add(1, Ordering::SeqCst);
        Ok(FetchResponse::synthetic(
            request.url.clone(),
            299,
            Some("text/plain"),
            b"fetch".to_vec(),
        ))
    }
}

#[test]
fn legacy_loader_does_not_claim_single_hop_document_support() {
    let loader = LegacyLoader::new();
    let target = url("http://example.test/index.html");
    let request = FetchRequest::get(target.clone());

    assert!(loader
        .load_response_once(&request)
        .expect("capability probe")
        .is_none());
    assert_eq!(loader.loads.load(Ordering::SeqCst), 0);
    assert_eq!(loader.fetches.load(Ordering::SeqCst), 0);

    let response = loader.load_response(&target).expect("legacy load response");
    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"legacy");
    assert_eq!(loader.loads.load(Ordering::SeqCst), 1);
    assert_eq!(loader.fetches.load(Ordering::SeqCst), 0);
}

#[test]
fn http_loader_single_hop_load_exposes_redirect_and_forwards_request_headers() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();

    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept first hop");
        let mut request = [0u8; 4096];
        let read = stream.read(&mut request).expect("read request");
        let wire = String::from_utf8_lossy(&request[..read]);
        assert!(
            wire.to_ascii_lowercase().contains("cookie: hop=client"),
            "request-aware document load must carry browser-generated headers: {wire}"
        );
        stream
            .write_all(
                b"HTTP/1.1 302 Found\r\nLocation: /final.html\r\nSet-Cookie: hop=one; Path=/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .expect("write redirect");
    });

    let target = url(&format!("http://127.0.0.1:{port}/start.html"));
    let mut request = FetchRequest::get(target.clone());
    request.headers.insert_raw("cookie", "hop=client");
    let response = HttpLoader::default()
        .load_response_once(&request)
        .expect("transport works")
        .expect("http advertises the capability");

    assert_eq!(response.status, 302);
    assert_eq!(response.url, target);
    assert_eq!(response.headers.get("location").as_deref(), Some("/final.html"));
    assert_eq!(
        response.headers.get("set-cookie").as_deref(),
        Some("hop=one; Path=/")
    );
    assert!(!response.redirected);
    server.join().unwrap();
}

#[test]
fn http_loader_single_hop_load_keeps_error_status_as_response_metadata() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();

    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept request");
        let mut request = [0u8; 2048];
        let _ = stream.read(&mut request).expect("read request");
        stream
            .write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nContent-Length: 7\r\nConnection: close\r\n\r\nmissing",
            )
            .expect("write response");
    });

    let target = url(&format!("http://127.0.0.1:{port}/missing.html"));
    let response = HttpLoader::default()
        .load_response_once(&FetchRequest::get(target))
        .expect("transport works")
        .expect("http advertises the capability");

    assert_eq!(response.status, 404);
    assert_eq!(response.body, b"missing");
    server.join().unwrap();
}

#[test]
fn default_loader_single_hop_load_preserves_http_memory_shadowing() {
    let target = "http://127.0.0.1:9/shadow.html";
    let mut memory = MemoryLoader::new();
    memory.insert(target, "embedded");
    let loader = DefaultLoader::new().with_memory(memory);

    let response = loader
        .load_response_once(&FetchRequest::get(url(target)))
        .expect("embedded load")
        .expect("default loader can expose static memory as one hop");

    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"embedded");
    assert_eq!(response.url.to_string(), target);
}
