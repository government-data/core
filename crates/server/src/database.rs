//! The database pool: one installation is one organisation (decision 0076);
//! the rows' `tenant_id` takes the main tenant from the database's default
//! until the schema's contraction.

use infrastructure::PgConfig;

/// The pool.
pub(crate) struct Database {
    pub(crate) pool: infrastructure::PgPool,
}

/// Connect (eagerly, so a bad DSN or a down database fails now) and bring the
/// schema up to date. `run_migrations` also validates the checksums of the
/// migrations already applied, so schema drift is caught at startup.
pub(crate) async fn connect(pg_config: &PgConfig) -> Result<Database, Box<dyn std::error::Error>> {
    let pool = infrastructure::connect(pg_config).await?;
    infrastructure::run_migrations(&pool).await?;
    Ok(Database { pool })
}
