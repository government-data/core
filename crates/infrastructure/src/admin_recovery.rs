//! After restoring, on a new server, the backup of a company that was
//! deregistered: the administrator support named signs in (with a new
//! password), the other local accounts have to choose theirs again («¿Olvidaste
//! tu contraseña?»), and no session, recovery link or linked external account
//! (Google, Entra ID…) is left. So whoever caused the deregistration does not
//! get back in with what they had.

use auth_gateway::PasswordHash;
use domain::Email;
use sqlx::PgPool;

/// What was done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminRecovery {
    /// The administrator did not exist and was created.
    pub created: bool,
    /// Local accounts that have to choose their password again.
    pub accounts_to_reset: u64,
    pub sessions_closed: u64,
    pub links_removed: u64,
}

/// All in one transaction: either it ends like this, or as it was.
pub async fn recover_admin(
    pool: &PgPool,
    email: &Email,
    hash: &PasswordHash,
) -> Result<AdminRecovery, sqlx::Error> {
    let mut tx = pool.begin().await?;
    // Directory accounts stay with the directory; pending ones already are.
    let accounts = sqlx::query(
        "UPDATE admin_users SET password_hash = $2, failed_attempts = 0, locked_until = NULL,
                updated_at = now()
          WHERE email <> $1 AND password_hash NOT IN ('!directory', '!pending')",
    )
    .bind(email.as_str())
    .bind(PasswordHash::pending_activation().as_str())
    .execute(&mut *tx)
    .await?
    .rows_affected();
    let created: bool = sqlx::query_scalar(
        "INSERT INTO admin_users (email, password_hash, role) VALUES ($1, $2, 'admin')
         ON CONFLICT (tenant_id, email) DO UPDATE
             SET password_hash = EXCLUDED.password_hash, role = 'admin',
                 failed_attempts = 0, locked_until = NULL, updated_at = now()
         RETURNING (xmax = 0)",
    )
    .bind(email.as_str())
    .bind(hash.as_str())
    .fetch_one(&mut *tx)
    .await?;
    let sessions = sqlx::query("DELETE FROM admin_sessions")
        .execute(&mut *tx)
        .await?
        .rows_affected();
    sqlx::query("DELETE FROM admin_password_resets")
        .execute(&mut *tx)
        .await?;
    let links = sqlx::query("DELETE FROM admin_oauth_identities")
        .execute(&mut *tx)
        .await?
        .rows_affected();
    tx.commit().await?;
    Ok(AdminRecovery {
        created,
        accounts_to_reset: accounts,
        sessions_closed: sessions,
        links_removed: links,
    })
}
