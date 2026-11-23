//! [`Es256Signer`]: the OIDC provider's second key, ECDSA P-256 (JWS
//! `alg: ES256`, RFC 7518 §3.4), for the relying parties that only verify RSA
//! or EC keys. The Ed25519 one ([`crate::oidc::Ed25519Signer`]) stays
//! the default; each application's settings choose.
//!
//! Also where the provider's keys are read from the environment: `NAME` or,
//! as for every secret, a file in `NAME_FILE`.

use auth_gateway::TokenSigner;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use ring::rand::SystemRandom;
use ring::signature::{
    EcdsaKeyPair, KeyPair, UnparsedPublicKey, ECDSA_P256_SHA256_FIXED,
    ECDSA_P256_SHA256_FIXED_SIGNING,
};
use serde_json::{json, Value};

use crate::oidc::kid_of;

/// The variable of the current ES256 key.
pub const ES256_KEY_VAR: &str = "OIDC_SIGNING_KEY_ES256";
/// The variable of the previous ES256 keys, while their tokens live.
pub const ES256_PREVIOUS_VAR: &str = "OIDC_SIGNING_KEY_ES256_PREVIOUS";

/// Signs with a P-256 key (PKCS#8, base64: `OIDC_SIGNING_KEY_ES256`, from
/// `server oidc-keygen --es256`). The signature is the fixed `r || s` form
/// JWS needs, not DER. The `kid` is derived from the public key.
///
/// Rotated like the Ed25519 key: the old one goes to
/// `OIDC_SIGNING_KEY_ES256_PREVIOUS`, still verifies and stays in the JWK Set.
pub struct Es256Signer {
    pair: EcdsaKeyPair,
    kid: String,
    rng: SystemRandom,
    /// Previous keys: (uncompressed public point, kid), to verify only.
    previous: Vec<(Vec<u8>, String)>,
}

impl std::fmt::Debug for Es256Signer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Es256Signer")
            .field("kid", &self.kid)
            .finish_non_exhaustive()
    }
}

impl Es256Signer {
    pub fn from_pkcs8_base64(private_key: &str) -> Result<Self, String> {
        let pkcs8 = STANDARD
            .decode(private_key.trim().as_bytes())
            .map_err(|_| format!("{ES256_KEY_VAR}: not base64"))?;
        let rng = SystemRandom::new();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &pkcs8, &rng)
            .map_err(|_| format!("{ES256_KEY_VAR}: not a P-256 PKCS#8 key"))?;
        let kid = kid_of(pair.public_key().as_ref());
        Ok(Self {
            pair,
            kid,
            rng,
            previous: Vec::new(),
        })
    }

    /// Also verify (and publish) these previous keys, PKCS#8 in base64.
    pub fn with_previous<'a>(
        mut self,
        keys: impl IntoIterator<Item = &'a str>,
    ) -> Result<Self, String> {
        for key in keys {
            let old = Self::from_pkcs8_base64(key)
                .map_err(|e| e.replace(ES256_KEY_VAR, ES256_PREVIOUS_VAR))?;
            if old.kid != self.kid && !self.previous.iter().any(|(_, k)| *k == old.kid) {
                self.previous
                    .push((old.pair.public_key().as_ref().to_vec(), old.kid));
            }
        }
        Ok(self)
    }

    /// A new private key, PKCS#8 in base64.
    pub fn generate() -> Result<String, String> {
        let pkcs8 =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &SystemRandom::new())
                .map_err(|_| "could not generate the key".to_string())?;
        Ok(STANDARD.encode(pkcs8.as_ref()))
    }

    /// The public keys it verifies with, the current one first.
    fn public_keys(&self) -> impl Iterator<Item = (&[u8], &str)> {
        std::iter::once((self.pair.public_key().as_ref(), self.kid.as_str())).chain(
            self.previous
                .iter()
                .map(|(key, kid)| (key.as_slice(), kid.as_str())),
        )
    }
}

