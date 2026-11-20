//! The provider's keys, one [`TokenSigner`] per algorithm: each application's
//! tokens are signed with the one its settings name, any of them verifies,
//! and the JWK Set and the discovery document list them all.

use std::sync::Arc;

use serde_json::Value;

use super::TokenError;
use crate::domain::{OidcClient, SigningAlg, TokenSigner};

#[derive(Clone)]
pub(super) struct Signers(Vec<Arc<dyn TokenSigner>>);

impl Signers {
    pub(super) fn new(first: Arc<dyn TokenSigner>) -> Self {
        Self(vec![first])
    }

    /// Add a signer; one of the same algorithm is replaced.
    pub(super) fn add(&mut self, signer: Arc<dyn TokenSigner>) {
        self.0.retain(|s| s.algorithm() != signer.algorithm());
        self.0.push(signer);
    }

    pub(super) fn get(&self, alg: SigningAlg) -> Option<&dyn TokenSigner> {
        self.0
            .iter()
            .find(|s| s.algorithm() == alg.as_str())
            .map(|s| s.as_ref())
    }

    /// The signer of `client`'s tokens. If its key is not configured, the
    /// application cannot get tokens: an operator's mistake, said plainly.
    pub(super) fn for_client(&self, client: &OidcClient) -> Result<&dyn TokenSigner, TokenError> {
        let alg = client.settings.token_alg;
        self.get(alg).ok_or_else(|| {
            TokenError::Internal(format!(
                "client {} signs with {} and that key is not configured \
                 (OIDC_SIGNING_KEY_ES256)",
                client.id,
                alg.as_str()
            ))
        })
    }

    /// The claims of a token any of the keys issued.
    pub(super) fn verify(&self, typ: &str, token: &str) -> Option<String> {
        self.0.iter().find_map(|s| s.verify(typ, token))
    }

    /// The algorithms there is a key for, the default one first.
    pub(super) fn algorithms(&self) -> Vec<&'static str> {
        self.0.iter().map(|s| s.algorithm()).collect()
    }

    /// One JWK Set with every key.
    pub(super) fn jwks(&self) -> String {
        let keys: Vec<Value> = self
            .0
            .iter()
            .filter_map(|s| serde_json::from_str::<Value>(&s.jwks()).ok())
            .filter_map(|set| set.get("keys").and_then(Value::as_array).cloned())
            .flatten()
            .collect();
        serde_json::json!({ "keys": keys }).to_string()
    }
}
