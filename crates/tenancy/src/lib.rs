//! The tenant of the current task.
//!
//! In the online edition each company is a tenant. The API resolves it from
//! the subdomain and runs the request inside [`within`]; the Postgres pool
//! reads it with [`current`] when it lends each connection and sets it in
//! `app.tenant_id`, which is what the RLS policies filter on. A task launched
//! with [`spawn`] inherits the tenant of whoever launched it.
//!
//! On-premise has a single tenant, the main one, and needs none of this.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;

/// The on-premise tenant, and the one of the data from before tenants.
pub const MAIN_ID: &str = "00000000-0000-0000-0000-000000000001";

/// What is set when there is no tenant in the online edition: it matches
/// none, so nothing is seen or written.
pub const NONE_ID: &str = "00000000-0000-0000-0000-000000000000";

/// The subdomain of the shared entry (online edition): one types the email
/// and gets into their companies without knowing their address. It belongs
/// to none.
pub const ENTRY: &str = "entrar";

/// How many tenants the installation serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// On-premise: everything belongs to the main tenant.
    Single,
    /// Online: each request and each task brings its tenant.
    Multiple,
}

/// The tenant being worked with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tenant {
    pub id: String,
    /// The subdomain.
    pub slug: String,
}

impl Tenant {
    pub fn main() -> Self {
        Self {
            id: MAIN_ID.into(),
            slug: "main".into(),
        }
    }
}

/// Whether the company can use the service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Active,
    /// Unpaid or with the license revoked: read-only.
    Suspended,
    /// Deregistered: as if it did not exist.
    Closed,
}

impl Status {
    /// From its persisted name.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "active" => Some(Self::Active),
            "suspended" => Some(Self::Suspended),
            "closed" => Some(Self::Closed),
            _ => None,
        }
    }
}

/// A tenant of the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantRecord {
    pub tenant: Tenant,
    pub name: String,
    pub status: Status,
    /// The database (and the API replicas) where its data lives.
    pub cell: String,
}

/// The tenant registry.
#[async_trait]
pub trait Directory: Send + Sync {
    /// The tenant of a subdomain.
    async fn by_slug(&self, slug: &str) -> Result<Option<TenantRecord>, String>;
    /// The ones of a cell that are not deregistered (for background tasks).
    async fn in_cell(&self, cell: &str) -> Result<Vec<TenantRecord>, String>;
    /// The ones of a cell (not deregistered) with an account for that email
    /// that already chose its password: the shared entry (`entrar.<domain>`).
    async fn with_email(&self, _email: &str, _cell: &str) -> Result<Vec<TenantRecord>, String> {
        Ok(Vec::new())
    }
}

tokio::task_local! {
    static CURRENT: Tenant;
}

/// Run `f` inside a tenant.
pub async fn within<F: Future>(tenant: Tenant, f: F) -> F::Output {
    CURRENT.scope(tenant, f).await
}

/// The tenant of the current task, if any.
pub fn current() -> Option<Tenant> {
    CURRENT.try_with(Clone::clone).ok()
}

/// The key of the current tenant for what is kept in memory per company: its
/// id, or the main one if there is none (on-premise).
pub fn key() -> String {
    current().map_or_else(|| MAIN_ID.to_string(), |t| t.id)
}

/// A value in memory per tenant (each company's applied settings: its mail,
/// its directory…). Every read and write is the current tenant's ([`key`]): a
/// company never sees or overwrites another's.
pub struct PerTenant<T>(Arc<RwLock<HashMap<String, T>>>);

impl<T> Clone for PerTenant<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> std::fmt::Debug for PerTenant<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let n = self
            .0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len();
        f.debug_struct("PerTenant").field("tenants", &n).finish()
    }
}

impl<T> Default for PerTenant<T> {
    fn default() -> Self {
        Self(Arc::default())
    }
}

impl<T: Clone> PerTenant<T> {
    /// The current tenant's, if it has one.
    pub fn get(&self) -> Option<T> {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&key())
            .cloned()
    }

    /// Replace the current tenant's.
    pub fn set(&self, value: T) {
        self.0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key(), value);
    }

    /// Change it in place (starting from `T::default()` if there was none).
    pub fn update<R>(&self, f: impl FnOnce(&mut T) -> R) -> R
    where
        T: Default,
    {
        let mut map = self
            .0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(map.entry(key()).or_default())
    }

    /// Whether the current tenant already has its own.
    pub fn contains(&self) -> bool {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(&key())
    }
}

/// `tokio::spawn` that keeps the tenant of whoever launches the task.
pub fn spawn<F>(f: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    match current() {
        Some(tenant) => tokio::spawn(CURRENT.scope(tenant, f)),
        None => tokio::spawn(f),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acme() -> Tenant {
        Tenant {
            id: "7f3".into(),
            slug: "acme".into(),
        }
    }

    #[tokio::test]
    async fn each_tenant_keeps_its_own_value() {
        let value: PerTenant<String> = PerTenant::default();
        let a = Tenant {
            id: "a".into(),
            slug: "a".into(),
        };
        let b = Tenant {
            id: "b".into(),
            slug: "b".into(),
        };
        // Without a tenant it is the main one (on-premise).
        value.set("main".into());
        within(a.clone(), async { value.set("a's".into()) }).await;
        assert_eq!(
            within(a, async { value.get() }).await.as_deref(),
            Some("a's")
        );
        assert_eq!(within(b.clone(), async { value.get() }).await, None);
        assert!(!within(b, async { value.contains() }).await);
        assert_eq!(value.get().as_deref(), Some("main"));
        assert_eq!(
            within(Tenant::main(), async { value.get() })
                .await
                .as_deref(),
            Some("main")
        );
    }

    #[tokio::test]
    async fn the_tenant_is_only_set_inside_within() {
        assert_eq!(current(), None);
        let inside = within(acme(), async { current() }).await;
        assert_eq!(inside, Some(acme()));
        assert_eq!(current(), None);
    }

    #[tokio::test]
    async fn spawned_tasks_keep_the_tenant_but_plain_spawns_do_not() {
        let (inherited, lost) = within(acme(), async {
            (
                spawn(async { current() }).await.unwrap(),
                tokio::spawn(async { current() }).await.unwrap(),
            )
        })
        .await;
        assert_eq!(inherited, Some(acme()));
        assert_eq!(lost, None);
    }
}
