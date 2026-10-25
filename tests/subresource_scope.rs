//! Which files on disk a page's subresources — `<script src>`,
//! `<link rel=stylesheet>`, `<img src>` — may make the engine read.
//!
//! A `file:` page is confined to its local root: the directory the session
//! was opened at, when the page lies inside it, and otherwise the page's own
//! directory. A page from anywhere else never reads the disk at all. The
//! check is made when the file is opened, on the file actually opened.
//!
//! Every file here is synthetic, created in a fresh directory under the system
//! temp dir and removed when the test ends.

use std::fs;
use std::path::PathBuf;
use std::rc::Rc;

use browser_engine::{
    browser::Browser,
    document::Document,
    eventloop::ManualClock,
    net::{DefaultLoader, MemoryLoader, Url},
};

/// A 1x1 white PPM: small enough to write inline, and a format the engine
/// decodes.
const PIXEL: &[u8] = b"P6\n1 1\n255\n\xff\xff\xff";

/// A temp directory that removes itself, so a failing test leaves nothing.
///
/// `root/site/` is the site the session opens, with `index.html`, a nested
/// `pages/` directory and shared `assets/`. `root/outside/` is beyond its
/// reach and holds files that announce themselves if they are ever read.
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Fixture {
        let root = std::env::temp_dir().join(format!(
            "browser-engine-subresource-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        for dir in ["site/pages", "site/assets", "outside"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        let fixture = Fixture { root };
        fixture.write("site/assets/ok.js", "console.log('inside script');");
        fixture.write("site/assets/ok.css", "#ok { color: rgb(1, 2, 3); }");
        fs::write(fixture.path("site/assets/ok.ppm"), PIXEL).unwrap();
        fixture.write(
            "outside/evil.js",
            "console.log('synthetic outside script');",
        );
        fixture.write("outside/evil.css", "#evil { color: rgb(9, 9, 9); }");
        fs::write(fixture.path("outside/evil.ppm"), PIXEL).unwrap();
        fixture
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    fn url(&self, relative: &str) -> Url {
        Url::from_file_path(self.path(relative))
    }

    fn write(&self, relative: &str, contents: &str) {
        fs::write(self.path(relative), contents).unwrap();
    }

    /// Open `relative` as the session's first page.
    fn open(&self, relative: &str) -> Browser {
        open(DefaultLoader::new(), &self.url(relative))
    }

    #[cfg(unix)]
    fn link(&self, target: &str, link: &str) {
        std::os::unix::fs::symlink(target, self.path(link)).unwrap();
    }

    /// A directory junction: the Windows reparse point an unprivileged user
    /// can create (symbolic links need a privilege or Developer Mode).
    #[cfg(windows)]
    fn junction(&self, target: &str, link: &str) {
        let status = std::process::Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            // `cmd` reads a `/` inside an argument as the start of a switch.
            .arg(self.path(&link.replace('/', "\\")))
            .arg(self.path(&target.replace('/', "\\")))
            .stdout(std::process::Stdio::null())
            .status()
            .expect("cmd runs");
        assert!(status.success(), "mklink /J failed");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn open(loader: DefaultLoader, url: &Url) -> Browser {
    let mut browser = Browser::open_with_clock(Box::new(loader), url, Rc::new(ManualClock::new()))
        .expect("page loads");
    browser.document_mut().runtime.quiet = true;
    browser
}

/// A page that loads one script, one stylesheet and one image from `prefix`.
fn page_loading(prefix: &str, name: &str) -> String {
    format!(
        r#"<head><link rel="stylesheet" href="{prefix}{name}.css"></head>
           <img src="{prefix}{name}.ppm">
           <script src="{prefix}{name}.js"></script>"#
    )
}

/// What the page managed to load: script output, stylesheet rules beyond the
/// UA defaults, and decoded images.
#[derive(Debug, PartialEq)]
struct Loaded {
    console: String,
    author_rules: usize,
    images: usize,
}

fn loaded(document: &Document) -> Loaded {
    let ua_rules = browser_engine::css::parser::parse_css(browser_engine::document::UA_STYLESHEET)
        .rules
        .len();
    let image_count = image_urls(&document.dom)
        .iter()
        .filter(|src| {
            document
                .resolve(src)
                .is_some_and(|url| document.images.get(&url).is_some())
        })
        .count();
    Loaded {
        console: document.runtime.console.join("\n"),
        author_rules: document.stylesheet.rules.len() - ua_rules,
        images: image_count,
    }
}

fn image_urls(node: &browser_engine::dom::Node) -> Vec<String> {
    let mut out = Vec::new();
    if let browser_engine::dom::NodeType::Element(element) = &node.node_type {
        if element.tag_name == "img" {
            out.extend(element.get_attr("src").map(str::to_string));
        }
    }
    for child in &node.children {
        out.extend(image_urls(child));
    }
    out
}

fn nothing_loaded() -> Loaded {
    Loaded {
        console: String::new(),
        author_rules: 0,
        images: 0,
    }
}

fn blocked_diagnostics(document: &Document) -> usize {
    document
        .diagnostics
        .iter()
        .filter(|d| d.message.contains("blocked"))
        .count()
}

// ── Out of the local root: refused ────────────────────────────────────────────

#[test]
fn a_local_page_may_not_load_subresources_from_outside_its_site() {
    let fixture = Fixture::new("sibling");
    fixture.write("site/index.html", &page_loading("../outside/", "evil"));
    let browser = fixture.open("site/index.html");

    assert_eq!(loaded(browser.document()), nothing_loaded());
    assert_eq!(
        blocked_diagnostics(browser.document()),
        3,
        "{:?}",
        browser.document().diagnostics
    );
}

#[test]
fn percent_encoded_dots_do_not_reach_outside_the_site() {
    let fixture = Fixture::new("encoded");
    fixture.write("site/index.html", &page_loading("%2e%2e/outside/", "evil"));
    let browser = fixture.open("site/index.html");

    assert_eq!(loaded(browser.document()), nothing_loaded());
    assert_eq!(
        blocked_diagnostics(browser.document()),
        3,
        "{:?}",
        browser.document().diagnostics
    );
}

#[test]
fn an_absolute_file_url_outside_the_site_is_refused() {
    let fixture = Fixture::new("absolute");
    let outside = fixture.url("outside/").to_string();
    fixture.write("site/index.html", &page_loading(&outside, "evil"));
    let browser = fixture.open("site/index.html");

    assert_eq!(loaded(browser.document()), nothing_loaded());
    assert_eq!(blocked_diagnostics(browser.document()), 3);
}

#[test]
fn a_network_page_never_loads_local_files() {
    let fixture = Fixture::new("network");
    let outside = fixture.url("outside/").to_string();
    let mut memory = MemoryLoader::new();
    memory.insert(
        "http://page.test/index.html",
        page_loading(&outside, "evil"),
    );
    let browser = open(
        DefaultLoader::new().with_memory(memory),
        &Url::parse("http://page.test/index.html").unwrap(),
    );

    assert_eq!(loaded(browser.document()), nothing_loaded());
    assert_eq!(blocked_diagnostics(browser.document()), 3);
}

#[test]
fn an_image_added_by_a_script_later_is_held_to_the_same_root() {
    let fixture = Fixture::new("late-image");
    fixture.write(
        "site/index.html",
        r#"<button id="b">add</button><div id="holder"></div>
           <script>
             document.getElementById("b").addEventListener("click", function () {
               const img = document.createElement("img");
               img.setAttribute("src", "../outside/evil.ppm");
               document.getElementById("holder").appendChild(img);
             });
           </script>"#,
    );
    let mut browser = fixture.open("site/index.html");
    let button =
        browser_engine::script::dom_api::get_element_by_id(&browser.document().dom, "b").unwrap();
    browser.click_node(&button);

    assert_eq!(loaded(browser.document()).images, 0);
    assert_eq!(
        blocked_diagnostics(browser.document()),
        1,
        "{:?}",
        browser.document().diagnostics
    );
}

#[cfg(unix)]
#[test]
fn a_link_inside_the_site_that_leads_outside_is_refused() {
    let fixture = Fixture::new("links");
    fixture.link("../../outside/evil.js", "site/assets/escape.js");
    fixture.link("../outside", "site/escape-dir");
    fixture.write(
        "site/index.html",
        r#"<script src="assets/escape.js"></script>
           <script src="escape-dir/evil.js"></script>
           <link rel="stylesheet" href="escape-dir/evil.css">
           <img src="escape-dir/evil.ppm">"#,
    );
    let browser = fixture.open("site/index.html");

    assert_eq!(loaded(browser.document()), nothing_loaded());
    assert_eq!(
        blocked_diagnostics(browser.document()),
        4,
        "{:?}",
        browser.document().diagnostics
    );
}

#[cfg(windows)]
#[test]
fn a_junction_inside_the_site_that_leads_outside_is_refused() {
    let fixture = Fixture::new("junction-out");
    fixture.junction("outside", "site/escape-dir");
    fixture.write("site/index.html", &page_loading("escape-dir/", "evil"));
    let browser = fixture.open("site/index.html");

    assert_eq!(loaded(browser.document()), nothing_loaded());
    assert_eq!(
        blocked_diagnostics(browser.document()),
        3,
        "{:?}",
        browser.document().diagnostics
    );
}

#[cfg(windows)]
#[test]
fn a_junction_that_stays_inside_the_site_is_followed() {
    let fixture = Fixture::new("junction-in");
    fixture.junction("site/assets", "site/alias");
    fixture.write("site/index.html", &page_loading("alias/", "ok"));
    let browser = fixture.open("site/index.html");

    assert_eq!(
        loaded(browser.document()),
        Loaded {
            console: "inside script".into(),
            author_rules: 1,
            images: 1,
        }
    );
}

#[cfg(unix)]
#[test]
fn a_link_that_stays_inside_the_site_is_followed() {
    let fixture = Fixture::new("link-in");
    fixture.link("assets", "site/alias");
    fixture.write("site/index.html", &page_loading("alias/", "ok"));
    let browser = fixture.open("site/index.html");

    assert_eq!(
        loaded(browser.document()),
        Loaded {
            console: "inside script".into(),
            author_rules: 1,
            images: 1,
        }
    );
}

// ── Inside the local root, and everything that is not the disk: unchanged ─────

#[test]
fn a_local_page_loads_subresources_from_inside_its_site() {
    let fixture = Fixture::new("inside");
    fixture.write("site/index.html", &page_loading("assets/", "ok"));
    let browser = fixture.open("site/index.html");

    assert_eq!(
        loaded(browser.document()),
        Loaded {
            console: "inside script".into(),
            author_rules: 1,
            images: 1,
        }
    );
    assert!(browser.document().diagnostics.is_empty());
}

#[test]
fn a_nested_page_reached_from_the_site_may_use_parent_directory_assets() {
    let fixture = Fixture::new("nested");
    fixture.write(
        "site/index.html",
        r#"<a id="go" href="pages/nested.html">go</a>"#,
    );
    fixture.write("site/pages/nested.html", &page_loading("../assets/", "ok"));
    let mut browser = fixture.open("site/index.html");
    browser.follow_link("pages/nested.html").expect("navigates");

    assert_eq!(
        loaded(browser.document()),
        Loaded {
            console: "inside script".into(),
            author_rules: 1,
            images: 1,
        }
    );
}

/// The two roots differ on purpose: the site root for what a page runs or
/// renders, the page's own directory for what its script reads back.
#[test]
fn a_nested_page_may_use_site_assets_but_not_fetch_them() {
    let fixture = Fixture::new("fetch-vs-subresource");
    fixture.write("site/index.html", "<p>site</p>");
    fixture.write(
        "site/pages/nested.html",
        r#"<head><link rel="stylesheet" href="../assets/ok.css"></head>
           <script>
             fetch("../assets/ok.css")
                 .then(function (r) { console.log("fetched " + r.status); },
                       function (e) { console.log("rejected: " + e); });
           </script>"#,
    );
    let mut browser = fixture.open("site/index.html");
    browser.follow_link("pages/nested.html").expect("navigates");
    browser.document_mut().runtime.quiet = true;
    browser.settle_network(20);

    let loaded = loaded(browser.document());
    assert_eq!(
        loaded.author_rules, 1,
        "the stylesheet comes from the site root"
    );
    assert!(
        loaded.console.starts_with("rejected: TypeError"),
        "fetch() keeps the page's own directory: {}",
        loaded.console
    );
}

#[test]
fn a_page_opened_directly_is_the_root_of_its_own_session() {
    let fixture = Fixture::new("direct");
    fixture.write("site/pages/nested.html", &page_loading("../assets/", "ok"));
    let browser = fixture.open("site/pages/nested.html");

    assert_eq!(loaded(browser.document()), nothing_loaded());
    assert_eq!(blocked_diagnostics(browser.document()), 3);
}

#[test]
fn navigating_out_of_the_site_still_works_and_that_page_keeps_its_own_directory() {
    let fixture = Fixture::new("navigate-out");
    fixture.write("site/index.html", "<p>site</p>");
    fixture.write("outside/evil.html", &page_loading("", "evil"));
    let mut browser = fixture.open("site/index.html");
    browser
        .follow_link("../outside/evil.html")
        .expect("top-level navigation is not a subresource");

    assert_eq!(
        loaded(browser.document()),
        Loaded {
            console: "synthetic outside script".into(),
            author_rules: 1,
            images: 1,
        }
    );
}

#[test]
fn a_missing_file_inside_the_site_is_still_not_found() {
    let fixture = Fixture::new("missing");
    fixture.write(
        "site/index.html",
        r#"<script src="assets/missing.js"></script>"#,
    );
    let browser = fixture.open("site/index.html");

    let diagnostics = &browser.document().diagnostics;
    assert_eq!(diagnostics.len(), 1);
    assert!(
        diagnostics[0].message.starts_with("not found"),
        "{diagnostics:?}"
    );
}

/// A demo page with demo-scheme assets, and an http page with assets on
/// another origin — all served from memory.
fn memory_site() -> DefaultLoader {
    let mut memory = MemoryLoader::new();
    memory.insert("demo:///site/index.html", page_loading("/shared/", "ok"));
    memory.insert("demo:///shared/ok.js", "console.log('demo script');");
    memory.insert("demo:///shared/ok.css", "#ok { color: red; }");
    memory.insert("demo:///shared/ok.ppm", PIXEL);
    memory.insert(
        "http://page.test/index.html",
        page_loading("http://cdn.test/", "ok"),
    );
    memory.insert(
        "http://cdn.test/ok.js",
        "console.log('cross-origin script');",
    );
    memory.insert("http://cdn.test/ok.css", "#ok { color: red; }");
    memory.insert("http://cdn.test/ok.ppm", PIXEL);
    DefaultLoader::new().with_memory(memory)
}

#[test]
fn memory_and_network_subresources_are_unaffected() {
    let demo = open(
        memory_site(),
        &Url::parse("demo:///site/index.html").unwrap(),
    );
    assert_eq!(
        loaded(demo.document()),
        Loaded {
            console: "demo script".into(),
            author_rules: 1,
            images: 1,
        }
    );
    let network = open(
        memory_site(),
        &Url::parse("http://page.test/index.html").unwrap(),
    );
    assert_eq!(
        loaded(network.document()),
        Loaded {
            console: "cross-origin script".into(),
            author_rules: 1,
            images: 1,
        }
    );
}
