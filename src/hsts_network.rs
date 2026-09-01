// ============================================================
//  hsts_network.rs — HSTS enforcement around NetworkBackend
// ============================================================

use std::cell::RefCell;
use std::net::Ipv4Addr;
use std::rc::Rc;
use std::time::Duration;

use crate::eventloop::Clock;
use crate::hsts::HstsCache;
use crate::net::{FetchCompletion, FetchId, FetchRequest, NetworkBackend};

/// Shared HSTS state for one browser session/profile.
pub type HstsCacheRef = Rc<RefCell<HstsCache>>;

/// Parse the legacy IPv4 textual forms accepted by the WHATWG URL host parser.
///
/// The engine's shared URL parser intentionally preserves RFC-3986 host
/// spelling, but web-platform hosts such as `127.1`, `2130706433`,
/// `0x7f000001`, and octal components denote IPv4 addresses rather than DNS
/// names. RFC 6797 forbids applying HSTS policy to IP address literals, so the
/// HSTS network boundary must make that semantic distinction even before the
/// shared URL layer grows full WHATWG host canonicalization.
fn whatwg_ipv4_address(host: &str) -> Option<Ipv4Addr> {
    let host = host.strip_suffix('.').unwrap_or(host);
    if let Ok(address) = host.parse::<Ipv4Addr>() {
        return Some(address);
    }
    if host.is_empty() {
        return None;
    }

    let parts: Vec<&str> = host.split('.').collect();
    if parts.is_empty() || parts.len() > 4 || parts.iter().any(|part| part.is_empty()) {
        return None;
    }

    fn parse_number(part: &str) -> Option<u64> {
        let (digits, radix) = if let Some(hex) = part
            .strip_prefix("0x")
            .or_else(|| part.strip_prefix("0X"))
        {
            if hex.is_empty() {
                return None;
            }
            (hex, 16)
        } else if part.len() >= 2 && part.starts_with('0') {
            (&part[1..], 8)
        } else {
            (part, 10)
        };
        if digits.is_empty() {
            return Some(0);
        }
        u64::from_str_radix(digits, radix).ok()
    }

    let mut numbers = Vec::with_capacity(parts.len());
    for part in parts {
        numbers.push(parse_number(part)?);
    }
    for number in &numbers[..numbers.len().saturating_sub(1)] {
        if *number > 255 {
            return None;
        }
    }

    let last_limit = 1u64 << (8 * (5 - numbers.len()));
    if *numbers.last()? >= last_limit {
        return None;
    }

    let mut value = *numbers.last()?;
    for (index, number) in numbers[..numbers.len() - 1].iter().enumerate() {
        value += number << (8 * (3 - index));
    }
    let value = u32::try_from(value).ok()?;
    Some(Ipv4Addr::from(value))
}

/// Applies learned HTTP Strict Transport Security state at the network boundary.
///
/// The wrapped backend remains transport-only. This decorator owns the two
/// user-agent policy transitions required by RFC 6797:
///
/// - before dispatch, expired Known HSTS Host state is evicted and an HTTP
///   request to a still-known HSTS Host is rewritten to HTTPS (including the
///   RFC port mapping performed by [`HstsCache`]);
/// - after a successful HTTPS response arrives, the first
///   `Strict-Transport-Security` field is processed into the shared cache.
///
/// Keeping HSTS here means every caller of the decorated backend gets the same
/// policy without teaching JavaScript, cookies, or an individual HTTP client
/// about HSTS.
pub struct HstsNetwork {
    inner: Rc<dyn NetworkBackend>,
    cache: HstsCacheRef,
    clock: Rc<dyn Clock>,
}

impl HstsNetwork {
    pub fn new(
        inner: Rc<dyn NetworkBackend>,
        cache: HstsCacheRef,
        clock: Rc<dyn Clock>,
    ) -> HstsNetwork {
        HstsNetwork { inner, cache, clock }
    }

    pub fn with_new_cache(inner: Rc<dyn NetworkBackend>, clock: Rc<dyn Clock>) -> HstsNetwork {
        HstsNetwork::new(inner, Rc::new(RefCell::new(HstsCache::new())), clock)
    }

    pub fn cache(&self) -> HstsCacheRef {
        self.cache.clone()
    }

    pub fn inner(&self) -> &Rc<dyn NetworkBackend> {
        &self.inner
    }

    fn now_ms(&self) -> u64 {
        self.clock.now_ms().max(0.0) as u64
    }

