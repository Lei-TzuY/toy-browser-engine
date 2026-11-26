//! Temporary observations for the two hosted Windows navigation fixtures.
use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

#[link(name = "kernel32")]
extern "system" {
    fn QueryPerformanceCounter(counter: *mut i64) -> i32;
    fn QueryPerformanceFrequency(frequency: *mut i64) -> i32;
}

pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        [
            ("TBE_HOSTED_U23_TRACE", "1"),
            ("RUNNER_ENVIRONMENT", "github-hosted"),
            ("GITHUB_ACTIONS", "true"),
            ("GITHUB_REPOSITORY", "Lei-TzuY/toy-browser-engine"),
        ]
        .iter()
        .copied()
        .all(|(key, value)| std::env::var(key).as_deref() == Ok(value))
    })
}

pub fn event(name: &str, details: fmt::Arguments<'_>) {
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    static FREQUENCY: OnceLock<i64> = OnceLock::new();
    if !enabled() || COUNT.fetch_add(1, Ordering::Relaxed) >= 128 {
        return;
    }
    let frequency = FREQUENCY.get_or_init(|| {
        let mut value = 0;
        unsafe {
            QueryPerformanceFrequency(&mut value);
        }
        value
    });
    let mut counter = 0;
    if unsafe { QueryPerformanceCounter(&mut counter) } != 0 {
        eprintln!("U23_TRACE qpc={counter} hz={frequency} {name} {details}");
    }
}

pub fn fixture_url(url: &str) -> &str {
    if let Some((port, path)) = url.strip_prefix("http://127.0.0.1:").and_then(|s| s.split_once('/')) {
        if port.parse::<u16>().is_ok()
            && matches!(path, "index.html" | "not-found" | "unavailable" | "target.html")
        {
            return url;
        }
    }
    "[non-fixture URL omitted]"
}