impl TokenSigner for Es256Signer {
    fn sign(&self, typ: &str, claims: &str) -> String {
        let header = json!({ "alg": "ES256", "typ": typ, "kid": self.kid });
        let signing_input = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(header.to_string()),
            URL_SAFE_NO_PAD.encode(claims)
        );
        // Fails only if the system's randomness does, which nothing survives.
        let signature = self
            .pair
            .sign(&self.rng, signing_input.as_bytes())
            .expect("ECDSA signing needs the system's randomness");
        format!(
            "{signing_input}.{}",
            URL_SAFE_NO_PAD.encode(signature.as_ref())
        )
    }

    fn verify(&self, typ: &str, token: &str) -> Option<String> {
        let mut parts = token.split('.');
        let (header, claims, signature) = (parts.next()?, parts.next()?, parts.next()?);
        if parts.next().is_some() {
            return None;
        }
        let decoded: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(header).ok()?).ok()?;
        if decoded.get("alg")? != "ES256" || decoded.get("typ")? != typ {
            return None;
        }
        let kid = decoded.get("kid")?.as_str()?;
        let (key, _) = self.public_keys().find(|(_, k)| *k == kid)?;
        let signature = URL_SAFE_NO_PAD.decode(signature).ok()?;
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, key)
            .verify(format!("{header}.{claims}").as_bytes(), &signature)
            .ok()?;
        String::from_utf8(URL_SAFE_NO_PAD.decode(claims).ok()?).ok()
    }

    fn algorithm(&self) -> &'static str {
        "ES256"
    }

    fn jwks(&self) -> String {
        let keys: Vec<Value> = self
            .public_keys()
            .filter_map(|(key, kid)| {
                // Uncompressed point: 0x04 || x (32 bytes) || y (32 bytes).
                let (x, y) = key.strip_prefix(&[4])?.split_at_checked(32)?;
                Some(json!({
                    "kty": "EC",
                    "crv": "P-256",
                    "x": URL_SAFE_NO_PAD.encode(x),
                    "y": URL_SAFE_NO_PAD.encode(y),
                    "kid": kid,
                    "alg": "ES256",
                    "use": "sig",
                }))
            })
            .collect();
        json!({ "keys": keys }).to_string()
    }
}

/// A key from `name` or, without it, from the file in `name_FILE` (a
/// trailing newline trimmed). `None` if neither is set; an unreadable file
/// is an error, not a missing key.
pub fn key_from_env(name: &str) -> Result<Option<String>, String> {
    if let Some(value) = std::env::var(name).ok().filter(|v| !v.trim().is_empty()) {
        return Ok(Some(value));
    }
    let file_var = format!("{name}_FILE");
    match std::env::var(&file_var)
        .ok()
        .filter(|p| !p.trim().is_empty())
    {
        None => Ok(None),
        Some(path) => std::fs::read_to_string(&path)
            .map(|k| Some(k.trim().to_string()).filter(|k| !k.is_empty()))
            .map_err(|e| format!("{file_var} {path}: {e}")),
    }
}

/// Keys separated by commas or whitespace, as the `*_PREVIOUS` variables
/// hold them.
pub fn split_keys(keys: &str) -> Vec<String> {
    keys.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|k| !k.is_empty())
        .map(str::to_string)
        .collect()
}

