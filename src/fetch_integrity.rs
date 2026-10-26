//! Subresource Integrity verification for Fetch response bodies.
//!
//! The Fetch layer carries the authored metadata unchanged. This module parses
//! only the SRI algorithms the engine understands, selects the strongest
//! algorithm present, and accepts the bytes when any digest at that strength
//! matches. Unsupported algorithms are ignored, as required for hash agility.

use sha2::{Digest, Sha256, Sha384, Sha512};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SriAlgorithm {
    Sha256,
    Sha384,
    Sha512,
}

impl SriAlgorithm {
    fn parse(token: &str) -> Option<Self> {
        if token.eq_ignore_ascii_case("sha256") {
            Some(Self::Sha256)
        } else if token.eq_ignore_ascii_case("sha384") {
            Some(Self::Sha384)
        } else if token.eq_ignore_ascii_case("sha512") {
            Some(Self::Sha512)
        } else {
            None
        }
    }

    fn digest_base64(self, bytes: &[u8]) -> String {
        match self {
            Self::Sha256 => base64_encode(&Sha256::digest(bytes)),
            Self::Sha384 => base64_encode(&Sha384::digest(bytes)),
            Self::Sha512 => base64_encode(&Sha512::digest(bytes)),
        }
    }
}

/// Return whether `bytes` satisfy the request's integrity metadata.
///
/// Empty metadata and metadata containing only unsupported algorithms both
/// succeed. When several supported algorithms are present, only entries using
/// the strongest algorithm participate in matching; any digest at that strength
/// may match.
pub(crate) fn bytes_match_integrity(metadata: &str, bytes: &[u8]) -> bool {
    let parsed: Vec<(SriAlgorithm, &str)> = metadata
        .split(' ')
        .filter(|item| !item.is_empty())
        .filter_map(|item| {
            let expression = item.split('?').next().unwrap_or(item);
            let mut parts = expression.split('-');
            let algorithm = SriAlgorithm::parse(parts.next().unwrap_or(""))?;
            let value = parts.next().unwrap_or("");
            Some((algorithm, value))
        })
        .collect();

    let Some(strongest) = parsed.iter().map(|(algorithm, _)| *algorithm).max() else {
        return true;
    };
    let actual = strongest.digest_base64(bytes);
    parsed
        .iter()
        .any(|(algorithm, expected)| *algorithm == strongest && *expected == actual)
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(((bytes.len() + 2) / 3) * 4);
    let mut index = 0;
    while index + 3 <= bytes.len() {
        let chunk = ((bytes[index] as u32) << 16)
            | ((bytes[index + 1] as u32) << 8)
            | bytes[index + 2] as u32;
        output.push(TABLE[((chunk >> 18) & 0x3f) as usize] as char);
        output.push(TABLE[((chunk >> 12) & 0x3f) as usize] as char);
        output.push(TABLE[((chunk >> 6) & 0x3f) as usize] as char);
        output.push(TABLE[(chunk & 0x3f) as usize] as char);
        index += 3;
    }

    match bytes.len() - index {
        1 => {
            let chunk = (bytes[index] as u32) << 16;
            output.push(TABLE[((chunk >> 18) & 0x3f) as usize] as char);
            output.push(TABLE[((chunk >> 12) & 0x3f) as usize] as char);
            output.push('=');
            output.push('=');
        }
        2 => {
            let chunk = ((bytes[index] as u32) << 16) | ((bytes[index + 1] as u32) << 8);
            output.push(TABLE[((chunk >> 18) & 0x3f) as usize] as char);
            output.push(TABLE[((chunk >> 12) & 0x3f) as usize] as char);
            output.push(TABLE[((chunk >> 6) & 0x3f) as usize] as char);
            output.push('=');
        }
        _ => {}
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_sha2_digests_match_standard_base64() {
        assert_eq!(
            SriAlgorithm::Sha256.digest_base64(b"ok"),
            "Jok2eyBcFs4y7UIAlCuLix4mLfxw2byfvHfElpmk8d8="
        );
        assert_eq!(
            SriAlgorithm::Sha384.digest_base64(b"ok"),
            "xGY5+2SLy7sVzWpRRtIYtTmlK8feFq07VSi1ixAtRoi5HuFeR2Lz+5roxSCgi6m1"
        );
        assert_eq!(
            SriAlgorithm::Sha512.digest_base64(b"ok"),
            "n7u7Wg8yn5eC4jVvpB2Jz5s2lDJ8GpNNavKp3y1/k2zoNxf7UTGWpM5VSEcXCM1xNMKumbPDV7yrsur8e5t1cA=="
        );
    }

    #[test]
    fn strongest_supported_algorithm_controls_matching() {
        let weak_good = "sha256-Jok2eyBcFs4y7UIAlCuLix4mLfxw2byfvHfElpmk8d8=";
        assert!(!bytes_match_integrity(
            &format!("{weak_good} sha512-not-the-digest"),
            b"ok"
        ));
        assert!(bytes_match_integrity(
            "sha512-wrong sha512-n7u7Wg8yn5eC4jVvpB2Jz5s2lDJ8GpNNavKp3y1/k2zoNxf7UTGWpM5VSEcXCM1xNMKumbPDV7yrsur8e5t1cA==",
            b"ok"
        ));
    }

    #[test]
    fn unsupported_algorithms_are_ignored_for_hash_agility() {
        assert!(bytes_match_integrity("md5-deadbeef sha999-anything", b"ok"));
    }
}
