// ============================================================
//  demo.rs  —  The built-in demo site
// ============================================================
//
//  `cargo run` with no arguments serves `examples/site/` out of memory, so the
//  default run needs no files on disk yet still goes through the whole
//  resource-loading path: relative URLs, an external stylesheet, an external
//  script, and PNG/JPEG images.
//
//  The same directory is a real site on disk, so
//  `cargo run -- examples/site/index.html` renders the identical page through
//  the filesystem loader.

use browser_engine::net::MemoryLoader;

/// Where the in-memory demo site is rooted.
pub const ENTRY_URL: &str = "demo:///index.html";

/// Every file of the demo site, embedded at compile time.
const FILES: &[(&str, &[u8])] = &[
    (
        "demo:///index.html",
        include_bytes!("../examples/site/index.html"),
    ),
    (
        "demo:///pages/about.html",
        include_bytes!("../examples/site/pages/about.html"),
    ),
    (
        "demo:///form.html",
        include_bytes!("../examples/site/form.html"),
    ),
    (
        "demo:///async.html",
        include_bytes!("../examples/site/async.html"),
    ),
    (
        "demo:///promise.html",
        include_bytes!("../examples/site/promise.html"),
    ),
    (
        "demo:///js/promise.js",
        include_bytes!("../examples/site/js/promise.js"),
    ),
    (
        "demo:///fetch.html",
        include_bytes!("../examples/site/fetch.html"),
    ),
    (
        "demo:///js/fetch.js",
        include_bytes!("../examples/site/js/fetch.js"),
    ),
    (
        "demo:///css/fetch.css",
        include_bytes!("../examples/site/css/fetch.css"),
    ),
    (
        "demo:///api/data.json",
        include_bytes!("../examples/site/api/data.json"),
    ),
    (
        "demo:///api/note.txt",
        include_bytes!("../examples/site/api/note.txt"),
    ),
    (
        "demo:///api/echo.json",
        include_bytes!("../examples/site/api/echo.json"),
    ),
    (
        "demo:///css/async.css",
        include_bytes!("../examples/site/css/async.css"),
    ),
    (
        "demo:///js/async.js",
        include_bytes!("../examples/site/js/async.js"),
    ),
    (
        "demo:///results.html",
        include_bytes!("../examples/site/results.html"),
    ),
    (
        "demo:///css/form.css",
        include_bytes!("../examples/site/css/form.css"),
    ),
    (
        "demo:///js/form.js",
        include_bytes!("../examples/site/js/form.js"),
    ),
    (
        "demo:///css/site.css",
        include_bytes!("../examples/site/css/site.css"),
    ),
    (
        "demo:///js/app.js",
        include_bytes!("../examples/site/js/app.js"),
    ),
    (
        "demo:///logo.png",
        include_bytes!("../examples/site/logo.png"),
    ),
    (
        "demo:///photo.jpg",
        include_bytes!("../examples/site/photo.jpg"),
    ),
    (
        "demo:///assets/icon.png",
        include_bytes!("../examples/site/assets/icon.png"),
    ),
];

/// A loader serving the embedded demo site.
pub fn site() -> MemoryLoader {
    let mut loader = MemoryLoader::new();
    for (url, bytes) in FILES {
        loader.insert(url, bytes.to_vec());
    }
    loader
}
