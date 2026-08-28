use std::net::IpAddr;

use crate::net::Url;

/// Security-sensitive origin classification for browser policy decisions.
///
/// Unlike the legacy fetch-origin helper, this model distinguishes opaque URLs
/// from local resources. `about:`, `data:`, `mailto:`, `urn:`, hostless
/// HTTP(S), and unknown opaque schemes therefore do not accidentally acquire
/// local-directory privileges merely because they have no host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecurityOrigin {
    /// A normal network tuple origin.
    Tuple {
        scheme: String,
        host: String,
        port: u16,
    },
    /// A deliberately local resource namespace used by `file:` and `demo:`.
    Local {
        scheme: String,
        /// The document directory, always ending in `/`.
        directory: String,
    },
    /// A unique/opaque origin that is never same-origin with another URL.
    Opaque,
}

impl SecurityOrigin {
    /// Classify the security origin for `url`.
    pub fn of(url: &Url) -> Self {
        if matches!(url.scheme(), "http" | "https") && !url.host().is_empty() {
            return SecurityOrigin::Tuple {
                scheme: url.scheme().to_string(),
                host: url.host().to_ascii_lowercase(),
                port: url
                    .port_or_default()
                    .expect("HTTP(S) URLs always have an effective port"),
            };
        }

        if matches!(url.scheme(), "file" | "demo") && !url.is_opaque() {
            let path = url.path();
            let directory = match path.rfind('/') {
                Some(index) => path[..=index].to_string(),
                None => "/".to_string(),
            };
            return SecurityOrigin::Local {
                scheme: url.scheme().to_string(),
                directory,
            };
        }

        SecurityOrigin::Opaque
    }

    /// Whether this origin may perform the engine's same-origin fetch to
    /// `target`.
    ///
    /// Opaque origins deliberately never compare equal, including against the
    /// URL that produced them. Local origins retain the engine's existing
    /// directory-subtree confinement for `file:` and `demo:` resources.
    pub fn can_fetch(&self, target: &Url) -> bool {
        let target_origin = SecurityOrigin::of(target);
        match (self, target_origin) {
            (
                SecurityOrigin::Tuple {
                    scheme,
                    host,
                    port,
                },
                SecurityOrigin::Tuple {
                    scheme: other_scheme,
                    host: other_host,
                    port: other_port,
                },
            ) => scheme == &other_scheme && host == &other_host && *port == other_port,
            (
                SecurityOrigin::Local { scheme, directory },
                SecurityOrigin::Local {
                    scheme: other_scheme,
                    ..
                },
            ) => scheme == &other_scheme && target.path().starts_with(directory),
            _ => false,
        }
    }

    /// Whether this origin is "potentially trustworthy" for secure-context
    /// policy decisions.
    ///
    /// This follows the useful subset of the Secure Contexts trust algorithm
    /// that the engine can represent today:
    ///
    /// - every HTTPS tuple origin is trustworthy;
    /// - HTTP is trustworthy only for loopback IP literals or `localhost`
    ///   names (including subdomains ending in `.localhost`);
    /// - the engine's intentional local namespaces (`file:` and `demo:`) are
    ///   trustworthy;
    /// - opaque origins are not trustworthy on their own.
    ///
    /// Treating loopback development origins as trustworthy matters for a
    /// browser engine because local test servers should be able to exercise
    /// secure-only APIs without weakening ordinary clear-text HTTP origins.
    pub fn is_potentially_trustworthy(&self) -> bool {
        match self {
            SecurityOrigin::Tuple { scheme, host, .. } => {
                if scheme == "https" {
                    return true;
                }
                if scheme != "http" {
                    return false;
                }

                let host = host.trim_end_matches('.');
                if host.eq_ignore_ascii_case("localhost")
                    || host.to_ascii_lowercase().ends_with(".localhost")
                {
                    return true;
                }

                let ip_literal = host.trim_start_matches('[').trim_end_matches(']');
                ip_literal
                    .parse::<IpAddr>()
                    .map(|ip| ip.is_loopback())
                    .unwrap_or(false)
            }
            SecurityOrigin::Local { .. } => true,
            SecurityOrigin::Opaque => false,
        }
    }

    /// Serialize the request `Origin` header representation.
    ///
    /// Local and opaque origins serialize as `null`; tuple origins omit their
    /// default HTTP(S) port.
    pub fn header_value(&self) -> String {
        match self {
            SecurityOrigin::Tuple { scheme, host, port } => {
                let default_port = match scheme.as_str() {
                    "https" => 443,
                    _ => 80,
                };
                if *port == default_port {
                    format!("{scheme}://{host}")
                } else {
                    format!("{scheme}://{host}:{port}")
                }
            }
            SecurityOrigin::Local { .. } | SecurityOrigin::Opaque => "null".to_string(),
        }
    }

    pub fn is_opaque(&self) -> bool {
        matches!(self, SecurityOrigin::Opaque)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(input: &str) -> Url {
        Url::parse(input).expect("valid URL")
    }

    #[test]
    fn network_tuple_uses_effective_port() {
        let origin = SecurityOrigin::of(&url("http://example.test/page"));
        assert!(origin.can_fetch(&url("http://example.test:80/api")));
        assert!(!origin.can_fetch(&url("http://example.test:8080/api")));
        assert_eq!(origin.header_value(), "http://example.test");
    }

    #[test]
    fn opaque_urls_never_gain_local_privileges() {
        for input in [
            "about:blank",
            "data:text/plain,hello",
            "mailto:user@example.test",
            "urn:isbn:9780131103627",
            "widget:opaque-value",
            "http:relative-hostless",
        ] {
            let parsed = url(input);
            let origin = SecurityOrigin::of(&parsed);
            assert!(origin.is_opaque(), "{input} should be opaque for security");
            assert!(!origin.can_fetch(&parsed), "opaque origins are unique");
            assert_eq!(origin.header_value(), "null");
        }
    }

    #[test]
    fn local_resources_remain_confined_to_their_directory() {
        let origin = SecurityOrigin::of(&url("demo:///site/index.html"));
        assert!(origin.can_fetch(&url("demo:///site/api/data.json")));
        assert!(!origin.can_fetch(&url("demo:///secrets.txt")));
        assert!(!origin.can_fetch(&url("http://example.test/site/api/data.json")));
        assert_eq!(origin.header_value(), "null");
    }

    #[test]
    fn secure_context_trust_covers_https_loopback_and_local_origins() {
        for input in [
            "https://example.test/",
            "https://example.test:8443/",
            "http://localhost/",
            "http://dev.localhost:8080/",
            "http://127.0.0.1:3000/",
            "http://127.99.4.2/",
            "http://[::1]/",
            "file:///tmp/index.html",
            "demo:///index.html",
        ] {
            assert!(
                SecurityOrigin::of(&url(input)).is_potentially_trustworthy(),
                "{input} should be potentially trustworthy"
            );
        }
    }

    #[test]
    fn ordinary_http_and_opaque_origins_are_not_trustworthy() {
        for input in [
            "http://example.test/",
            "http://192.168.1.10/",
            "http://10.0.0.1/",
            "about:blank",
            "data:text/plain,hello",
            "widget:opaque-value",
        ] {
            assert!(
                !SecurityOrigin::of(&url(input)).is_potentially_trustworthy(),
                "{input} should not be potentially trustworthy"
            );
        }
    }
}
