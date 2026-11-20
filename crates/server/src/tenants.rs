//! The database pool and the tenant of each request, per edition.
//!
//! On-premise everything belongs to the main tenant. Online every request
//! brings its company (from the subdomain) and the pool sets it on each
//! connection; migrations run before, on a pool without a tenant.

use std::sync::Arc;

use application::Edition;
use infrastructure::PgConfig;

use crate::config::env_value;

/// The pool and how the tenants of this API are found.
pub(crate) struct Tenancy {
    pub(crate) pool: infrastructure::PgPool,
    /// The cell this API serves (`ORQ_CELDA`, `main` by default).
    pub(crate) cell: String,
    /// The directory of companies; `None` on-premise.
    pub(crate) directory: Option<Arc<dyn tenancy::Directory>>,
    /// The domain companies come in through (`<slug>.<domain>`); online only.
    pub(crate) domain: Option<String>,
    pub(crate) resolver: api::tenancy::TenantResolver,
}

/// Connect (eagerly, so a bad DSN or a down database fails now) and bring the
/// schema up to date. `run_migrations` also validates the checksums of the
/// migrations already applied, so schema drift is caught at startup.
pub(crate) async fn connect(
    edition: Edition,
    pg_config: &PgConfig,
) -> Result<Tenancy, Box<dyn std::error::Error>> {
    let mode = match edition {
        Edition::OnPremise => tenancy::Mode::Single,
        Edition::Online => tenancy::Mode::Multiple,
    };
    let pool = infrastructure::connect(pg_config).await?;
    infrastructure::run_migrations(&pool).await?;
    let cell = env_value("ORQ_CELDA").unwrap_or_else(|| "main".into());
    if mode == tenancy::Mode::Single {
        return Ok(Tenancy {
            pool,
            cell,
            directory: None,
            domain: None,
            resolver: api::tenancy::TenantResolver::single(),
        });
    }
    pool.close().await;
    let pool = infrastructure::connect_for(pg_config, mode).await?;
    let directory: Arc<dyn tenancy::Directory> =
        Arc::new(infrastructure::PgTenantDirectory::new(pool.clone()));
    let domain = env_value("ORQ_DOMINIO")
        .or_else(|| env_value("APP_DOMAIN"))
        .ok_or("online needs ORQ_DOMINIO: companies come in through <company>.<domain>")?;
    tracing::info!(%domain, %cell, "multiple tenants");
    let resolver = api::tenancy::TenantResolver::multiple(&domain, &cell, directory.clone());
    Ok(Tenancy {
        pool,
        cell,
        directory: Some(directory),
        domain: Some(domain),
        resolver,
    })
}
