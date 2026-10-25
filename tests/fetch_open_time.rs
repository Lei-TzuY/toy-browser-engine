//! A local page's `fetch()` is checked twice: when it is called, and again
//! when the network phase opens the file. Between the two the page's
//! directory can change — a file replaced by a link, a directory by a
//! junction — and the second check is the one that decides what is read.
//!
//! These tests make that swap happen deterministically: `fetch()` runs while
//! the page loads, the test rearranges the synthetic files, and only then
//! turns the event loop. Every file is created in a fresh directory under the
//! system temp dir and removed when the test ends.

use std::fs;
use std::path::PathBuf;
use std::rc::Rc;

use browser_engine::{
    browser::Browser,
    eventloop::ManualClock,
    net::{DefaultLoader, Url},
};

/// `root/site/` holds the page; `root/outside/` is beyond its reach.
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Fixture {
        let root = std::env::temp_dir().join(format!(
            "browser-engine-fetch-open-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("site/sub")).unwrap();
        fs::create_dir_all(root.join("outside")).unwrap();
        fs::write(root.join("site/data.txt"), "synthetic inside").unwrap();
        fs::write(root.join("site/sub/data.txt"), "synthetic inside sub").unwrap();
        fs::write(root.join("outside/data.txt"), "synthetic beyond").unwrap();
        Fixture { root }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// Open a page in `site/` whose script fetches `reference` at load time.
    /// The request is queued, not yet sent.
    fn open_fetching(&self, reference: &str) -> Browser {
        fs::write(
            self.path("site/page.html"),
            format!(
                r#"<script>
                     fetch("{reference}")
                         .then(function (r) {{ return r.text().then(function (t) {{
                             console.log(r.status + " " + t); }}); }},
                               function (e) {{ console.log("rejected: " + e); }});
                   </script>"#
            ),
        )
        .unwrap();
        let mut browser = Browser::open_with_clock(
            Box::new(DefaultLoader::new()),
            &Url::from_file_path(self.path("site/page.html")),
            Rc::new(ManualClock::new()),
        )
        .expect("page loads");
        browser.document_mut().runtime.quiet = true;
        assert!(
            browser.document().runtime.console.is_empty(),
            "the request must still be pending when the test rearranges the files"
        );
        browser
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn settle(mut browser: Browser) -> String {
    browser.settle_network(20);
    browser.document().runtime.console.join("\n")
}

#[test]
fn an_unchanged_file_is_read_at_open_time() {
    let fixture = Fixture::new("unchanged");
    let browser = fixture.open_fetching("data.txt");
    assert_eq!(settle(browser), "200 synthetic inside");
}

#[test]
fn a_file_removed_before_the_open_is_a_404() {
    let fixture = Fixture::new("removed");
    let browser = fixture.open_fetching("data.txt");
    fs::remove_file(fixture.path("site/data.txt")).unwrap();
    assert_eq!(settle(browser), "404 not found");
}

#[cfg(unix)]
#[test]
fn a_file_replaced_by_a_link_outside_after_the_check_is_not_read() {
    let fixture = Fixture::new("file-swap");
    let browser = fixture.open_fetching("data.txt");

    fs::remove_file(fixture.path("site/data.txt")).unwrap();
    std::os::unix::fs::symlink("../outside/data.txt", fixture.path("site/data.txt")).unwrap();

    let logs = settle(browser);
    assert!(logs.starts_with("rejected: TypeError"), "{logs}");
    assert!(!logs.contains("synthetic beyond"), "{logs}");
}

#[cfg(unix)]
#[test]
fn a_directory_replaced_by_a_link_outside_after_the_check_is_not_read() {
    let fixture = Fixture::new("dir-swap");
    let browser = fixture.open_fetching("sub/data.txt");

    fs::rename(fixture.path("site/sub"), fixture.path("site/sub-old")).unwrap();
    std::os::unix::fs::symlink("../outside", fixture.path("site/sub")).unwrap();

    let logs = settle(browser);
    assert!(logs.starts_with("rejected: TypeError"), "{logs}");
    assert!(!logs.contains("synthetic beyond"), "{logs}");
}

#[cfg(windows)]
#[test]
fn a_directory_replaced_by_a_junction_outside_after_the_check_is_not_read() {
    let fixture = Fixture::new("junction-swap");
    let browser = fixture.open_fetching("sub/data.txt");

    // A directory junction needs no privilege, unlike a symbolic link.
    fs::rename(fixture.path("site/sub"), fixture.path("site/sub-old")).unwrap();
    let status = std::process::Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(fixture.path("site/sub"))
        .arg(fixture.path("outside"))
        .stdout(std::process::Stdio::null())
        .status()
        .expect("cmd runs");
    assert!(status.success(), "mklink /J failed");

    let logs = settle(browser);
    assert!(logs.starts_with("rejected: TypeError"), "{logs}");
    assert!(!logs.contains("synthetic beyond"), "{logs}");
}
