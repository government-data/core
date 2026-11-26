//! The main lake's users whose keys are given, not made by the lake
//! (decision 0091, rulings 46 and 51): the Trino coordinator's
//! (`query-engine`, limited to the installation's warehouses), made when
//! its role's secrets reach the core, and the STS issuer's, made when the
//! API starts. Each is asked of the main lake's adapter with the keys
//! given, which it takes as they are; asking again changes nothing. No key
//! is logged or audited.

use std::collections::BTreeMap;

use auth_gateway::Lake;

use crate::lake::{LakeCredentialKind, LakeError, LakeKeys};

use super::MachinesService;

/// The role secrets that carry the query engine's lake keys, as the module
/// that runs the coordinator declares them (ruling 51).
pub const QUERY_ENGINE_ACCESS_KEY: &str = "lake-query-engine-access-key";
pub const QUERY_ENGINE_SECRET_KEY: &str = "lake-query-engine-secret-key";

/// The query engine's lake keys among a role's secrets, when it holds both.
pub fn query_engine_keys(secrets: &BTreeMap<String, String>) -> Option<LakeKeys> {
    let access_key = secrets.get(QUERY_ENGINE_ACCESS_KEY)?.trim();
    let secret_key = secrets.get(QUERY_ENGINE_SECRET_KEY)?.trim();
    (!access_key.is_empty() && !secret_key.is_empty()).then(|| LakeKeys {
        access_key: access_key.into(),
        secret_key: secret_key.into(),
    })
}

impl MachinesService {
    /// Make or update the main lake's user of `kind` with the `keys` given
    /// ([`LakeCredentialKind::keys_are_given`]).
    pub async fn ensure_lake_user(
        &self,
        kind: LakeCredentialKind,
        keys: &LakeKeys,
    ) -> Result<(), LakeError> {
        if !kind.keys_are_given() {
            return Err(LakeError::Invalid(format!("{kind:?}'s keys are not given")));
        }
        let Some(api) = &self.lake_api else {
            return Err(LakeError::Unavailable("no lake adapter".into()));
        };
        let admin = api.for_lake(&Lake::main(), "");
        admin.ensure_credentials(kind, Some(keys)).await.map(|_| ())
    }
}
