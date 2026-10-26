//! A local page is confined to its own directory subtree. These tests check
//! that the confinement holds for the file the disk actually opens, not only
//! for the URL's spelling: a symbolic link inside the directory must not lend
//! the page a file that lives outside it.
//!
//! Every file here is synthetic, created in a fresh directory under the
//! system temp dir and removed when the test ends.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use browser_engine::{
    browser::Browser,
    eventloop::ManualClock,
    net::{DefaultLoader, Url},
};

/// A temp directory that removes itself, so a failing test leaves nothing.
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    /// `root/site/` holds the page; `root/outside/` is beyond its reach.
    fn new(name: &str) -> Fixture {
        let root = std::env::temp_dir().join(format!(
            "browser-engine-containment-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("site")).unwrap();
        fs::create_dir_all(root.join("outside")).unwrap();
        fs::write(root.join("site/page.html"), "<p>page</p>").unwrap();
        fs::write(root.join("site/inside.txt"), "synthetic inside").unwrap();
        fs::write(root.join("outside/beyond.txt"), "synthetic beyond").unwrap();
        Fixture { root }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    fn link(&self, target: &str, link: &str) {
        symlink(Path::new(target), self.path(link)).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Open `page` and report what `fetch(reference)` produced.
fn fetch_from(page: &Path, reference: &str) -> String {
    let mut browser = Browser::open_with_clock(
        Box::new(DefaultLoader::new()),
        &Url::from_file_path(page),
        Rc::new(ManualClock::new()),
    )
    .expect("page loads");
    browser.document_mut().runtime.quiet = true;

    let document = browser.document_mut();
    document.runtime.run_script(
        &mut document.dom,
        &format!(
            r#"fetch("{reference}")
                .then(function (r) {{ return r.text().then(function (t) {{
                    console.log(r.status + " " + t); }}); }},
                      function (e) {{ console.log("rejected: " + e); }});"#
        ),
    );
    document.run_microtask_checkpoint();
    browser.settle_network(20);
    browser.document().runtime.console.join("\n")
}

#[test]
fn ordinary_files_and_links_that_stay_inside_are_readable() {
    let fixture = Fixture::new("inside");
    fixture.link("inside.txt", "site/alias.txt");
    let page = fixture.path("site/page.html");

    assert_eq!(fetch_from(&page, "inside.txt"), "200 synthetic inside");
    assert_eq!(fetch_from(&page, "alias.txt"), "200 synthetic inside");
    assert_eq!(fetch_from(&page, "missing.txt"), "404 not found");
}

#[test]
fn a_page_reached_through_a_linked_directory_keeps_its_own_files() {
    let fixture = Fixture::new("linked-dir");
    fixture.link("site", "mirror");
    let page = fixture.path("mirror/page.html");

    assert_eq!(fetch_from(&page, "inside.txt"), "200 synthetic inside");
}

#[test]
fn a_file_link_pointing_outside_the_directory_is_refused() {
    let fixture = Fixture::new("file-link");
    fixture.link("../outside/beyond.txt", "site/escape.txt");
    let page = fixture.path("site/page.html");

    let logs = fetch_from(&page, "escape.txt");
    assert!(logs.starts_with("rejected: TypeError"), "{logs}");
    assert!(!logs.contains("synthetic beyond"), "{logs}");
}

#[test]
fn a_directory_link_pointing_outside_the_directory_is_refused() {
    let fixture = Fixture::new("dir-link");
    fixture.link("../outside", "site/escape-dir");
    let page = fixture.path("site/page.html");

    let logs = fetch_from(&page, "escape-dir/beyond.txt");
    assert!(logs.starts_with("rejected: TypeError"), "{logs}");
    assert!(!logs.contains("synthetic beyond"), "{logs}");
}
