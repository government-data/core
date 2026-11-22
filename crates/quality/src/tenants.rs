//! The tenants of this server's cell: a port, as the module does not know
//! how the server finds them.

use async_trait::async_trait;
use tenancy::Tenant;

/// Port: the tenants the server serves (the server: the cell's, as the
/// scheduler finds them).
#[async_trait]
pub trait Tenants: Send + Sync {
    async fn tenants(&self) -> Result<Vec<Tenant>, String>;
}
