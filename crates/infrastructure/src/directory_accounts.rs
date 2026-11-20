//! Postgres adapter for the [`DirectoryAccounts`] port.
//!
//! The account lives in `admin_users` (keyed by email, with the
//! directory-managed password marker); what it may do comes from the IAM
//! groups its directory groups feed (`iam_groups.directory_groups`, ADR 0016).

use async_trait::async_trait;
use auth_gateway::{AdminAccount, DirectoryAccounts, DirectoryUser, PasswordHash};
use domain::{AdminId, RepositoryError, Role};
use sqlx::postgres::PgPool;

use crate::admin_repo::{backend, row_to_account};

/// The adapter.
#[derive(Clone)]
pub struct PgDirectoryAccounts {
    pool: PgPool,
}

impl std::fmt::Debug for PgDirectoryAccounts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgDirectoryAccounts")
            .finish_non_exhaustive()
    }
}

impl PgDirectoryAccounts {
    /// Build the adapter over the pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl DirectoryAccounts for PgDirectoryAccounts {
    async fn grants_access(&self, groups: &[String]) -> Result<bool, RepositoryError> {
        let lowered: Vec<String> = groups.iter().map(|g| g.to_lowercase()).collect();
        sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM iam_groups WHERE directory_groups && $1::text[])",
        )
        .bind(lowered)
        .fetch_one(&self.pool)
        .await
        .map_err(backend)
    }

    async fn upsert(
        &self,
        user: &DirectoryUser,
        role: &Role,
    ) -> Result<AdminAccount, RepositoryError> {
        let marker = PasswordHash::directory_managed();
        // The `WHERE` on the update makes a local (argon2) account with the
        // same email untouchable from the directory: no row comes back, and
        // that is reported as `EmailTaken` instead of silently taking it over.
        let row = sqlx::query(
            "INSERT INTO admin_users (email, password_hash, role, display_name)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (tenant_id, email) DO UPDATE
                SET role = EXCLUDED.role,
                    display_name = EXCLUDED.display_name,
                    updated_at = now()
              WHERE admin_users.password_hash = EXCLUDED.password_hash
             RETURNING id::text AS id, email, password_hash, failed_attempts, role, display_name,
                       EXTRACT(EPOCH FROM locked_until)::bigint AS locked_until_epoch",
        )
        .bind(user.email.as_str())
        .bind(marker.as_str())
        .bind(role.as_str())
        .bind(user.display_name.as_deref())
        .fetch_optional(&self.pool)
        .await
        .map_err(backend)?
        .ok_or(RepositoryError::EmailTaken)?;
        row_to_account(&row)?
            .ok_or_else(|| RepositoryError::Backend("upsert returned no row".into()))
    }

    /// Replace the account's directory memberships with the IAM groups its
    /// directory groups feed; those added by hand stay.
    async fn sync_groups(&self, id: &AdminId, groups: &[String]) -> Result<(), RepositoryError> {
        let lowered: Vec<String> = groups.iter().map(|g| g.to_lowercase()).collect();
        let mut tx = self.pool.begin().await.map_err(backend)?;
        sqlx::query(
            "DELETE FROM iam_group_members WHERE admin_id = $1::uuid AND source = 'directory'",
        )
        .bind(id.as_str())
        .execute(&mut *tx)
        .await
        .map_err(backend)?;
        sqlx::query(
            "INSERT INTO iam_group_members (group_id, admin_id, source)
             SELECT g.id, $1::uuid, 'directory' FROM iam_groups g
              WHERE g.directory_groups && $2::text[]",
        )
        .bind(id.as_str())
        .bind(lowered)
        .execute(&mut *tx)
        .await
        .map_err(backend)?;
        tx.commit().await.map_err(backend)
    }
}
