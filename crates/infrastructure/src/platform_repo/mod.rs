//! The Postgres adapter of the [`PlatformRepository`] port.
//!
//! Identifiers cross the boundary as text (`id::text` when reading,
//! `$n::uuid` when writing), as in the rest of the adapters, so the domain
//! does not depend on a UUID library. A malformed identifier reaches Postgres
//! as an invalid cast and is reported as `NotFound`: for whoever asks, an id
//! that is not a UUID is an id that does not exist.
//!
//! Each topic lives in its module as inherent methods; the trait
//! implementation here only hands each call to them.

use std::collections::HashMap;

use ::iam::Policy;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use platform::{
    Group, GroupFields, NewSecret, NewServiceAccount, PlatformRepository, PrincipalKind,
    RepositoryError, Secret, SecretId, ServiceAccount, UserAccess,
};
use serde_json::Value;
use sqlx::postgres::{PgPool, PgRow};
use sqlx::Row;

mod license;
mod policies;
mod secrets;

type R<T> = Result<T, RepositoryError>;

/// Translate a sqlx error into the port's vocabulary.
fn db(err: sqlx::Error) -> RepositoryError {
    if let sqlx::Error::Database(e) = &err {
        match e.code().as_deref() {
            // invalid_text_representation: an id that is not a UUID.
            Some("22P02") => return RepositoryError::NotFound,
            Some("23505") => {
                return RepositoryError::Conflict("a record with that name already exists".into())
            }
            Some("23503") => {
                return RepositoryError::Conflict(
                    "the record is referenced or references something that does not exist".into(),
                )
            }
            _ => {}
        }
    }
    RepositoryError::Backend(err.to_string())
}

fn affected(rows: u64) -> R<()> {
    if rows == 0 {
        Err(RepositoryError::NotFound)
    } else {
        Ok(())
    }
}

/// The adapter.
#[derive(Debug, Clone)]
pub struct PgPlatformRepository {
    pool: PgPool,
}

impl PgPlatformRepository {
    /// Build on an existing pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl PlatformRepository for PgPlatformRepository {
    async fn list_secrets(&self, scope: &iam::Scope) -> R<Vec<Secret>> {
        PgPlatformRepository::list_secrets(self, scope).await
    }
    async fn create_secret(&self, secret: &NewSecret) -> R<Secret> {
        PgPlatformRepository::create_secret(self, secret).await
    }
    async fn update_secret(
        &self,
        id: &SecretId,
        description: Option<&str>,
        sealed: Option<(&str, &Value)>,
    ) -> R<Secret> {
        PgPlatformRepository::update_secret(self, id, description, sealed).await
    }
    async fn delete_secret(&self, id: &SecretId) -> R<()> {
        PgPlatformRepository::delete_secret(self, id).await
    }
    async fn secret_values(&self, ids: &[SecretId]) -> R<HashMap<SecretId, String>> {
        PgPlatformRepository::secret_values(self, ids).await
    }

    async fn list_policies(&self) -> R<Vec<Policy>> {
        PgPlatformRepository::list_policies(self).await
    }
    async fn put_policy(&self, policy: &Policy) -> R<()> {
        PgPlatformRepository::put_policy(self, policy).await
    }
    async fn delete_policy(&self, name: &str) -> R<()> {
        PgPlatformRepository::delete_policy(self, name).await
    }
    async fn policy_usage(&self) -> R<HashMap<String, i64>> {
        PgPlatformRepository::policy_usage(self).await
    }
    async fn attached(&self, kind: PrincipalKind, id: &str) -> R<Vec<String>> {
        PgPlatformRepository::attached(self, kind, id).await
    }
    async fn attach(&self, kind: PrincipalKind, id: &str, policies: &[String]) -> R<()> {
        PgPlatformRepository::attach(self, kind, id, policies).await
    }
    async fn account_policies(&self, admin_id: &str) -> R<Vec<String>> {
        PgPlatformRepository::account_policies(self, admin_id).await
    }
    async fn user_access(&self) -> R<Vec<UserAccess>> {
        PgPlatformRepository::user_access(self).await
    }

    async fn list_groups(&self) -> R<Vec<Group>> {
        PgPlatformRepository::list_groups(self).await
    }
    async fn create_group(&self, fields: &GroupFields) -> R<String> {
        PgPlatformRepository::create_group(self, fields).await
    }
    async fn update_group(&self, id: &str, fields: &GroupFields) -> R<()> {
        PgPlatformRepository::update_group(self, id, fields).await
    }
    async fn delete_group(&self, id: &str) -> R<()> {
        PgPlatformRepository::delete_group(self, id).await
    }
    async fn set_group_members(&self, id: &str, admin_ids: &[String]) -> R<()> {
        PgPlatformRepository::set_group_members(self, id, admin_ids).await
    }

    async fn list_service_accounts(&self, owner: Option<&str>) -> R<Vec<ServiceAccount>> {
        PgPlatformRepository::list_service_accounts(self, owner).await
    }
    async fn create_service_account(&self, account: &NewServiceAccount) -> R<String> {
        PgPlatformRepository::create_service_account(self, account).await
    }
    async fn delete_service_account(&self, id: &str, owner: Option<&str>) -> R<()> {
        PgPlatformRepository::delete_service_account(self, id, owner).await
    }

    async fn usage(&self, since: DateTime<Utc>) -> R<license_model::Usage> {
        PgPlatformRepository::usage(self, since).await
    }
    async fn admin_emails(&self) -> R<Vec<String>> {
        PgPlatformRepository::admin_emails(self).await
    }
}