/// The ES256 signer, if `OIDC_SIGNING_KEY_ES256` (or its `_FILE`) is set,
/// with the previous keys of `OIDC_SIGNING_KEY_ES256_PREVIOUS`.
pub fn es256_signer_from_env() -> Result<Option<Es256Signer>, String> {
    let Some(key) = key_from_env(ES256_KEY_VAR)? else {
        return Ok(None);
    };
    let previous = split_keys(&key_from_env(ES256_PREVIOUS_VAR)?.unwrap_or_default());
    Es256Signer::from_pkcs8_base64(&key)?
        .with_previous(previous.iter().map(String::as_str))
        .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signer() -> Es256Signer {
        Es256Signer::from_pkcs8_base64(&Es256Signer::generate().unwrap()).unwrap()
    }

    /// Verify `token` with the JWK of the set under its `kid`, as a relying
    /// party would: from `x` and `y` alone.
    fn verifies_with_jwks(jwks: &str, token: &str) -> bool {
        let jwks: Value = serde_json::from_str(jwks).unwrap();
        let header: Value = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(token.split('.').next().unwrap())
                .unwrap(),
        )
        .unwrap();
        let Some(key) = jwks["keys"]
            .as_array()
            .unwrap()
            .iter()
            .find(|k| k["kid"] == header["kid"])
        else {
            return false;
        };
        assert_eq!(key["kty"], "EC");
        assert_eq!(key["crv"], "P-256");
        assert_eq!(key["alg"], "ES256");
        assert_eq!(key["use"], "sig");
        let mut point = vec![4u8];
        for c in ["x", "y"] {
            let coord = URL_SAFE_NO_PAD.decode(key[c].as_str().unwrap()).unwrap();
            assert_eq!(coord.len(), 32);
            point.extend(coord);
        }
        let (input, sig) = token.rsplit_once('.').unwrap();
        let sig = URL_SAFE_NO_PAD.decode(sig).unwrap();
        // The fixed form JWS asks for: r || s, 64 bytes.
        assert_eq!(sig.len(), 64);
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, point)
            .verify(input.as_bytes(), &sig)
            .is_ok()
    }

    #[test]
    fn signs_verifies_and_the_jwks_key_alone_verifies_too() {
        let s = signer();
        let token = s.sign("at+jwt", r#"{"sub":"ana"}"#);
        let header: Value = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(token.split('.').next().unwrap())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(header["alg"], "ES256");
        assert_eq!(
            s.verify("at+jwt", &token).as_deref(),
            Some(r#"{"sub":"ana"}"#)
        );
        assert!(verifies_with_jwks(&s.jwks(), &token));
        // Another type, another key or a touched payload do not verify.
        assert!(s.verify("JWT", &token).is_none());
        assert!(signer().verify("at+jwt", &token).is_none());
        let mut parts: Vec<&str> = token.split('.').collect();
        let forged = URL_SAFE_NO_PAD.encode(r#"{"sub":"eva"}"#);
        parts[1] = &forged;
        assert!(s.verify("at+jwt", &parts.join(".")).is_none());
        // An EdDSA token is not taken for one of these.
        let ed = crate::oidc::Ed25519Signer::from_pkcs8_base64(
            &crate::oidc::Ed25519Signer::generate().unwrap(),
        )
        .unwrap();
        assert!(s.verify("JWT", &ed.sign("JWT", "{}")).is_none());
        assert!(
            Es256Signer::from_pkcs8_base64(&crate::oidc::Ed25519Signer::generate().unwrap())
                .is_err()
        );
    }

    #[test]
    fn a_rotated_key_still_verifies_and_is_published() {
        let old_key = Es256Signer::generate().unwrap();
        let old = Es256Signer::from_pkcs8_base64(&old_key).unwrap();
        let signed_before = old.sign("JWT", r#"{"a":1}"#);
        let new = signer().with_previous([old_key.as_str()]).unwrap();
        assert_eq!(
            new.verify("JWT", &signed_before).as_deref(),
            Some(r#"{"a":1}"#)
        );
        let signed_after = new.sign("JWT", "{}");
        assert!(old.verify("JWT", &signed_after).is_none());
        let jwks = new.jwks();
        let parsed: Value = serde_json::from_str(&jwks).unwrap();
        assert_eq!(parsed["keys"].as_array().unwrap().len(), 2);
        assert_eq!(parsed["keys"][0]["kid"], new.kid.as_str());
        assert!(verifies_with_jwks(&jwks, &signed_before));
        assert!(verifies_with_jwks(&jwks, &signed_after));
        assert!(signer().verify("JWT", &signed_before).is_none());
    }

    #[test]
    fn previous_keys_split_on_commas_and_whitespace() {
        assert_eq!(split_keys(" a, b\nc,,"), vec!["a", "b", "c"]);
        assert!(split_keys("").is_empty());
    }
}
