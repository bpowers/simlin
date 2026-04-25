// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! One-time launch token used as defense-in-depth against another local
//! process opening tabs into our editor. The loopback bind is the primary
//! boundary; the token guards against a sibling process on the same machine
//! racing us to the URL by guessing a port. WebSocket bearer enforcement (the
//! actual consumer of this token) lands with the WebSocket itself in a later
//! phase; for now we only need to issue and embed it in the launch URL.

/// Generate a fresh launch token. Returns a 43-character URL-safe (RFC 4648
/// "URL and Filename safe" alphabet) string with no padding, encoding 32 bytes
/// of OS-grade randomness — 256 bits of entropy.
///
/// Each call produces a fresh token; this is intentionally not memoized,
/// because the binary is meant to issue exactly one token at startup.
pub fn generate_launch_token() -> String {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use rand::Rng;

    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two consecutive calls must produce different tokens. Collisions in 256
    /// bits of entropy are vanishingly unlikely; if this assertion ever fails
    /// it almost certainly means we accidentally returned a constant.
    #[test]
    fn repeated_calls_produce_different_tokens() {
        let a = generate_launch_token();
        let b = generate_launch_token();
        assert_ne!(a, b);
    }

    #[test]
    fn token_length_is_43_characters() {
        let token = generate_launch_token();
        assert_eq!(token.len(), 43, "token was {token:?}");
    }

    /// All output characters must come from the URL-safe base64 alphabet
    /// (A-Z, a-z, 0-9, '-', '_'). Padding ('=') is forbidden because we use
    /// the no-pad encoder; presence of any other character would indicate a
    /// programming mistake.
    #[test]
    fn all_characters_are_url_safe_base64() {
        let token = generate_launch_token();
        for (i, c) in token.chars().enumerate() {
            let allowed = c.is_ascii_alphanumeric() || c == '-' || c == '_';
            assert!(
                allowed,
                "char {i} ({c:?}) in token {token:?} is not URL-safe base64"
            );
        }
    }

    /// Mid-confidence smoke test: across 100 tokens we expect each
    /// position-character cell to be visited only a small number of times.
    /// More importantly, the resulting set should have 100 distinct entries
    /// (probabilistic uniqueness check on top of the pairwise one above).
    #[test]
    fn many_tokens_are_pairwise_distinct() {
        let count = 100;
        let mut all = std::collections::HashSet::new();
        for _ in 0..count {
            all.insert(generate_launch_token());
        }
        assert_eq!(all.len(), count);
    }
}