    fn prepare_request(&self, mut request: FetchRequest) -> FetchRequest {
        let now_ms = self.now_ms();
        let mut cache = self.cache.borrow_mut();

        // RFC 6797 §8.1.1 requires expired Known HSTS Hosts to be evicted.
        // Do this at the request boundary so a profile that stops receiving
        // STS responses cannot accumulate stale policies indefinitely.
        cache.purge_expired(now_ms);

        // RFC 6797 policy is defined for domain names, never IP literals.
        // Because this engine's URL parser preserves legacy numeric IPv4 host
        // spellings, guard the request boundary against manually seeded or
        // otherwise stale policy keyed by a spelling that WHATWG resolves to
        // an IPv4 address.
        if whatwg_ipv4_address(request.url.host()).is_none() {
            request.url = cache.upgrade_url(&request.url, now_ms);
        }
        request
    }

    fn absorb_completion(&self, completion: &mut FetchCompletion) {
        let Ok(response) = &mut completion.result else {
            return;
        };

        // RFC 6797 must not learn Known HSTS Host state for IP literals. The
        // shared URL parser recognizes dotted-decimal IPs indirectly via the
        // cache, but preserves WHATWG legacy spellings such as `127.1` and
        // `2130706433`; reject those semantic IPv4 addresses here too.
        if whatwg_ipv4_address(response.url.host()).is_some() {
            return;
        }

        // RFC 6797 §8.1 requires a UA receiving duplicate STS fields over
        // secure transport to process only the first one. HeaderMap::get()
        // would join duplicates with a comma, so inspect the ordered fields.
        let first_sts = response
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("strict-transport-security"))
            .map(|(_, value)| value.to_string());

        if let Some(value) = first_sts {
            self.cache
                .borrow_mut()
                .observe_response(&response.url, &value, self.now_ms());
        }
    }
}

impl NetworkBackend for HstsNetwork {
    fn start(&self, id: FetchId, request: FetchRequest) {
        self.inner.start(id, self.prepare_request(request));
    }

    fn poll(&self) -> Vec<FetchCompletion> {
        let mut completions = self.inner.poll();
        for completion in &mut completions {
            self.absorb_completion(completion);
        }
        completions
    }

    fn cancel(&self, id: FetchId) {
        self.inner.cancel(id);
    }

    fn is_busy(&self) -> bool {
        self.inner.is_busy()
    }

    fn wait(&self, timeout: Duration) -> bool {
        self.inner.wait(timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eventloop::ManualClock;
    use crate::net::{ManualNetwork, Url};

    fn url(input: &str) -> Url {
        Url::parse(input).unwrap()
    }

    #[test]
    fn whatwg_ipv4_parser_recognizes_legacy_numeric_spellings() {
        for host in [
            "127.0.0.1",
            "127.1",
            "127.0.1",
            "2130706433",
            "0x7f000001",
            "0177.0.0.1",
            "127.1.",
        ] {
            assert_eq!(
                whatwg_ipv4_address(host),
                Some(Ipv4Addr::new(127, 0, 0, 1)),
                "host {host:?}"
            );
        }
        assert_eq!(
            whatwg_ipv4_address("0x0a000001"),
            Some(Ipv4Addr::new(10, 0, 0, 1))
        );
        assert_eq!(whatwg_ipv4_address("example.test"), None);
        assert_eq!(whatwg_ipv4_address("1.2.3.4.5"), None);
        assert_eq!(whatwg_ipv4_address("256.1.1.1"), None);
    }

    #[test]
    fn request_boundary_evicts_expired_entries_before_hsts_lookup() {
        let clock = Rc::new(ManualClock::new());
        let transport = Rc::new(ManualNetwork::new());
        let network = HstsNetwork::with_new_cache(transport, clock.clone());

        network.cache().borrow_mut().observe_response(
            &url("https://expired.test/"),
            "max-age=1",
            0,
        );
        assert_eq!(network.cache().borrow().len(), 1);

        clock.set(1_000.0);
        let prepared = network.prepare_request(FetchRequest::get(url("http://expired.test/data")));

        assert_eq!(prepared.url.to_string(), "http://expired.test/data");
        assert!(network.cache().borrow().is_empty());
    }

    #[test]
    fn request_boundary_never_upgrades_legacy_ipv4_spelling() {
        let clock = Rc::new(ManualClock::new());
        let transport = Rc::new(ManualNetwork::new());
        let network = HstsNetwork::with_new_cache(transport, clock);

        // Seed through the lower-level cache API to prove the network boundary
        // remains safe even if a legacy numeric spelling entered persisted or
        // embedder-managed state.
        assert!(network.cache().borrow_mut().observe_response(
            &url("https://2130706433/"),
            "max-age=60",
            0,
        ));
        let prepared = network.prepare_request(FetchRequest::get(url("http://2130706433/data")));
        assert_eq!(prepared.url.to_string(), "http://2130706433/data");
    }
}
