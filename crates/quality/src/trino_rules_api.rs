//! The route Trino reads its access-control rules from (decision 0062,
//! point 6): `GET /internal/trino/rules`. Trino's coordinator is known by
//! its machine certificate (decision 0066, point 9), on Traefik's internal
//! entry point, through [`MachineCertificates`]; in development, until its
//! stack has certificates, by a secret in the query (`?key=`), compared in
//! constant time, only when one is set (`QUALITY_TRINO_RULES_KEY`, which
//! production does not set). With neither, 404. The rules are
//! [`crate::trino_rules`]' for every tenant's datasets whose mapping on
//! `main` reads a source on Trino. A tenant whose catalog does not answer
//! gives none: the rest is served, with fewer rights and never more, and
//! always as 200, since Trino quotes the whole URL, the secret too, to its
//! clients when the answer is another status.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use sqlx::PgPool;
use tenancy::Tenant;
use uuid::Uuid;

use crate::catalog::Catalog;
use crate::machine_certs::{MachineCertificates, TRINO_COORDINATOR};
use crate::mapping::Mapping;
use crate::source_store;
use crate::trino_rules::{self, Extracted};

/// The route's path, as the API serves it.
pub const PATH: &str = "/internal/trino/rules";

/// The fewest characters a secret has.
const KEY_MIN: usize = 32;

/// The secret Trino sends in the query. Never printed.
#[derive(Clone, PartialEq, Eq)]
pub struct RulesKey(String);

impl RulesKey {
    /// At least 32 characters of `A-Z`, `a-z`, `0-9`, `-` and `_`, which a
    /// URL's query carries as they are.
    pub fn parse(key: &str) -> Result<Self, String> {
        let plain = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
        if key.len() < KEY_MIN || !key.chars().all(plain) {
            return Err(format!(
                "at least {KEY_MIN} characters of A-Z, a-z, 0-9, - and _"
            ));
        }
        Ok(Self(key.to_string()))
    }

    /// Whether `given` is the secret, in time that does not depend on
    /// where they differ.
    fn is(&self, given: &str) -> bool {
        let (a, b) = (self.0.as_bytes(), given.as_bytes());
        let mut diff = u8::from(a.len() != b.len());
        for (i, x) in a.iter().enumerate() {
            diff |= x ^ b.get(i).copied().unwrap_or(0);
        }
        std::hint::black_box(diff) == 0
    }
}

impl fmt::Debug for RulesKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RulesKey(…)")
    }
}

/// Port: the tenants whose datasets Trino serves (the server: the cell's,
/// as the scheduler finds them).
#[async_trait]
pub trait Tenants: Send + Sync {
    async fn tenants(&self) -> Result<Vec<Tenant>, String>;
}

/// What the route needs besides the catalog and the sources: who may read
/// the rules, by certificate or by key, and whose they are.
#[derive(Clone)]
pub struct RulesAccess {
    /// The development's secret in the query; production sets none.
    pub key: Option<RulesKey>,
    /// Trino's coordinator by its certificate; `None` where the certificate
    /// header is not trusted.
    pub machines: Option<Arc<dyn MachineCertificates>>,
    /// The API's own client in Trino (`qd-api`), the owner.
    pub client: String,
    pub tenants: Arc<dyn Tenants>,
}

impl fmt::Debug for RulesAccess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RulesAccess")
            .field("key", &self.key.is_some())
            .field("machines", &self.machines.is_some())
            .field("client", &self.client)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
struct RulesApi {
    pool: PgPool,
    catalog: Arc<dyn Catalog>,
    access: RulesAccess,
}

/// `GET /internal/trino/rules`, with Trino's certificate or the secret.
pub fn routes(pool: PgPool, catalog: Arc<dyn Catalog>, access: RulesAccess) -> Router {
    Router::new().route(PATH, get(rules)).with_state(RulesApi {
        pool,
        catalog,
        access,
    })
}

