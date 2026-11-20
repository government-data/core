//! Service accounts' tokens (ADR 0016): `<access key>.<secret>`. The access
//! key is public and finds the account; only the SHA-256 of the secret (32
//! random bytes, so a fast hash is enough) is stored.

use argon2::password_hash::rand_core::{OsRng, RngCore};
use async_trait::async_trait;
use auth_gateway::{ServicePrincipal, ServiceTokens};
use domain::{AdminId, RepositoryError, Role};
use platform::{MintedToken, TokenMint};
use sqlx::postgres::PgPool;
use sqlx::Row;

use crate::tokens::{random_hex, sha256_hex};

/// What every access key starts with, so a leaked token is recognisable.
const PREFIX: &str = "CAL";

fn access_key() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    let tail: String = bytes
        .iter()
        .map(|b| ALPHABET[(*b as usize) % ALPHABET.len()] as char)
        .collect();
    format!("{PREFIX}{tail}")
}

/// Makes the tokens with the operating system's CSPRNG.
#[derive(Debug, Default, Clone, Copy)]
pub struct ServiceTokenMint;

impl TokenMint for ServiceTokenMint {
    fn mint(&self) -> MintedToken {
        let access_key = access_key();
        let secret = random_hex();
        MintedToken {
            secret_hash: sha256_hex(&secret),
            token: format!("{access_key}.{secret}"),
            access_key,
        }
    }
}

/// Checks the tokens against `service_accounts`, in the request's tenant.
#[derive(Debug, Clone)]
pub struct PgServiceTokens {
    pool: PgPool,
}

impl PgServiceTokens {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ServiceTokens for PgServiceTokens {
    async fn verify(&self, token: &str) -> Result<Option<ServicePrincipal>, RepositoryError> {
        let Some((key, secret)) = token.split_once('.') else {
            return Ok(None);
        };
        if !key.starts_with(PREFIX) || secret.is_empty() {
            return Ok(None);
        }
        // Expired ones, and those whose owner is locked or never activated,
        // do not count.
        let row = sqlx::query(
            "SELECT s.id::text AS id, s.secret_hash, u.id::text AS owner_id, u.role
               FROM service_accounts s JOIN admin_users u ON u.id = s.owner_id
              WHERE s.access_key = $1
                AND (s.expires_at IS NULL OR s.expires_at > now())
                AND (u.locked_until IS NULL OR u.locked_until <= now())",
        )
        .bind(key)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| RepositoryError::Backend(e.to_string()))?;
        let Some(row) = row else {
            return Ok(None);
        };
        let stored: String = row
            .try_get("secret_hash")
            .map_err(|e| RepositoryError::Backend(e.to_string()))?;
        if !constant_time_eq(stored.as_bytes(), sha256_hex(secret).as_bytes()) {
            return Ok(None);
        }
        let get = |name: &str| -> Result<String, RepositoryError> {
            row.try_get(name)
                .map_err(|e| RepositoryError::Backend(e.to_string()))
        };
        let id = get("id")?;
        // When it was last used, without failing the request over it.
        let _ = sqlx::query("UPDATE service_accounts SET last_used_at = now() WHERE id = $1::uuid")
            .bind(&id)
            .execute(&self.pool)
            .await;
        Ok(Some(ServicePrincipal {
            service_account_id: id,
            owner_id: AdminId::new(get("owner_id")?),
            owner_role: Role::parse(&get("role")?)
                .map_err(|e| RepositoryError::Backend(e.to_string()))?,
        }))
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minted_tokens_carry_their_key_and_the_hash_of_their_secret() {
        let m = ServiceTokenMint.mint();
        assert!(m.access_key.starts_with(PREFIX) && m.access_key.len() == 19);
        let (key, secret) = m.token.split_once('.').unwrap();
        assert_eq!(key, m.access_key);
        assert_eq!(sha256_hex(secret), m.secret_hash);
        assert_ne!(ServiceTokenMint.mint().token, m.token);
    }
}
