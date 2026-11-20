//! Whom the notices go to: the administrators.

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
}
