//! The database pool and the tenant of each request: everything belongs to
//! the main tenant (decision 0076: one installation is one organisation).

use async_trait::async_trait;
use infrastructure::PgConfig;
use quality::tenants::Tenants;

/// The pool and how the tenant of each request is found.
pub(crate) struct Tenancy {
    pub(crate) pool: infrastructure::PgPool,
    pub(crate) resolver: api::tenancy::TenantResolver,
}

/// Connect (eagerly, so a bad DSN or a down database fails now) and bring the
/// schema up to date. `run_migrations` also validates the checksums of the
/// migrations already applied, so schema drift is caught at startup.
pub(crate) async fn connect(pg_config: &PgConfig) -> Result<Tenancy, Box<dyn std::error::Error>> {
    let pool = infrastructure::connect(pg_config).await?;
    infrastructure::run_migrations(&pool).await?;
    Ok(Tenancy {
        pool,
        resolver: api::tenancy::TenantResolver::single(),
    })
}

/// The tenants the scheduler finds: the main one.
#[derive(Clone)]
pub(crate) struct CellTenants;

impl From<&crate::tenants::Tenancy> for CellTenants {
    fn from(_: &crate::tenants::Tenancy) -> Self {
        Self
    }
}

#[async_trait]
impl Tenants for CellTenants {
    async fn tenants(&self) -> Result<Vec<tenancy::Tenant>, String> {
        Ok(vec![tenancy::Tenant::main()])
    }
}
