use toy_browser_engine::FetchMetadataDestination;

#[test]
fn request_destination_strings_match_fetch_standard_tokens() {
    let cases = [
        (FetchMetadataDestination::Empty, "empty"),
        (FetchMetadataDestination::Audio, "audio"),
        (FetchMetadataDestination::AudioWorklet, "audioworklet"),
        (FetchMetadataDestination::Document, "document"),
        (FetchMetadataDestination::Embed, "embed"),
        (FetchMetadataDestination::Font, "font"),
        (FetchMetadataDestination::Frame, "frame"),
        (FetchMetadataDestination::Iframe, "iframe"),
        (FetchMetadataDestination::Image, "image"),
        (FetchMetadataDestination::Json, "json"),
        (FetchMetadataDestination::Manifest, "manifest"),
        (FetchMetadataDestination::Object, "object"),
        (FetchMetadataDestination::PaintWorklet, "paintworklet"),
        (FetchMetadataDestination::Report, "report"),
        (FetchMetadataDestination::Script, "script"),
        (FetchMetadataDestination::ServiceWorker, "serviceworker"),
        (FetchMetadataDestination::SharedWorker, "sharedworker"),
        (FetchMetadataDestination::Style, "style"),
        (FetchMetadataDestination::Text, "text"),
        (FetchMetadataDestination::Track, "track"),
        (FetchMetadataDestination::Video, "video"),
        (FetchMetadataDestination::WebIdentity, "webidentity"),
        (FetchMetadataDestination::Worker, "worker"),
        (FetchMetadataDestination::Xslt, "xslt"),
    ];

    for (destination, token) in cases {
        assert_eq!(destination.as_str(), token);
    }
}

#[test]
fn destination_categories_are_distinct() {
    assert!(FetchMetadataDestination::Image.is_subresource());
    assert!(FetchMetadataDestination::Style.is_subresource());
    assert!(FetchMetadataDestination::Xslt.is_subresource());
    assert!(!FetchMetadataDestination::Document.is_subresource());

    assert!(FetchMetadataDestination::Document.is_navigation());
    assert!(FetchMetadataDestination::Frame.is_navigation());
    assert!(FetchMetadataDestination::Iframe.is_navigation());
    assert!(FetchMetadataDestination::Object.is_navigation());
    assert!(!FetchMetadataDestination::Worker.is_navigation());

    assert!(FetchMetadataDestination::Script.is_script_like());
    assert!(FetchMetadataDestination::AudioWorklet.is_script_like());
    assert!(FetchMetadataDestination::PaintWorklet.is_script_like());
    assert!(FetchMetadataDestination::ServiceWorker.is_script_like());
    assert!(FetchMetadataDestination::SharedWorker.is_script_like());
    assert!(FetchMetadataDestination::Worker.is_script_like());
    assert!(!FetchMetadataDestination::Xslt.is_script_like());
}
