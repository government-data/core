//! The installation's administrators, as the notices see them: whom to email
//! when a new version arrives or an update ends.

use std::sync::Arc;

/// Whom to tell.
#[async_trait::async_trait]
pub trait AdminContacts: Send + Sync {
    async fn admin_emails(&self) -> Result<Vec<String>, String>;
}

/// The installation's administrators.
pub struct PlatformAdmins(pub Arc<dyn platform::PlatformRepository>);

impl std::fmt::Debug for PlatformAdmins {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PlatformAdmins")
    }
}

#[async_trait::async_trait]
impl AdminContacts for PlatformAdmins {
    async fn admin_emails(&self) -> Result<Vec<String>, String> {
        self.0.admin_emails().await.map_err(|e| e.to_string())
    }
}
