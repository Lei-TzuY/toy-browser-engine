use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use browser_engine::net::{
    DefaultNetwork, FetchCompletion, FetchError, FetchRequest, FetchResponse, HttpLoader,
    LoadError, NetworkBackend, Resource, ResourceLoader, Url,
};

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

struct DistinctLoader;

impl ResourceLoader for DistinctLoader {
    fn load(&self, target: &Url) -> Result<Resource, LoadError> {
        Err(LoadError::NotFound(target.to_string()))
    }

    fn fetch(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        Ok(FetchResponse::synthetic(
            request.url.clone(),
            200,
            Some("text/plain"),
            b"follow".to_vec(),
        ))
    }

    fn fetch_once(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        Ok(FetchResponse::synthetic(
            request.url.clone(),
            299,
            Some("text/plain"),
            b"single".to_vec(),
        ))
    }
}

fn one_completion(network: &DefaultNetwork) -> FetchCompletion {
    let mut completions = network.poll();
    if completions.is_empty() {
        assert!(
            network.wait(Duration::from_secs(2)) || network.is_busy(),
            "network became idle without exposing a completion"
        );
        completions = network.poll();
    }
    assert_eq!(completions.len(), 1);
    completions.into_iter().next().unwrap()
}

#[test]
fn single_hop_default_routes_http_through_fetch_once() {
    let network = DefaultNetwork::new_single_hop(Arc::new(DistinctLoader));
    network.start(
        1,
        FetchRequest::get(url("http://example.test/resource")),
    );

    let completion = one_completion(&network);
    let response = completion.result.expect("response");
    assert_eq!(response.status, 299);
    assert_eq!(response.body, b"single");
}

#[test]
fn single_hop_default_keeps_non_http_on_the_local_fetch_path() {
    let network = DefaultNetwork::new_single_hop(Arc::new(DistinctLoader));
    network.start(2, FetchRequest::get(url("demo:///resource")));

    let completions = network.poll();
    assert_eq!(completions.len(), 1);
    let response = completions[0].result.as_ref().expect("local response");
    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"follow");
}

#[test]
fn legacy_default_constructor_keeps_existing_http_fetch_semantics() {
    let network = DefaultNetwork::new(Arc::new(DistinctLoader));
    network.start(
        3,
        FetchRequest::get(url("http://example.test/resource")),
    );

    let completion = one_completion(&network);
    let response = completion.result.expect("response");
    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"follow");
}

#[test]
fn single_hop_default_exposes_a_real_http_redirect() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut request = [0u8; 2048];
        let _ = stream.read(&mut request).expect("read request");
        stream
            .write_all(
                b"HTTP/1.1 302 Found\r\nLocation: /next\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .expect("write redirect");
    });

    let network = DefaultNetwork::new_single_hop(Arc::new(HttpLoader::default()));
    let request_url = url(&format!("http://127.0.0.1:{port}/start"));
    network.start(4, FetchRequest::get(request_url.clone()));

    let completion = one_completion(&network);
    let response = completion.result.expect("wire response");
    assert_eq!(response.status, 302);
    assert_eq!(response.url, request_url);
    assert_eq!(response.headers.get("location").as_deref(), Some("/next"));
    assert!(!response.redirected);
    server.join().unwrap();
}