/// The `key` of a query, if it has one.
fn key_of(query: &str) -> Option<&str> {
    query.split('&').find_map(|pair| pair.strip_prefix("key="))
}

async fn rules(
    State(api): State<RulesApi>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if !api.access.allows(query.as_deref(), &headers).await {
        return Err(StatusCode::NOT_FOUND);
    }
    let tenants = api.access.tenants.tenants().await.unwrap_or_else(|e| {
        tracing::error!("trino rules: the tenants: {e}");
        Vec::new()
    });
    let mut extracted = Vec::new();
    for tenant in tenants {
        let id = tenant.id.clone();
        match tenancy::within(tenant, api.extracted(&id)).await {
            Ok(found) => extracted.extend(found),
            Err(e) => tracing::error!(tenant = %id, "trino rules: no rules for its datasets: {e}"),
        }
    }
    Ok(Json(trino_rules::rules(&api.access.client, &extracted)))
}

impl RulesAccess {
    /// Whether the caller is Trino's coordinator: the secret in `query`,
    /// when one is set to compare it with, or an accepted certificate of
    /// the coordinator's role.
    async fn allows(&self, query: Option<&str>, headers: &HeaderMap) -> bool {
        let given = query.and_then(key_of).unwrap_or_default();
        if self.key.as_ref().is_some_and(|key| key.is(given)) {
            return true;
        }
        match &self.machines {
            Some(machines) => machines.machine(headers, TRINO_COORDINATOR).await.is_some(),
            None => false,
        }
    }
}

impl RulesApi {
    /// The current tenant's datasets whose mapping reads a source on
    /// Trino. A mapping that does not parse, or names no source or one
    /// that is not there, gives none.
    async fn extracted(&self, tenant: &str) -> Result<Vec<Extracted>, String> {
        let tenant = Uuid::parse_str(tenant).map_err(|e| e.to_string())?;
        let mut found = Vec::new();
        for mapping in self.catalog.mappings(tenant).await? {
            let Some((dataset, source)) = Mapping::parse(&mapping.yaml)
                .ok()
                .and_then(|m| Some((m.dataset.value, m.from?.value.source)))
            else {
                continue;
            };
            let driver = source_store::driver_of(&self.pool, &source)
                .await
                .map_err(|e| e.to_string())?;
            if driver.as_deref().is_some_and(read_by_trino) {
                found.push(Extracted { dataset, source });
            }
        }
        Ok(found)
    }
}

/// Whether Trino reads a source of `driver`: never since decision 0077,
/// every driver is DuckDB's. This route goes with the rest of Trino.
fn read_by_trino(_driver: &str) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "synthetic-key-0123456789-abcdefghij";

    #[test]
    fn a_secret_is_long_plain_and_never_printed() {
        let key = RulesKey::parse(KEY).unwrap();
        assert!(!format!("{key:?}").contains("synthetic"));
        for bad in [
            "",
            "short",
            "synthetic key 0123456789 abcdefghij",
            &"é".repeat(40),
        ] {
            assert!(RulesKey::parse(bad).is_err(), "{bad}");
        }
        assert!(RulesKey::parse(&"a?".repeat(20)).is_err());
    }

    #[test]
    fn only_the_whole_secret_is_the_secret() {
        let key = RulesKey::parse(KEY).unwrap();
        assert!(key.is(KEY));
        let longer = format!("{KEY}x");
        for other in ["", &KEY[..KEY.len() - 1], &longer, &KEY.replace('0', "1")] {
            assert!(!key.is(other), "{other}");
        }
    }

    #[test]
    fn the_key_is_read_from_the_query() {
        assert_eq!(key_of("key=abc"), Some("abc"));
        assert_eq!(key_of("x=1&key=abc&y=2"), Some("abc"));
        assert_eq!(key_of("monkey=abc"), None);
        assert_eq!(key_of(""), None);
    }
}
