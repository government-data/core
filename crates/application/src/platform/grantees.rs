//! [`contracts::Grantees`] from the platform's IAM (decision 0089, step
//! 5): every account that is active (a local password or the directory's;
//! not one still invited), its actor built as for a request, asked of each
//! resource. Nothing is cached here beyond the actors the platform keeps.

use std::sync::Arc;

use async_trait::async_trait;
use auth_gateway::{AccountStatus, AdminRepository};
use contracts::{Grantee, Grantees};

use super::PlatformService;

/// The adapter: the accounts and the platform that builds their actors.
#[derive(Clone)]
pub struct PlatformGrantees {
    accounts: Arc<dyn AdminRepository>,
    platform: PlatformService,
}

impl std::fmt::Debug for PlatformGrantees {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlatformGrantees").finish_non_exhaustive()
    }
}

impl PlatformGrantees {
    pub fn new(accounts: Arc<dyn AdminRepository>, platform: PlatformService) -> Self {
        Self { accounts, platform }
    }
}

#[async_trait]
impl Grantees for PlatformGrantees {
    async fn allowed(&self, action: &str, resources: &[String]) -> Result<Vec<Grantee>, String> {
        let accounts = self
            .accounts
            .list_accounts()
            .await
            .map_err(|e| format!("the accounts: {e:?}"))?;
        let mut out = Vec::new();
        for account in accounts
            .into_iter()
            .filter(|a| a.status != AccountStatus::Pending)
        {
            let id = account.id.as_str();
            let actor = self
                .platform
                .actor(id, &account.role, None)
                .await
                .map_err(|e| format!("an account's permissions: {e:?}"))?;
            let held: Vec<String> = resources
                .iter()
                .filter(|r| actor.can(action, r))
                .cloned()
                .collect();
            if !held.is_empty() {
                out.push(Grantee {
                    account: id.to_owned(),
                    resources: held,
                });
            }
        }
        Ok(out)
    }
}
