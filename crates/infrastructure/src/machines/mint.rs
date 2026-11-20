//! Join tokens (decision 0029): `qdj_` and 32 bytes of the operating
//! system's CSPRNG in base64url; what is stored is their SHA-256 in hex.

use auth_gateway::{JoinTokenMint, JOIN_TOKEN_PREFIX};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ring::digest::{digest, SHA256};
use ring::rand::{SecureRandom, SystemRandom};

/// Makes join tokens with [`SystemRandom`].
#[derive(Debug, Clone)]
pub struct RingJoinTokenMint {
    rng: SystemRandom,
}

impl Default for RingJoinTokenMint {
    fn default() -> Self {
        Self::new()
    }
}

impl RingJoinTokenMint {
    pub fn new() -> Self {
        Self {
            rng: SystemRandom::new(),
        }
    }
}

impl JoinTokenMint for RingJoinTokenMint {
    fn mint(&self) -> Option<String> {
        let mut bytes = [0u8; 32];
        self.rng.fill(&mut bytes).ok()?;
        Some(format!(
            "{JOIN_TOKEN_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(bytes)
        ))
    }

    fn hash(&self, token: &str) -> String {
        digest(&SHA256, token.as_bytes())
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use auth_gateway::is_join_token;

    use super::*;

    #[test]
    fn machines_join_tokens_are_random_and_of_their_form() {
        let mint = RingJoinTokenMint::new();
        let a = mint.mint().unwrap();
        let b = mint.mint().unwrap();
        assert_ne!(a, b);
        for token in [&a, &b] {
            assert!(is_join_token(token), "{token}");
            let bytes = URL_SAFE_NO_PAD.decode(&token[4..]).unwrap();
            assert_eq!(bytes.len(), 32);
        }
    }

    #[test]
    fn machines_join_tokens_are_stored_as_their_sha256() {
        let mint = RingJoinTokenMint::new();
        // SHA-256 of "abc" (FIPS 180-2, appendix B.1).
        assert_eq!(
            mint.hash("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let token = mint.mint().unwrap();
        assert_eq!(mint.hash(&token), mint.hash(&token));
        assert_ne!(mint.hash(&token), token);
        assert_eq!(mint.hash(&token).len(), 64);
    }
}
