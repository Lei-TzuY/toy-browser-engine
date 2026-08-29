use std::io::{Read, Write};
use std::net::TcpListener;

use browser_engine::net::{
    DefaultLoader, FetchError, FetchRequest, FetchResponse, HttpLoader, LoadError, MemoryLoader,
    Resource, ResourceLoader, Url,
};

struct CustomFetchLoader;

impl ResourceLoader for CustomFetchLoader {
    fn load(&self, url: &Url) -> Result<Resource, LoadError> {
        Err(LoadError::NotFound(url.to_string()))
    }

    fn fetch(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        let mut response = FetchResponse::synthetic(
            request.url.clone(),
            299,
            Some("text/plain"),
            b"custom-fetch".to_vec(),
        );
        response.headers.insert_raw("x-custom-fetch", "yes");
        Ok(response)
    }
}

#[test]
fn default_fetch_once_preserves_custom_fetch_overrides() {
    let request = FetchRequest::get(Url::parse("demo:///resource.txt").unwrap());
    let response = CustomFetchLoader.fetch_once(&request).unwrap();

    assert_eq!(response.status, 299);
    assert_eq!(response.body, b"custom-fetch");
    assert_eq!(response.headers.get("x-custom-fetch").as_deref(), Some("yes"));
}

#[test]
fn http_loader_fetch_once_exposes_an_intermediate_redirect() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
    let port = listener.local_addr().unwrap().port();

    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept one request");
        let mut request_bytes = [0u8; 2048];
        let _ = stream.read(&mut request_bytes).expect("read request");
        stream
            .write_all(
                b"HTTP/1.1 302 Found\r\nLocation: /final.txt\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .expect("write redirect");
    });

    let start = Url::parse(&format!("http://127.0.0.1:{port}/start.txt")).unwrap();
    let response = HttpLoader::default()
        .fetch_once(&FetchRequest::get(start.clone()))
        .expect("single-hop fetch");

    assert_eq!(response.status, 302);
    assert_eq!(response.url, start);
    assert_eq!(response.headers.get("location").as_deref(), Some("/final.txt"));
    assert!(!response.redirected);
    server.join().unwrap();
}

#[test]
fn default_loader_fetch_once_keeps_memory_shadowing_for_http_urls() {
    let mut memory = MemoryLoader::new();
    memory.insert("http://127.0.0.1:9/shadow.txt", b"embedded".to_vec());
    let loader = DefaultLoader::new().with_memory(memory);
    let request = FetchRequest::get(Url::parse("http://127.0.0.1:9/shadow.txt").unwrap());

    let response = loader.fetch_once(&request).expect("memory response");

    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"embedded");
    assert_eq!(response.url, request.url);
}
