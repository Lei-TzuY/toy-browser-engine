use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use browser_engine::net::{
    FetchError, FetchRequest, FetchResponse, HttpLoader, LoadError, NetworkBackend, Resource,
    ResourceLoader, ThreadedNetwork, Url,
};

struct DistinctFetchLoader;

impl ResourceLoader for DistinctFetchLoader {
    fn load(&self, url: &Url) -> Result<Resource, LoadError> {
        Err(LoadError::NotFound(url.to_string()))
    }

    fn fetch(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        Ok(FetchResponse::synthetic(
            request.url.clone(),
            200,
            Some("text/plain"),
            b"followed".to_vec(),
        ))
    }

    fn fetch_once(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        let mut response = FetchResponse::synthetic(
            request.url.clone(),
            302,
            Some("text/plain"),
            b"single-hop".to_vec(),
        );
        response.headers.insert_raw("location", "/next");
        Ok(response)
    }
}

fn one_completion(network: &ThreadedNetwork) -> browser_engine::net::FetchCompletion {
    if !network.wait(Duration::from_secs(2)) {
        assert!(
            network.is_busy(),
            "network became idle without exposing a completion"
        );
    }
    let completions = network.poll();
    assert_eq!(completions.len(), 1);
    completions.into_iter().next().unwrap()
}

#[test]
fn single_hop_constructor_calls_fetch_once_off_thread() {
    let network = ThreadedNetwork::new_single_hop(Arc::new(DistinctFetchLoader));
    let request = FetchRequest::get(Url::parse("http://example.test/start").unwrap());

    network.start(41, request);
    let completion = one_completion(&network);
    let response = completion.result.expect("single-hop response");

    assert_eq!(completion.id, 41);
    assert_eq!(response.status, 302);
    assert_eq!(response.body, b"single-hop");
    assert_eq!(response.headers.get("location").as_deref(), Some("/next"));
}

#[test]
fn existing_threaded_constructor_keeps_redirect_following_fetch_semantics() {
    let network = ThreadedNetwork::new(Arc::new(DistinctFetchLoader));
    let request = FetchRequest::get(Url::parse("http://example.test/start").unwrap());

    network.start(42, request);
    let completion = one_completion(&network);
    let response = completion.result.expect("ordinary fetch response");

    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"followed");
}

#[test]
fn single_hop_threaded_http_exposes_real_redirect_response() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
    let port = listener.local_addr().unwrap().port();

    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept one request");
        let mut request_bytes = [0u8; 2048];
        let _ = stream.read(&mut request_bytes).expect("read request");
        stream
            .write_all(
                b"HTTP/1.1 307 Temporary Redirect\r\nLocation: /upload-next\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .expect("write redirect");
    });

    let url = Url::parse(&format!("http://127.0.0.1:{port}/upload")).unwrap();
    let network = ThreadedNetwork::new_single_hop(Arc::new(HttpLoader::default()));
    network.start(43, FetchRequest::get(url.clone()));

    let completion = one_completion(&network);
    let response = completion.result.expect("wire response");
    assert_eq!(response.status, 307);
    assert_eq!(response.url, url);
    assert_eq!(
        response.headers.get("location").as_deref(),
        Some("/upload-next")
    );
    assert!(!response.redirected);

    server.join().unwrap();
}

#[test]
fn cancelled_single_hop_completion_is_not_delivered() {
    let network = ThreadedNetwork::new_single_hop(Arc::new(DistinctFetchLoader));
    let request = FetchRequest::get(Url::parse("http://example.test/cancel").unwrap());

    network.start(44, request);
    network.cancel(44);
    let _ = network.wait(Duration::from_secs(2));

    assert!(network.poll().is_empty());
}
