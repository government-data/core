//! What the license looks at: the plan's usage and whom to tell.

use super::*;

impl PgPlatformRepository {
    pub(super) async fn admin_emails(&self) -> R<Vec<String>> {
        sqlx::query_scalar(
            "SELECT email FROM admin_users WHERE role = 'admin' AND email LIKE '%@%' ORDER BY email",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)
    }

    /// Users who are not administrators. Nothing counts as a workflow or an
    /// execution until the quality pipelines exist (ADR 0016).
    pub(super) async fn usage(&self, _since: DateTime<Utc>) -> R<license_model::Usage> {
        let users: i64 =
            sqlx::query_scalar("SELECT count(*) FROM admin_users WHERE role <> 'admin'")
                .fetch_one(&self.pool)
                .await
                .map_err(db)?;
        Ok(license_model::Usage {
            users: u32::try_from(users).unwrap_or(u32::MAX),
            ..license_model::Usage::default()
        })
    }
}
