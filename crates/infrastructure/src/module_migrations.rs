//! The feature modules' migrations on PostgreSQL: the database-backed
//! [`MigrationExecutor`] the `contracts` seam was waiting for.
//!
//! The registry plans the migrations synchronously (`is_applied`, `apply`);
//! this executor loads what is applied first, collects the pending ones as
//! the registry hands them over, and [`PgModuleMigrator::run`] then applies
//! each in its own transaction and records it with its checksum.

use std::collections::HashMap;
use std::error::Error;

use contracts::MigrationExecutor;
use sha2::{Digest, Sha256};
use sqlx::PgPool;

/// What went wrong applying a module migration.
#[derive(Debug)]
pub enum ModuleMigrationError {
    Database(sqlx::Error),
    /// A migration already applied has changed since: migrations only add.
    Changed {
        id: String,
    },
}

impl std::fmt::Display for ModuleMigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(e) => write!(f, "module migrations: {e}"),
            Self::Changed { id } => {
                write!(f, "module migration {id} was applied and has changed since")
            }
        }
    }
}

impl Error for ModuleMigrationError {}

impl From<sqlx::Error> for ModuleMigrationError {
    fn from(e: sqlx::Error) -> Self {
        Self::Database(e)
    }
}

/// Plans against what the database has applied, then applies the rest.
#[derive(Debug, Default)]
pub struct PgModuleMigrator {
    applied: HashMap<String, String>,
    pending: Vec<(String, String)>,
}

/// The advisory lock the modules' migrations hold while they run.
const LOCK: i64 = 0x0071_6d6f_6475_6c65; // "qmodule"

fn checksum(sql: &str) -> String {
    Sha256::digest(sql.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl PgModuleMigrator {
    /// Reads the module migrations already applied.
    pub async fn load(pool: &PgPool) -> Result<Self, ModuleMigrationError> {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT id, checksum FROM public._module_migrations")
                .fetch_all(pool)
                .await?;
        Ok(Self {
            applied: rows.into_iter().collect(),
            pending: Vec::new(),
        })
    }

    /// Refuses a plan whose applied migrations changed: pass every
    /// registered migration (`<module>::<name>`, SQL).
    pub fn verify<'a>(
        &self,
        all: impl IntoIterator<Item = (String, &'a str)>,
    ) -> Result<(), ModuleMigrationError> {
        for (id, sql) in all {
            if self.applied.get(&id).is_some_and(|c| *c != checksum(sql)) {
                return Err(ModuleMigrationError::Changed { id });
            }
        }
        Ok(())
    }

    /// Applies the pending migrations, each in its own transaction, under
    /// an advisory lock: two processes starting together (`bootstrap-admin`
    /// and `serve`, or two replicas) apply each migration once. What another
    /// process applied while this one waited is skipped.
    pub async fn run(self, pool: &PgPool) -> Result<Vec<String>, ModuleMigrationError> {
        let mut conn = pool.acquire().await?;
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(LOCK)
            .execute(&mut *conn)
            .await?;
        let outcome = self.apply_pending(&mut conn).await;
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(LOCK)
            .execute(&mut *conn)
            .await?;
        outcome
    }

    async fn apply_pending(
        self,
        conn: &mut sqlx::PgConnection,
    ) -> Result<Vec<String>, ModuleMigrationError> {
        let done: Vec<String> = sqlx::query_scalar("SELECT id FROM public._module_migrations")
            .fetch_all(&mut *conn)
            .await?;
        let mut applied = Vec::new();
        for (id, sql) in self
            .pending
            .into_iter()
            .filter(|(id, _)| !done.contains(id))
        {
            let mut tx = sqlx::Connection::begin(&mut *conn).await?;
            sqlx::raw_sql(&sql).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO public._module_migrations (id, checksum) VALUES ($1, $2)")
                .bind(&id)
                .bind(checksum(&sql))
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            applied.push(id);
        }
        Ok(applied)
    }
}

impl MigrationExecutor for PgModuleMigrator {
    fn is_applied(&self, id: &str) -> bool {
        self.applied.contains_key(id) || self.pending.iter().any(|(p, _)| p == id)
    }

    fn apply(&mut self, id: &str, sql: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.pending.push((id.to_string(), sql.to_string()));
        Ok(())
    }
}
